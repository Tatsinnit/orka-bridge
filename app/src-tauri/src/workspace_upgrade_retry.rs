//! Explicit retry retains incomplete destinations; it never recovers creation authority.
use super::*;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RetainedDestination {
    installation: Installation,
    binding: Option<String>,
    pub(super) model: Option<Vec<u8>>,
    pub(super) account: Option<Vec<u8>>,
}
impl RetainedDestination {
    fn name(&self) -> String {
        format!("retained-incomplete-{}", self.installation.installation)
    }
}
impl WorkspaceUpgrade {
    pub(super) fn archived_slots(&self, intent: &Intent) -> Result<Vec<Uuid>, RuntimeError> {
        intent
            .restore
            .iter()
            .map(|r| r.model.as_deref())
            .chain(intent.incomplete.iter().map(|b| b.model.as_deref()))
            .filter_map(|model| match SetupStore::upgrade_slot(model) {
                Ok(None) => None,
                Ok(Some(slot)) => Some(Ok(slot)),
                Err(_) => Some(Err(RuntimeError::RetainedState)),
            })
            .collect()
    }
    pub(super) fn pending_retention<'a>(
        &self,
        intent: &'a Intent,
    ) -> Option<&'a RetainedDestination> {
        intent
            .incomplete
            .last()
            .filter(|b| Some(&b.installation) == intent.destination.as_ref())
    }
    pub(super) fn validate_incomplete(
        &self,
        intent: &Intent,
        canonical: Option<&Installation>,
    ) -> Result<(), RuntimeError> {
        let mut seen = std::collections::HashSet::new();
        for b in &intent.incomplete {
            if b.installation.installation.is_nil()
                || b.installation.installation == intent.source.installation
                || !seen.insert(b.installation.installation)
            {
                return Err(RuntimeError::RetainedState);
            }
            let retained = self.local.join(b.name());
            let actual = installation(&retained)?;
            let root = if actual.as_ref() == Some(&b.installation) {
                if canonical == Some(&b.installation) {
                    return Err(RuntimeError::RetainedState);
                }
                retained
            } else if actual.is_none()
                && self
                    .pending_retention(intent)
                    .is_some_and(|last| std::ptr::eq(last, b))
                && canonical == Some(&b.installation)
            {
                self.local.join("local-vm-v1")
            } else {
                return Err(RuntimeError::RetainedState);
            };
            if let Some(hash) = &b.binding {
                verify_destination_binding(&root, &intent.target_binding, hash)?;
            }
        }
        Ok(())
    }
    /// A durable pending snapshot makes each metadata removal and rename replayable.
    /// Caller holds transaction and model custody; native lifecycle has already stopped.
    pub(super) fn complete_retention(
        &self,
        intent: &mut Intent,
        point: &impl Fn(&str) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let Some(b) = self.pending_retention(intent) else {
            return Ok(());
        };
        let root = self.local.join("local-vm-v1");
        let canonical = installation(&root)?;
        self.validate_incomplete(intent, canonical.as_ref())?;
        if canonical.as_ref() == Some(&b.installation) {
            let owner = Journal::recovery_owner(&root)?;
            for (name, snapshot, boundary) in [
                ("model-v1", &b.model, "retry-model-archived"),
                ("model-account-v1", &b.account, "retry-account-archived"),
            ] {
                let path = self.data.join("setup-v1").join(name);
                let current = read_bytes(&path, 16384)?;
                if current.is_some() && current != *snapshot {
                    return Err(RuntimeError::RetainedState);
                }
                if current.is_some() {
                    fs::remove_file(path)?;
                    image_cache::fs::sync(&Dir::open_ambient_dir(
                        self.data.join("setup-v1"),
                        cap_std::ambient_authority(),
                    )?)?;
                }
                point(boundary)?;
            }
            drop(owner); // Windows deny-sharing custody must end before rename.
            let parent = Dir::open_ambient_dir(&self.local, cap_std::ambient_authority())?;
            image_cache::fs::publish_named(&parent, "local-vm-v1", &b.name())?;
            image_cache::fs::sync(&parent)?;
            point("retry-destination-retained")?;
        } else if canonical.is_some() {
            return Err(RuntimeError::RetainedState);
        }
        intent.destination = None;
        intent.destination_binding_sha256 = None;
        intent.phase = Phase::SourceRetained;
        if !matches!(intent.credential, CredentialDisposition::Removed) {
            intent.credential = if intent.model.is_some() {
                CredentialDisposition::Reserved
            } else {
                CredentialDisposition::Absent
            };
        }
        self.persist(intent)?;
        point("retry-recorded")?;
        Ok(())
    }
    pub(crate) fn retry(
        &mut self,
        transaction: Uuid,
        source: &ImageSource,
        machine_id: Vec<u8>,
        closed: &watch::Receiver<bool>,
        runtime: &tokio::runtime::Handle,
    ) -> Result<UpgradeStatus, RuntimeError> {
        let cache = self.local.join("image-cache-v1");
        let progress = self.progress.clone();
        self.retry_with(
            transaction,
            source,
            machine_id,
            closed,
            runtime,
            || {
                let (profile, host) = match source {
                    ImageSource::Pinned { profile, host } => (profile, host),
                    #[cfg(debug_assertions)]
                    ImageSource::Development => {
                        return Err(ImageError::WorkspaceVersionUnsupported.into())
                    }
                };
                let cache = image_cache::ImageCache::open(&cache)?;
                let client = crate::runtime::acquire::ImageHttpClient::new(&profile.pin)?;
                Ok(
                    runtime.block_on(crate::runtime::setup_adapter::prepare_pinned_image(
                        &profile.pin,
                        host,
                        &cache,
                        &client,
                        closed.clone(),
                        move |event| {
                            if let Some(progress) = &progress {
                                progress(event);
                            }
                        },
                    ))?,
                )
            },
            |_| Ok(()),
            available_space,
        )
    }
    #[allow(clippy::too_many_arguments)] // Explicit native authority and acquisition/cancellation boundaries.
    pub(super) fn retry_with(
        &mut self,
        transaction: Uuid,
        source: &ImageSource,
        machine_id: Vec<u8>,
        closed: &watch::Receiver<bool>,
        runtime: &tokio::runtime::Handle,
        acquire: impl FnOnce() -> Result<PreparedImage, RuntimeError>,
        point: impl Fn(&str) -> Result<(), RuntimeError>,
        space: impl FnOnce(&Dir) -> Result<u64, ImageError>,
    ) -> Result<UpgradeStatus, RuntimeError> {
        cancelled(closed)?;
        let mut intent = self.read_intent()?.ok_or(RuntimeError::RetainedState)?;
        if intent.transaction != transaction || intent.restore.is_some() {
            return Err(RuntimeError::RetainedState);
        }
        let status = self.recover()?;
        if !matches!(
            status,
            UpgradeStatus::SourceSelected
                | UpgradeStatus::SourceRetained
                | UpgradeStatus::Incomplete
        ) {
            return Err(RuntimeError::RetainedState);
        }
        let root = self.local.join("local-vm-v1");
        let source_root = if status == UpgradeStatus::SourceSelected {
            root.clone()
        } else {
            self.local.join(&intent.retained)
        };
        let journal = Journal::open(&source_root)?;
        let backend = journal.record().backend;
        let (old, _, target, _) = source.upgrade_eligibility(&journal)?;
        if old != intent.source_binding || target != intent.target_binding {
            return Err(RuntimeError::RetainedState);
        }
        drop(journal);
        // Lock incomplete B before acquisition too: a live/uncertain owner is not a retry candidate.
        let custody = installation(&root)?
            .filter(|b| b != &intent.source)
            .map(|_| Journal::recovery_owner(&root))
            .transpose()?;
        let prepared = acquire()?;
        if prepared.identity() != &intent.target_binding {
            return Err(ImageError::Integrity.into());
        }
        let additional = prepared.required_install_bytes()?;
        let parent = Dir::open_ambient_dir(&self.local, cap_std::ambient_authority())?;
        if space(&parent)? < additional {
            return Err(ImageError::NoSpace.into());
        }
        cancelled(closed)?;
        point("retry-acquired")?;
        drop(custody);
        let setup = SetupStore::new(Some(self.data.clone()));
        let (_snapshot, _owner);
        if let Some(destination) = intent
            .destination
            .as_ref()
            .filter(|_| self.pending_retention(&intent).is_none())
        {
            let snapshot = setup
                .snapshot_for_upgrade(destination.installation)
                .map_err(|_| RuntimeError::RetainedState)?;
            intent.retained_allocated_bytes = intent
                .retained_allocated_bytes
                .checked_add(allocated(&root, None, true)?)
                .ok_or(ImageError::NoSpace)?;
            intent.incomplete.push(RetainedDestination {
                installation: destination.clone(),
                binding: intent.destination_binding_sha256.clone(),
                model: snapshot.model.clone(),
                account: snapshot.account.clone(),
            });
            _snapshot = Some(snapshot);
            _owner = None;
            self.validate_incomplete(&intent, installation(&root)?.as_ref())?;
            self.persist(&intent)?;
            point("retry-intent")?;
        } else {
            _snapshot = None;
            _owner = Some(
                setup
                    .lock_upgrade_metadata()
                    .map_err(|_| RuntimeError::RetainedState)?,
            );
        }
        self.complete_retention(&mut intent, &point)?;
        cancelled(closed)?;
        // Before A's first rename, either snapshot file may already have been removed.
        if installation(&root)?.as_ref() == Some(&intent.source) {
            let owner = Journal::recovery_owner(&root)?;
            for (name, snapshot) in self.source_metadata(&intent)? {
                let path = self.data.join("setup-v1").join(name);
                let current = read_bytes(&path, 16384)?;
                if current.is_some() && current != *snapshot {
                    return Err(RuntimeError::RetainedState);
                }
                if current.is_some() {
                    fs::remove_file(path)?;
                    image_cache::fs::sync(&Dir::open_ambient_dir(
                        self.data.join("setup-v1"),
                        cap_std::ambient_authority(),
                    )?)?;
                }
            }
            drop(owner);
            image_cache::fs::publish_named(&parent, "local-vm-v1", &intent.retained)?;
            image_cache::fs::sync(&parent)?;
            point("source-retained")?;
        }
        if installation(&root)?.is_some() {
            return Err(RuntimeError::RetainedState);
        }
        intent.phase = Phase::SourceRetained;
        intent.additional_bytes = additional;
        self.persist(&intent)?;
        cancelled(closed)?;
        let destination = Journal::create(&root, backend, machine_id)?;
        point("destination-created")?;
        intent.destination = installation(&root)?;
        intent.phase = Phase::DestinationCreated;
        self.persist(&intent)?;
        point("destination-recorded")?;
        let progress = self.progress.clone();
        let image = runtime.block_on(prepared.materialize(
            &destination.image_target(source.compatibility())?,
            closed.clone(),
            move |event| {
                if let Some(progress) = &progress {
                    progress(event);
                }
            },
        ))?;
        destination.validate_image(&image)?;
        intent.destination_binding_sha256 = Some(binding_hash(&root)?);
        intent.phase = Phase::ImagePrepared;
        self.persist(&intent)?;
        self.live_destination = Some(destination);
        point("image-prepared")?;
        Ok(UpgradeStatus::Fresh)
    }
}
