//! Native-only forward replacement. No renderer command may enter this backend
//! until restore and shared-credential guards are complete.
#![allow(dead_code)]
use crate::runtime::{
    identity::{file_identity, FileIdentity},
    image_cache::{self, available_space},
    image_install::allocation,
    image_types::{ImageError, ImageIdentity},
    journal::{InstallationRecord, Journal, StatePhase},
    types::RuntimeError,
};
#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
use crate::{
    local_vm::native::image::ImageSource, runtime::image_install::PreparedImage, setup::SetupStore,
};
use bridge_runtime_protocol::Uuid;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tokio::sync::watch;

const STORE: &str = "workspace-upgrades-v1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Installation {
    installation: Uuid,
    directory: FileIdentity,
    state: FileIdentity,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Intent,
    MetadataArchived,
    SourceRetained,
    DestinationCreated,
    ImagePrepared,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    version: u16,
    transaction: Uuid,
    created_at: u64,
    source: Installation,
    destination: Option<Installation>,
    source_binding: ImageIdentity,
    target_binding: ImageIdentity,
    source_binding_sha256: String,
    destination_binding_sha256: Option<String>,
    source_releases: Vec<String>,
    target_release: String,
    retained: String,
    phase: Phase,
    model: Option<Vec<u8>>,
    account: Option<Vec<u8>>,
    credential: CredentialDisposition,
    retained_allocated_bytes: u64,
    additional_bytes: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CredentialDisposition {
    Absent,
    Reserved,
}
#[derive(Debug, PartialEq)]
pub(crate) enum UpgradeStatus {
    None,
    SourceSelected,
    SourceRetained,
    Fresh,
    Incomplete,
    Consumed,
    Conflict,
}
pub(crate) struct WorkspaceUpgrade {
    data: PathBuf,
    local: PathBuf,
    directory: Dir,
    _owner: File,
    live_destination: Option<Journal>,
}
fn directory_identity(path: &Path) -> Result<FileIdentity, RuntimeError> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(RuntimeError::RetainedState);
    }
    let dir = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
    image_cache::not_reparse(&dir.try_clone()?.into_std_file())?;
    file_identity(&dir.into_std_file())
}
fn read_bytes(path: &Path, max: u64) -> Result<Option<Vec<u8>>, RuntimeError> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.file_type().is_file() && m.len() <= max => {}
        Ok(_) => return Err(RuntimeError::RetainedState),
        Err(e) => return Err(e.into()),
    }
    let file = File::open(path)?;
    image_cache::not_reparse(&file)?;
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(RuntimeError::RetainedState);
    }
    Ok(Some(bytes))
}
fn read_record(root: &Path) -> Result<InstallationRecord, RuntimeError> {
    let bytes = read_bytes(&root.join("record.json"), 32768)?.ok_or(RuntimeError::RetainedState)?;
    let record: InstallationRecord =
        serde_json::from_slice(&bytes).map_err(|_| RuntimeError::RetainedState)?;
    if record.version != 1 || record.installation_id.is_nil() || record.generation == 0 {
        return Err(RuntimeError::RetainedState);
    }
    Ok(record)
}
fn installation(root: &Path) -> Result<Option<Installation>, RuntimeError> {
    match fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(_) => {}
    }
    let directory = directory_identity(root)?;
    let record = read_record(root)?;
    let file = File::open(root.join("state.disk"))?;
    image_cache::not_reparse(&file)?;
    if file_identity(&file)? != record.state_identity {
        return Err(RuntimeError::RetainedState);
    }
    Ok(Some(Installation {
        installation: record.installation_id,
        directory,
        state: record.state_identity,
    }))
}
fn allocated(root: &Path, journal: Option<&Journal>) -> Result<u64, RuntimeError> {
    directory_identity(root)?;
    let mut bytes = 0u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        #[cfg(windows)]
        if journal.is_some()
            && kind.is_dir()
            && entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_prefix("c-"))
                .is_some_and(|nonce| {
                    nonce.len() == 32 && nonce.bytes().all(|b| b.is_ascii_hexdigit())
                })
        {
            // Only top-level native control namespaces admit AF_UNIX reparses;
            // the endpoint owner rechecks their exact tag, principals and ACL.
            let n = crate::runtime::windows::control::retained_allocation(&entry.path())?;
            bytes = bytes.checked_add(n).ok_or(ImageError::NoSpace)?;
            continue;
        }
        let n = if kind.is_dir() {
            allocated(&entry.path(), None)?
        } else if kind.is_file() {
            if let Some(journal) = journal.filter(|_| entry.file_name() == "owner.lock") {
                journal.owner_allocated_bytes()?
            } else {
                let file = File::open(entry.path())?;
                image_cache::not_reparse(&file)?;
                allocation(&file)?
            }
        } else {
            return Err(RuntimeError::RetainedState);
        };
        bytes = bytes.checked_add(n).ok_or(ImageError::NoSpace)?;
    }
    Ok(bytes)
}
fn binding_hash(root: &Path) -> Result<String, RuntimeError> {
    let bytes = read_bytes(&root.join("image-binding.json"), 256 * 1024)?
        .ok_or(RuntimeError::RetainedState)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn verify_binding(root: &Path, identity: &ImageIdentity, hash: &str) -> Result<(), RuntimeError> {
    if binding_hash(root)? != hash {
        return Err(RuntimeError::RetainedState);
    }
    let journal = Journal::open(root)?;
    let installed = crate::runtime::image_install::inspect_installed_image(
        &journal.image_target(&identity.state_compatibility_id)?,
    )?;
    if installed.identity() != identity {
        return Err(RuntimeError::RetainedState);
    }
    Ok(())
}
fn cancelled(closed: &watch::Receiver<bool>) -> Result<(), RuntimeError> {
    if *closed.borrow() {
        Err(RuntimeError::Closed)
    } else {
        Ok(())
    }
}
impl WorkspaceUpgrade {
    pub(crate) fn open(data: &Path, local: &Path) -> Result<Self, RuntimeError> {
        Self::open_with_sync(data, local, image_cache::fs::sync)
    }
    fn open_with_sync(
        data: &Path,
        local: &Path,
        sync: impl FnOnce(&Dir) -> Result<(), ImageError>,
    ) -> Result<Self, RuntimeError> {
        let local_dir = Dir::open_ambient_dir(local, cap_std::ambient_authority())?;
        let directory = image_cache::directory(&local_dir, STORE.as_ref())?;
        // The archive directory itself must survive before D's records can be removed.
        sync(&local_dir)?;
        let mut options = cap_std::fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        let owner = directory.open_with("owner.lock", &options)?.into_std();
        image_cache::not_reparse(&owner)?;
        owner.try_lock().map_err(|e| match e {
            fs::TryLockError::WouldBlock => RuntimeError::Busy,
            fs::TryLockError::Error(e) => e.into(),
        })?;
        Ok(Self {
            data: data.to_owned(),
            local: local.to_owned(),
            directory,
            _owner: owner,
            live_destination: None,
        })
    }
    fn read_intent(&self) -> Result<Option<Intent>, RuntimeError> {
        let path = self.local.join(STORE).join("intent.json");
        let Some(bytes) = read_bytes(&path, 128 * 1024)? else {
            if self.directory.try_exists("intent.next")? {
                return Err(RuntimeError::RetainedState);
            }
            return Ok(None);
        };
        let intent: Intent =
            serde_json::from_slice(&bytes).map_err(|_| RuntimeError::RetainedState)?;
        if intent.version != 1
            || intent.transaction.is_nil()
            || intent.source.installation.is_nil()
            || intent.retained != format!("retained-{}", intent.transaction)
            || intent.source_releases.is_empty()
            || intent.target_release.is_empty()
            || intent.destination.as_ref().is_some_and(|b| {
                b.installation.is_nil() || b.installation == intent.source.installation
            })
        {
            return Err(RuntimeError::RetainedState);
        }
        Ok(Some(intent))
    }
    fn persist(&self, intent: &Intent) -> Result<(), RuntimeError> {
        let bytes = serde_json::to_vec(intent).map_err(|_| RuntimeError::RetainedState)?;
        let path = self.local.join(STORE);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path.join("intent.next"))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        crate::runtime::journal::replace(&path.join("intent.next"), &path.join("intent.json"))?;
        image_cache::fs::sync(&self.directory)?;
        Ok(())
    }
    pub(crate) fn inspect(&self) -> Result<UpgradeStatus, RuntimeError> {
        self.recover()
    }
    /// Reconcile identities, not the last phase label. No recovery path synthesizes
    /// creation authority or moves an unexpected canonical directory out of the way.
    pub(crate) fn recover(&self) -> Result<UpgradeStatus, RuntimeError> {
        let Some(intent) = self.read_intent()? else {
            return Ok(UpgradeStatus::None);
        };
        let canonical = installation(&self.local.join("local-vm-v1"))?;
        let retained = installation(&self.local.join(&intent.retained))?;
        for (name, snapshot) in [
            ("model-v1", &intent.model),
            ("model-account-v1", &intent.account),
        ] {
            let current = read_bytes(&self.data.join("setup-v1").join(name), 16384)?;
            if current.is_some() && current != *snapshot {
                return Err(RuntimeError::RetainedState);
            }
        }
        let source_root = if retained.as_ref() == Some(&intent.source) {
            self.local.join(&intent.retained)
        } else if canonical.as_ref() == Some(&intent.source) {
            self.local.join("local-vm-v1")
        } else {
            return Err(RuntimeError::RetainedState);
        };
        verify_binding(
            &source_root,
            &intent.source_binding,
            &intent.source_binding_sha256,
        )?;
        match (canonical.as_ref(), retained.as_ref()) {
            (Some(a), None) if a == &intent.source && intent.destination.is_none() => {
                Ok(UpgradeStatus::SourceSelected)
            }
            (None, Some(a)) if a == &intent.source => Ok(UpgradeStatus::SourceRetained),
            (Some(b), Some(a)) if a == &intent.source && Some(b) == intent.destination.as_ref() => {
                if self.live_destination.is_some() {
                    return Ok(UpgradeStatus::Fresh);
                }
                let root = self.local.join("local-vm-v1");
                let phase = read_record(&root)?.phase;
                if phase != StatePhase::Preparing {
                    if let Some(hash) = &intent.destination_binding_sha256 {
                        verify_binding(&root, &intent.target_binding, hash)?;
                    } else if phase == StatePhase::Consumed {
                        return Err(RuntimeError::RetainedState);
                    }
                }
                Ok(if phase == StatePhase::Consumed {
                    UpgradeStatus::Consumed
                } else {
                    UpgradeStatus::Incomplete
                })
            }
            _ => Err(RuntimeError::RetainedState),
        }
    }
    #[cfg(any(
        test,
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    pub(crate) fn replace(
        &mut self,
        source: &ImageSource,
        machine_id: Vec<u8>,
        closed: &watch::Receiver<bool>,
        runtime: &tokio::runtime::Handle,
    ) -> Result<UpgradeStatus, RuntimeError> {
        let cache_path = self.local.join("image-cache-v1");
        self.replace_with(
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
                let cache = image_cache::ImageCache::open(&cache_path)?;
                let client = crate::runtime::acquire::ImageHttpClient::new(&profile.pin)?;
                Ok(
                    runtime.block_on(crate::runtime::setup_adapter::prepare_pinned_image(
                        &profile.pin,
                        host,
                        &cache,
                        &client,
                        closed.clone(),
                        |_| {},
                    ))?,
                )
            },
            |_| Ok(()),
            available_space,
        )
    }
    #[cfg(any(
        test,
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    #[allow(clippy::too_many_arguments)] // Explicit native authority and acquisition/cancellation boundaries.
    fn replace_with(
        &mut self,
        source: &ImageSource,
        machine_id: Vec<u8>,
        closed: &watch::Receiver<bool>,
        runtime: &tokio::runtime::Handle,
        acquire: impl FnOnce() -> Result<PreparedImage, RuntimeError>,
        point: impl Fn(&str) -> Result<(), RuntimeError>,
        space: impl FnOnce(&Dir) -> Result<u64, ImageError>,
    ) -> Result<UpgradeStatus, RuntimeError> {
        cancelled(closed)?;
        if self.read_intent()?.is_some() {
            return Err(RuntimeError::RetainedState);
        }
        let root = self.local.join("local-vm-v1");
        // Opening A proves the native lifecycle has released *all* journal custody.
        let journal = Journal::open(&root)?;
        let (source_binding, source_releases, target_binding, target_release) =
            source.upgrade_eligibility(&journal)?;
        let metadata = SetupStore::new(Some(self.data.clone()))
            .snapshot_for_upgrade(journal.record().installation_id)
            .map_err(|_| RuntimeError::RetainedState)?;
        let prepared = acquire()?;
        if prepared.identity() != &target_binding {
            return Err(ImageError::Integrity.into());
        }
        cancelled(closed)?;
        point("acquired")?;
        let additional_bytes = prepared.required_install_bytes()?;
        let parent = Dir::open_ambient_dir(&self.local, cap_std::ambient_authority())?;
        if space(&parent)? < additional_bytes {
            return Err(ImageError::NoSpace.into());
        }
        let transaction = Uuid::new_v4();
        let mut intent = Intent {
            version: 1,
            transaction,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| RuntimeError::Unavailable)?
                .as_secs(),
            source: installation(&root)?.ok_or(RuntimeError::RetainedState)?,
            destination: None,
            source_binding,
            target_binding,
            source_binding_sha256: binding_hash(&root)?,
            destination_binding_sha256: None,
            source_releases,
            target_release,
            retained: format!("retained-{transaction}"),
            phase: Phase::Intent,
            credential: if metadata.model.is_some() {
                CredentialDisposition::Reserved
            } else {
                CredentialDisposition::Absent
            },
            model: metadata.model.clone(),
            account: metadata.account.clone(),
            retained_allocated_bytes: allocated(&root, Some(&journal))?,
            additional_bytes,
        };
        let backend = journal.record().backend;
        self.persist(&intent)?;
        point("intent")?;
        // L's durable snapshot precedes each independent D metadata removal.
        for (name, snapshot, boundary) in [
            ("model-v1", &intent.model, "model-archived"),
            ("model-account-v1", &intent.account, "account-archived"),
        ] {
            cancelled(closed)?;
            let path = self.data.join("setup-v1").join(name);
            if read_bytes(&path, 16384)? != *snapshot {
                return Err(RuntimeError::RetainedState);
            }
            if snapshot.is_some() {
                fs::remove_file(path)?;
                let setup = Dir::open_ambient_dir(
                    self.data.join("setup-v1"),
                    cap_std::ambient_authority(),
                )?;
                image_cache::fs::sync(&setup)?;
            }
            point(boundary)?;
        }
        intent.phase = Phase::MetadataArchived;
        self.persist(&intent)?;
        cancelled(closed)?;
        drop(journal); // Windows deny-sharing handles must not survive the rename.
        if installation(&root)?.as_ref() != Some(&intent.source) {
            return Err(RuntimeError::RetainedState);
        }
        image_cache::fs::publish_named(&parent, "local-vm-v1", &intent.retained)?;
        image_cache::fs::sync(&parent)?;
        point("source-retained")?;
        intent.phase = Phase::SourceRetained;
        self.persist(&intent)?;
        cancelled(closed)?;
        let destination = Journal::create(&root, backend, machine_id)?;
        point("destination-created")?;
        intent.destination = installation(&root)?;
        intent.phase = Phase::DestinationCreated;
        self.persist(&intent)?;
        point("destination-recorded")?;
        let image = runtime.block_on(prepared.materialize(
            &destination.image_target(source.compatibility())?,
            closed.clone(),
            |_| {},
        ))?;
        destination.validate_image(&image)?;
        intent.destination_binding_sha256 = Some(binding_hash(&root)?);
        intent.phase = Phase::ImagePrepared;
        self.persist(&intent)?;
        self.live_destination = Some(destination);
        point("image-prepared")?;
        Ok(UpgradeStatus::Fresh)
    }
    /// Only the creating process may hand B to its native worker. Recovery cannot.
    pub(crate) fn take_destination(&mut self) -> Option<Journal> {
        self.live_destination.take()
    }
}
/// Ordinary launch remains unavailable for *any* transaction until the
/// association/deletion guards are implemented. No transaction preserves existing startup exactly.
pub(crate) fn admit(root: &Path) -> Result<(), RuntimeError> {
    let local = root.parent().ok_or(RuntimeError::RetainedState)?;
    let path = local.join(STORE);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
        Ok(_) => {
            directory_identity(&path)?;
            if read_bytes(&path.join("intent.json"), 128 * 1024)?.is_some()
                || path.join("intent.next").try_exists()?
            {
                Err(RuntimeError::RetainedState)
            } else {
                Ok(())
            }
        }
    }
}
/// Runs before ordinary VM configuration and model mismatch processing.
pub fn recover_startup(data: &Path, local: &Path) -> Result<(), RuntimeError> {
    if !local.join(STORE).try_exists()? {
        return Ok(());
    }
    let owner = WorkspaceUpgrade::open(data, local)?;
    owner.recover()?;
    admit(&local.join("local-vm-v1"))
}
#[cfg(test)]
#[path = "workspace_upgrade_tests.rs"]
mod tests;
