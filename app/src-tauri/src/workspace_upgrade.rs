//! Native-only replacement and recovery-only restore. No renderer command may enter this backend
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
use crate::setup::SetupStore;
use crate::{local_vm::native::image::ImageSource, runtime::image_install::PreparedImage};
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

#[path = "workspace_upgrade_retry.rs"]
mod retry;
const STORE: &str = "workspace-upgrades-v1";
type MetadataFiles<'a> = [(&'static str, &'a Option<Vec<u8>>); 2];
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
    Restoring,
    Restored,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Restore {
    model: Option<Vec<u8>>,
    account: Option<Vec<u8>>,
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
    #[serde(default)]
    restore: Option<Restore>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    incomplete: Vec<retry::RetainedDestination>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CredentialDisposition {
    Absent,
    Reserved,
    Transferring,
    Transferred,
    Removed,
}
#[derive(Debug, PartialEq)]
pub(crate) enum UpgradeStatus {
    None,
    SourceSelected,
    SourceRetained,
    Fresh,
    Incomplete,
    Consumed,
    Restoring,
    Restored,
    Conflict,
}
/// The only shape the renderer ever sees: no secret, slot, path or raw intent
/// bytes. Every field is native-selected; the renderer supplies no source,
/// image URL or restore target, only an opaque `transaction` echoed back.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStatusDto {
    pub state: &'static str,
    pub transaction: Option<Uuid>,
    pub created_at: Option<u64>,
    pub source_releases: Vec<String>,
    pub target_release: Option<String>,
    pub retained_allocated_bytes: Option<u64>,
    pub additional_bytes: Option<u64>,
    pub credential: &'static str,
    pub busy: bool,
    pub error: Option<&'static str>,
    pub repeated: bool,
    pub can_replace: bool,
    pub can_restore: bool,
    pub can_cancel: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpgradeCancelOutcome {
    Requested,
    Completed,
    NotCancellable,
}
#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum UpgradeActionReply {
    Completed { status: UpgradeStatusDto },
    Cancelled { status: UpgradeStatusDto },
}
impl UpgradeStatusDto {
    pub(crate) fn none() -> Self {
        Self {
            state: "none",
            transaction: None,
            created_at: None,
            source_releases: vec![],
            target_release: None,
            retained_allocated_bytes: None,
            additional_bytes: None,
            credential: "absent",
            busy: false,
            error: None,
            repeated: false,
            can_replace: false,
            can_restore: false,
            can_cancel: false,
        }
    }
    pub(crate) fn conflict(error: Option<&'static str>) -> Self {
        Self {
            state: "conflict",
            error,
            ..Self::none()
        }
    }
}
pub(crate) fn classify_error(error: &RuntimeError) -> Option<&'static str> {
    Some(match error {
        RuntimeError::Closed => return None,
        RuntimeError::Busy | RuntimeError::Image(ImageError::Busy) => "inUse",
        RuntimeError::StopUnconfirmed(_) => "stopUnconfirmed",
        RuntimeError::Io(e) if e.kind() == std::io::ErrorKind::StorageFull => "noSpace",
        RuntimeError::Image(ImageError::NoSpace) => "noSpace",
        RuntimeError::Image(ImageError::Integrity) => "integrity",
        RuntimeError::Image(
            ImageError::WorkspaceUpdateRequired | ImageError::WorkspaceVersionUnsupported,
        ) => "unsupported",
        RuntimeError::Image(ImageError::Network(_)) => "network",
        RuntimeError::Image(ImageError::Cancelled) => return None,
        RuntimeError::RetainedState => "unknown",
        _ => "unknown",
    })
}
/// No transaction yet, and no upgrade store needs to exist to answer this:
/// only the canonical installation directory is inspected, read-only.
fn eligible_status(
    data: &Path,
    local: &Path,
    source: Option<&ImageSource>,
) -> Result<UpgradeStatusDto, RuntimeError> {
    let Some(source) = source else {
        return Ok(UpgradeStatusDto::none());
    };
    let root = local.join("local-vm-v1");
    if !root.try_exists()? {
        return Ok(UpgradeStatusDto::none());
    }
    let journal = match Journal::open(&root) {
        Ok(journal) => journal,
        Err(RuntimeError::Busy) => return Err(RuntimeError::Busy),
        Err(_) => return Ok(UpgradeStatusDto::none()),
    };
    let (_source_binding, source_releases, _target_binding, target_release) =
        match source.upgrade_eligibility(&journal) {
            Ok(eligibility) => eligibility,
            Err(_) => return Ok(UpgradeStatusDto::none()),
        };
    let retained_allocated_bytes = allocated(&root, Some(&journal), true)?;
    let metadata = SetupStore::new(Some(data.to_owned()))
        .snapshot_for_upgrade(journal.record().installation_id)
        .map_err(|_| RuntimeError::RetainedState)?;
    let credential =
        saved_credential_label(metadata.model.as_deref(), journal.record().installation_id)?;
    Ok(UpgradeStatusDto {
        state: "eligible",
        source_releases,
        target_release: Some(target_release),
        retained_allocated_bytes: Some(retained_allocated_bytes),
        additional_bytes: Some(source.additional_space_bound()?),
        credential,
        can_replace: true,
        ..UpgradeStatusDto::none()
    })
}
fn saved_credential_label(
    model: Option<&[u8]>,
    installation: Uuid,
) -> Result<&'static str, RuntimeError> {
    if model.is_none() {
        return Ok("absent");
    }
    Ok(
        if SetupStore::transferred_model(model, installation)
            .map_err(|_| RuntimeError::RetainedState)?
            .is_some()
        {
            "eligible"
        } else {
            "attention"
        },
    )
}
pub(crate) struct WorkspaceUpgrade {
    data: PathBuf,
    local: PathBuf,
    directory: Dir,
    _owner: File,
    owner_process: u32,
    live_destination: Option<Journal>,
    progress:
        Option<std::sync::Arc<dyn Fn(crate::runtime::image_types::ImageProgress) + Send + Sync>>,
}
impl Drop for WorkspaceUpgrade {
    fn drop(&mut self) {
        // A concurrent fork may inherit even CLOEXEC descriptors until exec.
        // Only this process owns the transaction; a child must not unlock it.
        if self.owner_process == std::process::id() {
            let _ = self._owner.unlock();
        }
    }
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
// Incomplete installations may already have native endpoints but no reopenable
// journal. Only the installation root admits their validated native namespace.
fn allocated(
    root: &Path,
    journal: Option<&Journal>,
    _native_root: bool,
) -> Result<u64, RuntimeError> {
    directory_identity(root)?;
    let mut bytes = 0u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        #[cfg(windows)]
        if _native_root
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
            allocated(&entry.path(), None, false)?
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
fn verify_destination_binding(
    root: &Path,
    identity: &ImageIdentity,
    hash: &str,
) -> Result<(), RuntimeError> {
    if binding_hash(root)? != hash {
        return Err(RuntimeError::RetainedState);
    }
    let installed = Journal::inspect_recovery_image(root, &identity.state_compatibility_id)?;
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
    pub(crate) fn set_progress(
        &mut self,
        progress: std::sync::Arc<dyn Fn(crate::runtime::image_types::ImageProgress) + Send + Sync>,
    ) {
        self.progress = Some(progress);
    }
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
            owner_process: std::process::id(),
            live_destination: None,
            progress: None,
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
        // Leave room for schema-1 rollback snapshots: model <=16 KiB, account
        // <=256 B, up to four JSON bytes per archived byte, plus structure/phase
        // overhead. Filling retry history must never consume restore's capacity.
        let limit = if intent.restore.is_some() {
            128 * 1024
        } else {
            128 * 1024 - (4 * (16384 + 256) + 1024)
        };
        if bytes.len() > limit {
            return Err(RuntimeError::RetainedState);
        }
        let path = self.local.join(STORE);
        let pending = path.join(format!("intent-{}.next", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&pending)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        crate::runtime::journal::replace(&pending, &path.join("intent.json"))?;
        image_cache::fs::sync(&self.directory)?;
        Ok(())
    }
    fn slot(&self, intent: &Intent) -> Result<Option<Uuid>, RuntimeError> {
        SetupStore::upgrade_slot(intent.model.as_deref()).map_err(|_| RuntimeError::RetainedState)
    }
    fn slot_outcome(&self, slot: Uuid) -> Result<Option<bool>, RuntimeError> {
        match read_bytes(&self.local.join(STORE).join(format!("removed-{slot}")), 16)? {
            None => Ok(None),
            Some(bytes) if bytes == b"removing" => Ok(Some(false)),
            Some(bytes) if bytes == b"removed" => Ok(Some(true)),
            _ => Err(RuntimeError::RetainedState),
        }
    }
    fn slot_removed(&self, slot: Uuid) -> Result<bool, RuntimeError> {
        Ok(self.slot_outcome(slot)? == Some(true))
    }
    fn write_slot_outcome(&self, slot: Uuid, bytes: &[u8]) -> Result<(), RuntimeError> {
        let pending = format!("removed-{slot}-{}.next", Uuid::new_v4());
        let mut options = cap_std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = self.directory.open_with(&pending, &options)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        let root = self.local.join(STORE);
        crate::runtime::journal::replace(
            &root.join(pending),
            &root.join(format!("removed-{slot}")),
        )?;
        image_cache::fs::sync(&self.directory)?;
        Ok(())
    }
    fn reconcile_removed(&self) -> Result<(), RuntimeError> {
        let intents = self.intents()?;
        if intents.is_empty() {
            return Ok(());
        }
        let setup = SetupStore::new(Some(self.data.clone()));
        let _owner = setup
            .lock_upgrade_metadata()
            .map_err(|_| RuntimeError::RetainedState)?;
        for intent in intents {
            for model in [
                intent.model.as_deref(),
                intent.restore.as_ref().and_then(|r| r.model.as_deref()),
            ]
            .into_iter()
            .chain(intent.incomplete.iter().map(|b| b.model.as_deref()))
            {
                if let Some(slot) =
                    SetupStore::upgrade_slot(model).map_err(|_| RuntimeError::RetainedState)?
                {
                    if self.slot_removed(slot)? {
                        setup
                            .clear_upgrade_slot(slot)
                            .map_err(|_| RuntimeError::RetainedState)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn intents(&self) -> Result<Vec<Intent>, RuntimeError> {
        let mut intents: Vec<_> = self.read_intent()?.into_iter().collect();
        for entry in fs::read_dir(self.local.join(STORE))? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(RuntimeError::RetainedState);
            };
            if name.starts_with("completed-") && name.ends_with(".json") {
                let bytes =
                    read_bytes(&entry.path(), 128 * 1024)?.ok_or(RuntimeError::RetainedState)?;
                let intent: Intent =
                    serde_json::from_slice(&bytes).map_err(|_| RuntimeError::RetainedState)?;
                if intent.version != 1
                    || intent.transaction.is_nil()
                    || name != format!("completed-{}.json", intent.transaction)
                {
                    return Err(RuntimeError::RetainedState);
                }
                intents.push(intent);
            }
        }
        Ok(intents)
    }
    /// Called while holding transaction then model custody, including direct native calls.
    pub(crate) fn guard_model(
        &self,
        slot: Option<Uuid>,
        removal: bool,
    ) -> Result<(), RuntimeError> {
        for (index, intent) in self.intents()?.iter().enumerate() {
            let source = self.slot(intent)?;
            let outcome = source.map(|s| self.slot_outcome(s)).transpose()?.flatten();
            let removed = outcome == Some(true)
                || matches!(intent.credential, CredentialDisposition::Removed);
            let archived = self.archived_slots(intent)?;
            if outcome == Some(false) {
                return Err(RuntimeError::RetainedState);
            }
            for slot in &archived {
                if self.slot_outcome(*slot)? == Some(false) {
                    return Err(RuntimeError::RetainedState);
                }
            }
            if index == 0 && !matches!(intent.phase, Phase::ImagePrepared | Phase::Restored) {
                return Err(RuntimeError::RetainedState);
            }
            if index == 0
                && intent.phase != Phase::Restored
                && !removed
                && matches!(
                    intent.credential,
                    CredentialDisposition::Reserved | CredentialDisposition::Transferring
                )
            {
                return Err(RuntimeError::RetainedState);
            }
            if removal && slot.is_none() && source.is_some() && !removed {
                return Err(RuntimeError::RetainedState);
            }
            if let Some(slot) = slot {
                if (source == Some(slot) || archived.contains(&slot))
                    && (removal
                        || self.slot_outcome(slot)?.is_some()
                        || (source == Some(slot) && removed))
                {
                    return Err(RuntimeError::RetainedState);
                }
            }
        }
        Ok(())
    }
    /// Explicit association reuse only; caller retains confirmed lifecycle quiescence.
    pub(crate) fn transfer_credential(&mut self, transaction: Uuid) -> Result<bool, RuntimeError> {
        self.transfer_credential_with(transaction, |_| Ok(()))
    }
    fn transfer_credential_with(
        &mut self,
        transaction: Uuid,
        point: impl Fn(&str) -> Result<(), RuntimeError>,
    ) -> Result<bool, RuntimeError> {
        let mut intent = self.read_intent()?.ok_or(RuntimeError::RetainedState)?;
        if intent.transaction != transaction
            || intent.phase != Phase::ImagePrepared
            || intent.restore.is_some()
        {
            return Err(RuntimeError::RetainedState);
        }
        self.recover()?;
        let setup = SetupStore::new(Some(self.data.clone()));
        let _owner = setup
            .lock_upgrade_metadata()
            .map_err(|_| RuntimeError::RetainedState)?;
        let Some(slot) = self.slot(&intent)? else {
            return Ok(false);
        };
        if self.slot_outcome(slot)?.is_some()
            || matches!(intent.credential, CredentialDisposition::Removed)
        {
            return Err(RuntimeError::RetainedState);
        }
        let destination = intent
            .destination
            .as_ref()
            .ok_or(RuntimeError::RetainedState)?;
        let Some(model) =
            SetupStore::transferred_model(intent.model.as_deref(), destination.installation)
                .map_err(|_| RuntimeError::RetainedState)?
        else {
            return Ok(false);
        };
        let expected = [
            ("model-v1", Some(model)),
            ("model-account-v1", intent.account.clone()),
        ];
        self.check_transfer_metadata(&expected)?;
        if matches!(intent.credential, CredentialDisposition::Transferred) {
            return Ok(true);
        }
        intent.credential = CredentialDisposition::Transferring;
        self.persist(&intent)?;
        point("transfer-intent")?;
        let root = self.data.join("setup-v1");
        for (name, bytes) in expected {
            if let Some(bytes) = bytes {
                crate::setup::publish_upgrade_metadata(&root, name, &bytes)
                    .map_err(|_| RuntimeError::RetainedState)?;
            }
            point(name)?;
        }
        intent.credential = CredentialDisposition::Transferred;
        self.persist(&intent)?;
        point("transferred")?;
        Ok(true)
    }
    /// Exact host-slot removal after lifecycle shutdown, never archived guest cleanup.
    pub(crate) fn remove_credential(
        &mut self,
        transaction: Uuid,
        slot: Uuid,
        store: &impl crate::runtime::credential_store::CredentialStore,
    ) -> Result<(), RuntimeError> {
        self.remove_credential_with(transaction, slot, store, |_| Ok(()))
    }
    fn remove_credential_with(
        &mut self,
        transaction: Uuid,
        slot: Uuid,
        store: &impl crate::runtime::credential_store::CredentialStore,
        point: impl Fn(&str) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        self.recover()?;
        let intents = self.intents()?;
        let intent = intents
            .iter()
            .find(|i| i.transaction == transaction)
            .ok_or(RuntimeError::RetainedState)?;
        let archived = self.archived_slots(intent)?;
        if slot.is_nil() || (self.slot(intent)? != Some(slot) && !archived.contains(&slot)) {
            return Err(RuntimeError::RetainedState);
        }
        // A must already be stopped and retained, or coherently restored under the lifecycle owner.
        if !matches!(intent.phase, Phase::ImagePrepared | Phase::Restored) {
            return Err(RuntimeError::RetainedState);
        }
        let setup = SetupStore::new(Some(self.data.clone()));
        let _owner = setup
            .lock_upgrade_metadata()
            .map_err(|_| RuntimeError::RetainedState)?;
        if !self.slot_removed(slot)? {
            self.write_slot_outcome(slot, b"removing")?;
            point("removal-intent")?;
            store.delete(slot).map_err(|_| RuntimeError::Unavailable)?;
            point("vault-deleted")?;
            self.write_slot_outcome(slot, b"removed")?;
        }
        point("removal-recorded")?;
        setup
            .clear_upgrade_slot(slot)
            .map_err(|_| RuntimeError::RetainedState)?;
        point("metadata-removed")?;
        Ok(())
    }
    pub(crate) fn inspect(&self) -> Result<UpgradeStatus, RuntimeError> {
        self.recover()
    }
    fn credential_label(&self, intent: &Intent) -> Result<&'static str, RuntimeError> {
        if matches!(intent.credential, CredentialDisposition::Removed) {
            return Ok("removed");
        }
        if let Some(slot) = self.slot(intent)? {
            if self.slot_removed(slot)? {
                return Ok("removed");
            }
        }
        Ok(match intent.credential {
            CredentialDisposition::Absent => "absent",
            CredentialDisposition::Reserved | CredentialDisposition::Transferring => {
                return saved_credential_label(intent.model.as_deref(), intent.source.installation)
            }
            CredentialDisposition::Transferred => "transferred",
            CredentialDisposition::Removed => unreachable!(),
        })
    }
    fn check_transfer_metadata(
        &self,
        expected: &[(&str, Option<Vec<u8>>)],
    ) -> Result<(), RuntimeError> {
        for (name, bytes) in expected {
            let current = read_bytes(&self.data.join("setup-v1").join(name), 16384)?;
            if current.is_some() && current != *bytes {
                return Err(RuntimeError::RetainedState);
            }
        }
        Ok(())
    }
    pub(crate) fn validate_transfer(&self, transaction: Uuid) -> Result<(), RuntimeError> {
        let intent = self
            .read_intent()?
            .filter(|intent| intent.transaction == transaction)
            .ok_or(RuntimeError::RetainedState)?;
        if let Some(slot) = self.slot(&intent)? {
            // Both pending and completed removal deny transfer before quiescence.
            if self.slot_outcome(slot)?.is_some() {
                return Err(RuntimeError::RetainedState);
            }
        }
        if self.credential_label(&intent)? != "eligible" {
            return Err(RuntimeError::RetainedState);
        }
        let destination = intent
            .destination
            .as_ref()
            .ok_or(RuntimeError::RetainedState)?;
        let model =
            SetupStore::transferred_model(intent.model.as_deref(), destination.installation)
                .map_err(|_| RuntimeError::RetainedState)?;
        self.check_transfer_metadata(&[
            ("model-v1", model),
            ("model-account-v1", intent.account.clone()),
        ])
    }
    /// The renderer never supplies a slot; derive the exact one this
    /// transaction may remove. Prefers the live/current slot, falling back to
    /// a still-unresolved archived (retry/restore) reference.
    pub(crate) fn resolve_removal_slot(&self, transaction: Uuid) -> Result<Uuid, RuntimeError> {
        let intent = self
            .read_intent()?
            .filter(|intent| intent.transaction == transaction)
            .ok_or(RuntimeError::RetainedState)?;
        if let Some(slot) = self.slot(&intent)? {
            if !self.slot_removed(slot)? {
                return Ok(slot);
            }
        }
        for slot in self.archived_slots(&intent)? {
            if !self.slot_removed(slot)? {
                return Ok(slot);
            }
        }
        Err(RuntimeError::RetainedState)
    }
    /// Nonsecret projection for the renderer. Never fails on the happy path:
    /// an unreadable/inconsistent transaction becomes `conflict`, not an error
    /// the caller must separately translate.
    pub(crate) fn status_dto(&self, source: Option<&ImageSource>) -> UpgradeStatusDto {
        match self.status_dto_fallible(source) {
            Ok(dto) => dto,
            Err(error) => UpgradeStatusDto::conflict(classify_error(&error)),
        }
    }
    fn status_dto_fallible(
        &self,
        source: Option<&ImageSource>,
    ) -> Result<UpgradeStatusDto, RuntimeError> {
        self.reconcile_removed()?;
        let Some(intent) = self.read_intent()? else {
            return self.eligible_dto(source);
        };
        let status = self.recover()?;
        let state = match status {
            UpgradeStatus::None => "none",
            UpgradeStatus::SourceSelected => "sourceSelected",
            UpgradeStatus::SourceRetained => "sourceRetained",
            UpgradeStatus::Fresh => "fresh",
            UpgradeStatus::Incomplete => "incomplete",
            UpgradeStatus::Consumed => "consumed",
            UpgradeStatus::Restoring => "restoring",
            UpgradeStatus::Restored => "restored",
            UpgradeStatus::Conflict => "conflict",
        };
        Ok(UpgradeStatusDto {
            state,
            transaction: Some(intent.transaction),
            created_at: Some(intent.created_at),
            source_releases: intent.source_releases.clone(),
            target_release: Some(intent.target_release.clone()),
            retained_allocated_bytes: Some(intent.retained_allocated_bytes),
            additional_bytes: Some(intent.additional_bytes),
            credential: self.credential_label(&intent)?,
            busy: false,
            error: None,
            repeated: status == UpgradeStatus::Restored || !intent.incomplete.is_empty(),
            can_replace: matches!(
                status,
                UpgradeStatus::Restored
                    | UpgradeStatus::SourceSelected
                    | UpgradeStatus::SourceRetained
                    | UpgradeStatus::Incomplete
            ),
            can_restore: matches!(
                status,
                UpgradeStatus::SourceSelected
                    | UpgradeStatus::SourceRetained
                    | UpgradeStatus::Incomplete
                    | UpgradeStatus::Fresh
                    | UpgradeStatus::Consumed
                    | UpgradeStatus::Restoring
            ),
            can_cancel: false,
        })
    }
    /// No transaction yet. Offer replace only when the native-selected target
    /// recognizes the canonical installation as a trusted, qualified
    /// predecessor; current/newer/unknown/corrupt bindings stay `none` here,
    /// same as ordinary launch, never surfaced as an upgrade opportunity.
    fn eligible_dto(&self, source: Option<&ImageSource>) -> Result<UpgradeStatusDto, RuntimeError> {
        eligible_status(&self.data, &self.local, source)
    }
    /// Reconcile identities, not the last phase label. No recovery path synthesizes
    /// creation authority or moves an unexpected canonical directory out of the way.
    pub(crate) fn recover(&self) -> Result<UpgradeStatus, RuntimeError> {
        self.recover_with_live(self.live_destination.as_ref())
    }
    fn recover_with_live(&self, live: Option<&Journal>) -> Result<UpgradeStatus, RuntimeError> {
        self.reconcile_removed()?;
        let Some(intent) = self.read_intent()? else {
            return Ok(UpgradeStatus::None);
        };
        if intent.restore.is_some() {
            return self.restore_status(&intent);
        }
        let canonical = installation(&self.local.join("local-vm-v1"))?;
        self.validate_incomplete(&intent, canonical.as_ref())?;
        let retained = installation(&self.local.join(&intent.retained))?;
        if let Some(b) = canonical
            .as_ref()
            .filter(|b| Some(*b) == intent.destination.as_ref())
        {
            // B may have acquired a different saved association since setup.
            // Validate its current ownership rather than comparing it with A.
            let removed = self
                .slot(&intent)?
                .map(|slot| self.slot_removed(slot))
                .transpose()?
                .unwrap_or(false);
            if let Some(pending) = self.pending_retention(&intent) {
                for (name, snapshot) in [
                    ("model-v1", &pending.model),
                    ("model-account-v1", &pending.account),
                ] {
                    let current = read_bytes(&self.data.join("setup-v1").join(name), 16384)?;
                    if current.is_some() && current != *snapshot {
                        return Err(RuntimeError::RetainedState);
                    }
                }
            } else if matches!(intent.credential, CredentialDisposition::Transferring) && !removed {
                let expected =
                    SetupStore::transferred_model(intent.model.as_deref(), b.installation)
                        .map_err(|_| RuntimeError::RetainedState)?;
                for (name, bytes) in [
                    ("model-v1", &expected),
                    ("model-account-v1", &intent.account),
                ] {
                    let current = read_bytes(&self.data.join("setup-v1").join(name), 16384)?;
                    if current.is_some() && current != *bytes {
                        return Err(RuntimeError::RetainedState);
                    }
                }
            } else {
                let _metadata = SetupStore::new(Some(self.data.clone()))
                    .snapshot_for_upgrade(b.installation)
                    .map_err(|_| RuntimeError::RetainedState)?;
            }
        } else {
            for (name, snapshot) in [
                ("model-v1", &intent.model),
                ("model-account-v1", &intent.account),
            ] {
                let current = read_bytes(&self.data.join("setup-v1").join(name), 16384)?;
                if current.is_some() && current != *snapshot {
                    return Err(RuntimeError::RetainedState);
                }
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
                let root = self.local.join("local-vm-v1");
                if let Some(journal) = live {
                    if journal.record().installation_id != b.installation
                        || journal.record().state_identity != b.state
                    {
                        return Err(RuntimeError::RetainedState);
                    }
                    journal.verify_state()?;
                    if binding_hash(&root)?
                        != *intent
                            .destination_binding_sha256
                            .as_ref()
                            .ok_or(RuntimeError::RetainedState)?
                    {
                        return Err(RuntimeError::RetainedState);
                    }
                    let image = crate::runtime::image_install::inspect_installed_image(
                        &journal.image_target(&intent.target_binding.state_compatibility_id)?,
                    )?;
                    if image.identity() != &intent.target_binding {
                        return Err(RuntimeError::RetainedState);
                    }
                    if journal.record().phase == StatePhase::Consumed {
                        return Ok(UpgradeStatus::Consumed);
                    }
                    journal.require_fresh_creation()?;
                    return Ok(UpgradeStatus::Fresh);
                }
                let phase = read_record(&root)?.phase;
                if let Some(hash) = &intent.destination_binding_sha256 {
                    verify_destination_binding(&root, &intent.target_binding, hash)?;
                } else if phase == StatePhase::Consumed {
                    return Err(RuntimeError::RetainedState);
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
    pub(crate) fn replace(
        &mut self,
        source: &ImageSource,
        machine_id: Vec<u8>,
        closed: &watch::Receiver<bool>,
        runtime: &tokio::runtime::Handle,
    ) -> Result<UpgradeStatus, RuntimeError> {
        let cache_path = self.local.join("image-cache-v1");
        let progress = self.progress.clone();
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
        let previous = self.read_intent()?;
        if let Some(intent) = &previous {
            if intent.phase != Phase::Restored
                || self.restore_status(intent)? != UpgradeStatus::Restored
            {
                return Err(RuntimeError::RetainedState);
            }
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
            retained_allocated_bytes: allocated(&root, Some(&journal), true)?,
            additional_bytes,
            restore: None,
            incomplete: vec![],
        };
        let backend = journal.record().backend;
        if let Some(previous) = previous {
            // Keep the admission marker until the new intent atomically replaces
            // it. Completed history is a durable, idempotent copy, not a move.
            let name = format!("completed-{}.json", previous.transaction);
            let bytes = serde_json::to_vec(&previous).map_err(|_| RuntimeError::RetainedState)?;
            if let Some(archived) = read_bytes(&self.local.join(STORE).join(&name), 128 * 1024)? {
                if archived != bytes {
                    return Err(RuntimeError::RetainedState);
                }
            } else {
                let pending = format!("completed-{}-{}.next", previous.transaction, Uuid::new_v4());
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(self.local.join(STORE).join(&pending))?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                drop(file);
                point("completed-archive-staged")?;
                image_cache::fs::publish_named(&self.directory, &pending, &name)?;
            }
            image_cache::fs::sync(&self.directory)?;
            point("completed-archived")?;
        }
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
    pub(crate) fn restore(&mut self, transaction: Uuid) -> Result<UpgradeStatus, RuntimeError> {
        self.restore_with(transaction, |_| Ok(()))
    }
    fn source_metadata<'a>(&self, intent: &'a Intent) -> Result<MetadataFiles<'a>, RuntimeError> {
        let removed = self
            .slot(intent)?
            .map(|s| self.slot_outcome(s))
            .transpose()?
            .flatten()
            .is_some();
        let (model, account) =
            if removed || matches!(intent.credential, CredentialDisposition::Removed) {
                (&None, &None)
            } else {
                (&intent.model, &intent.account)
            };
        Ok([("model-v1", model), ("model-account-v1", account)])
    }
    fn restore_status(&self, intent: &Intent) -> Result<UpgradeStatus, RuntimeError> {
        let restore = intent.restore.as_ref().ok_or(RuntimeError::RetainedState)?;
        let destination = intent.destination.as_ref();
        let root = self.local.join("local-vm-v1");
        let retained_a = self.local.join(&intent.retained);
        let retained_b = self
            .local
            .join(format!("retained-new-{}", intent.transaction));
        let canonical = installation(&root)?;
        self.validate_incomplete(intent, canonical.as_ref())?;
        let a = installation(&retained_a)?;
        let b = installation(&retained_b)?;
        let source_selected = canonical.as_ref() == Some(&intent.source);
        let (source, target) = match (canonical.as_ref(), a.as_ref(), b.as_ref()) {
            (Some(current), Some(a), None)
                if Some(current) == destination && a == &intent.source =>
            {
                (&retained_a, Some(&root))
            }
            (None, Some(a), Some(b)) if a == &intent.source && Some(b) == destination => {
                (&retained_a, Some(&retained_b))
            }
            (Some(a), None, Some(b)) if a == &intent.source && Some(b) == destination => {
                (&root, Some(&retained_b))
            }
            (None, Some(a), None) if a == &intent.source && destination.is_none() => {
                (&retained_a, None)
            }
            (Some(a), None, None) if a == &intent.source && destination.is_none() => (&root, None),
            _ => return Err(RuntimeError::RetainedState),
        };
        verify_binding(
            source,
            &intent.source_binding,
            &intent.source_binding_sha256,
        )?;
        if let (Some(target), Some(hash)) = (target, &intent.destination_binding_sha256) {
            verify_destination_binding(target, &intent.target_binding, hash)?;
        }
        if intent.phase == Phase::Restored {
            if !source_selected {
                return Err(RuntimeError::RetainedState);
            }
            // Completion ends rollback authority over A's metadata. Its matching
            // app may subsequently change the association; validate current
            // ownership for fresh C, never replay the archived source bytes.
            let _metadata = SetupStore::new(Some(self.data.clone()))
                .snapshot_for_upgrade(intent.source.installation)
                .map_err(|_| RuntimeError::RetainedState)?;
            return Ok(UpgradeStatus::Restored);
        }
        for ((name, wanted), archived) in self
            .source_metadata(intent)?
            .into_iter()
            .zip([&restore.model, &restore.account])
        {
            let current = read_bytes(&self.data.join("setup-v1").join(name), 16384)?;
            let expected = if source_selected {
                wanted
            } else if canonical.is_some() {
                archived
            } else {
                &None
            };
            if current.is_some() && current != *expected {
                return Err(RuntimeError::RetainedState);
            }
        }
        Ok(UpgradeStatus::Restoring)
    }
    fn restore_with(
        &mut self,
        transaction: Uuid,
        point: impl Fn(&str) -> Result<(), RuntimeError>,
    ) -> Result<UpgradeStatus, RuntimeError> {
        self.reconcile_removed()?;
        let mut intent = self.read_intent()?.ok_or(RuntimeError::RetainedState)?;
        if intent.transaction != transaction || intent.phase == Phase::Restored {
            return Err(RuntimeError::RetainedState);
        }
        let root = self.local.join("local-vm-v1");
        let retained_a = self.local.join(&intent.retained);
        let retained_b_name = format!("retained-new-{transaction}");
        let retained_b = self.local.join(&retained_b_name);
        // An unhanded destination has never reached a VM worker. Discard its
        // creation capability; any handed-out/live custody must refuse below.
        self.live_destination = None;
        if self.pending_retention(&intent).is_some() {
            self.recover()?;
            let setup = SetupStore::new(Some(self.data.clone()));
            let _metadata = setup
                .lock_upgrade_metadata()
                .map_err(|_| RuntimeError::RetainedState)?;
            self.complete_retention(&mut intent, &point)?;
        }
        // A live/uncertain VM retains this custody and refuses storage switching.
        // Incomplete B is lockable for retention, never for boot or formatting.
        let canonical = installation(&root)?;
        let canonical_owner = canonical
            .as_ref()
            .map(|_| Journal::recovery_owner(&root))
            .transpose()?;
        let source_owner = installation(&retained_a)?
            .as_ref()
            .map(|_| Journal::recovery_owner(&retained_a))
            .transpose()?;
        let target_owner = installation(&retained_b)?
            .as_ref()
            .map(|_| Journal::recovery_owner(&retained_b))
            .transpose()?;
        let setup = SetupStore::new(Some(self.data.clone()));
        let (_snapshot, _recovery_owner);
        if intent.restore.is_none() {
            if retained_b.try_exists()? {
                return Err(RuntimeError::RetainedState);
            }
            // Inspection opens its own journal; drop custody only after proving
            // no active worker owns either installation, before read-only checks.
            drop((canonical_owner, source_owner, target_owner));
            if let Some(destination) = &intent.destination {
                if canonical.as_ref() != Some(destination) {
                    return Err(RuntimeError::RetainedState);
                }
                let snapshot = setup
                    .snapshot_for_upgrade(destination.installation)
                    .map_err(|_| RuntimeError::RetainedState)?;
                intent.restore = Some(Restore {
                    model: snapshot.model.clone(),
                    account: snapshot.account.clone(),
                });
                _snapshot = Some(snapshot);
                _recovery_owner = None;
            } else {
                _snapshot = None;
                _recovery_owner = Some(
                    setup
                        .lock_upgrade_metadata()
                        .map_err(|_| RuntimeError::RetainedState)?,
                );
                intent.restore = Some(Restore {
                    model: None,
                    account: None,
                });
            }
            intent.phase = Phase::Restoring;
            self.restore_status(&intent)?;
            self.persist(&intent)?;
            point("restore-intent")?;
        } else {
            drop((canonical_owner, source_owner, target_owner));
            _snapshot = None;
            _recovery_owner = Some(
                setup
                    .lock_upgrade_metadata()
                    .map_err(|_| RuntimeError::RetainedState)?,
            );
        }
        self.restore_status(&intent)?;
        let parent = Dir::open_ambient_dir(&self.local, cap_std::ambient_authority())?;
        let setup_dir =
            Dir::open_ambient_dir(self.data.join("setup-v1"), cap_std::ambient_authority())?;
        if intent.destination.is_some()
            && installation(&root)?.as_ref() == intent.destination.as_ref()
        {
            for (name, boundary) in [
                ("model-v1", "restore-model-archived"),
                ("model-account-v1", "restore-account-archived"),
            ] {
                let path = self.data.join("setup-v1").join(name);
                if read_bytes(&path, 16384)?.is_some() {
                    fs::remove_file(path)?;
                    image_cache::fs::sync(&setup_dir)?;
                }
                point(boundary)?;
            }
            image_cache::fs::publish_named(&parent, "local-vm-v1", &retained_b_name)?;
            image_cache::fs::sync(&parent)?;
            point("destination-retained")?;
        }
        self.restore_status(&intent)?;
        if !root.try_exists()? {
            image_cache::fs::publish_named(&parent, &intent.retained, "local-vm-v1")?;
            image_cache::fs::sync(&parent)?;
            point("source-restored")?;
        }
        self.restore_status(&intent)?;
        for ((name, snapshot), boundary) in self
            .source_metadata(&intent)?
            .into_iter()
            .zip(["restore-model-published", "restore-account-published"])
        {
            let path = self.data.join("setup-v1").join(name);
            if let Some(bytes) = snapshot {
                if read_bytes(&path, 16384)?.as_ref() != Some(bytes) {
                    let pending = self.data.join("setup-v1").join(format!(
                        "restore-{transaction}-{name}-{}.next",
                        Uuid::new_v4()
                    ));
                    let mut file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(&pending)?;
                    file.write_all(bytes)?;
                    file.sync_all()?;
                    drop(file);
                    crate::runtime::journal::replace(&pending, &path)?;
                }
            }
            image_cache::fs::sync(&setup_dir)?;
            point(boundary)?;
        }
        intent.phase = Phase::Restored;
        self.persist(&intent)?;
        point("restored")?;
        Ok(UpgradeStatus::Restored)
    }
    /// Only the creating process may hand B to its native worker. Recovery cannot.
    pub(crate) fn take_destination(&mut self) -> Option<Journal> {
        self.live_destination.take()
    }
}
/// Only a reconciled `None` (never replaced) or `Consumed` (B already booted
/// under this app) destination may proceed through ordinary launch. Every
/// other status needs an explicit upgrade action (retry/restore) first, and a
/// restored A is never admitted here even though its directory looks normal.
fn ordinary_launch_permitted(status: &UpgradeStatus) -> bool {
    matches!(status, UpgradeStatus::None | UpgradeStatus::Consumed)
}
/// Denial-only admission before touching VM custody; mutation paths still
/// acquire and validate the transaction again after confirmed shutdown.
pub(crate) fn validate_transaction(
    data: &Path,
    local: &Path,
    transaction: Uuid,
) -> Result<UpgradeStatus, RuntimeError> {
    if transaction.is_nil() || !local.join(STORE).try_exists()? {
        return Err(RuntimeError::RetainedState);
    }
    let owner = WorkspaceUpgrade::open(data, local)?;
    let intent = owner
        .read_intent()?
        .filter(|intent| intent.transaction == transaction)
        .ok_or(RuntimeError::RetainedState)?;
    if intent.phase == Phase::Restored {
        return Ok(UpgradeStatus::Restored);
    }
    if intent.phase == Phase::Restoring {
        return Ok(UpgradeStatus::Restoring);
    }
    let canonical = installation(&local.join("local-vm-v1"))?;
    if let Some(selected) = canonical {
        if Some(&selected) == intent.destination.as_ref() {
            return Ok(
                if read_record(&local.join("local-vm-v1"))?.phase == StatePhase::Consumed {
                    UpgradeStatus::Consumed
                } else {
                    UpgradeStatus::Incomplete
                },
            );
        }
        if selected == intent.source {
            return Ok(UpgradeStatus::SourceSelected);
        }
        return Err(RuntimeError::RetainedState);
    }
    Ok(UpgradeStatus::SourceRetained)
}
pub(crate) fn validate_new_replacement(data: &Path, local: &Path) -> Result<(), RuntimeError> {
    if !local.join(STORE).try_exists()? {
        return Ok(());
    }
    let owner = WorkspaceUpgrade::open(data, local)?;
    if owner
        .read_intent()?
        .is_none_or(|intent| intent.phase == Phase::Restored)
    {
        Ok(())
    } else {
        Err(RuntimeError::RetainedState)
    }
}
/// Read-only status for the renderer. Never creates the upgrade store as a
/// side effect of polling: the overwhelmingly common "never replaced" case
/// takes the same fast, non-mutating path as ordinary admission.
pub(crate) fn upgrade_status(
    data: &Path,
    local: &Path,
    source: Option<&ImageSource>,
) -> UpgradeStatusDto {
    match local.join(STORE).try_exists() {
        Ok(false) => match eligible_status(data, local, source) {
            Ok(dto) => dto,
            Err(RuntimeError::Busy) => UpgradeStatusDto {
                busy: true,
                ..UpgradeStatusDto::none()
            },
            Err(error) => UpgradeStatusDto::conflict(classify_error(&error)),
        },
        Ok(true) => match WorkspaceUpgrade::open(data, local) {
            Ok(owner) => owner.status_dto(source),
            Err(RuntimeError::Busy) => UpgradeStatusDto {
                busy: true,
                ..UpgradeStatusDto::none()
            },
            Err(error) => UpgradeStatusDto::conflict(classify_error(&error)),
        },
        Err(error) => UpgradeStatusDto::conflict(classify_error(&RuntimeError::Io(error))),
    }
}
/// Status-aware gate, called on every ordinary VM launch attempt, not only at
/// process startup. `data` is D (app-data); `root` is L's canonical installation.
pub(crate) fn admit_prepared(
    data: &Path,
    root: &Path,
    journal: &Journal,
) -> Result<(), RuntimeError> {
    let selected = installation(root)?.ok_or(RuntimeError::RetainedState)?;
    if selected.installation != journal.record().installation_id
        || selected.state != journal.record().state_identity
    {
        return Err(RuntimeError::RetainedState);
    }
    journal.verify_state()?;
    let local = root.parent().ok_or(RuntimeError::RetainedState)?;
    if !local.join(STORE).try_exists()? {
        return Ok(());
    }
    let owner = WorkspaceUpgrade::open(data, local)?;
    match owner.recover_with_live(Some(journal))? {
        UpgradeStatus::None | UpgradeStatus::Fresh | UpgradeStatus::Consumed => Ok(()),
        _ => Err(RuntimeError::RetainedState),
    }
}
pub(crate) fn admit(data: &Path, root: &Path) -> Result<(), RuntimeError> {
    let local = root.parent().ok_or(RuntimeError::RetainedState)?;
    if !local.join(STORE).try_exists()? {
        return Ok(());
    }
    let owner = WorkspaceUpgrade::open(data, local)?;
    if ordinary_launch_permitted(&owner.recover()?) {
        Ok(())
    } else {
        Err(RuntimeError::RetainedState)
    }
}
/// Runs before ordinary VM configuration and model mismatch processing.
/// Returns whether any upgrade transaction was ever observed this reconcile,
/// so callers can permanently suppress automatic first-test replay even when
/// a later separate status poll fails.
pub fn recover_startup(data: &Path, local: &Path) -> Result<bool, RuntimeError> {
    if !local.join(STORE).try_exists()? {
        return Ok(false);
    }
    let owner = WorkspaceUpgrade::open(data, local)?;
    let status = owner.recover()?;
    if !ordinary_launch_permitted(&status) {
        return Err(RuntimeError::RetainedState);
    }
    Ok(status != UpgradeStatus::None)
}
#[cfg(test)]
#[path = "workspace_upgrade_tests.rs"]
mod tests;
