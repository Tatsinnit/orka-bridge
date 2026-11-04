use super::{identity::*, types::*};
use bridge_runtime_protocol::{StateCreation, Uuid, STATE_BYTES};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

// Worst-case JSON for 4096 byte values plus metadata exceeds 16 KiB.
const MAX_RECORD: u64 = 32 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatePhase {
    Empty,
    Preparing,
    Prepared,
    Consumed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallationRecord {
    pub version: u16,
    pub installation_id: Uuid,
    pub generation: u64,
    pub revision: u64,
    pub backend: BackendKind,
    pub machine_id: Vec<u8>,
    pub mac: [u8; 6],
    pub state_identity: FileIdentity,
    pub phase: StatePhase,
    pub boot_nonce: Option<Uuid>,
}
pub struct Journal {
    root: PathBuf,
    _lock: Arc<File>,
    state: File,
    record: InstallationRecord,
    creation_live: bool,
}
fn lock(root: &Path) -> Result<File, RuntimeError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("owner.lock"))?;
    file.try_lock().map_err(|e| match e {
        fs::TryLockError::WouldBlock => RuntimeError::Busy,
        fs::TryLockError::Error(e) => e.into(),
    })?;
    Ok(file)
}
fn regular(path: &Path) -> Result<(), RuntimeError> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(RuntimeError::RetainedState);
    }
    Ok(())
}
impl Journal {
    pub fn create(
        root: &Path,
        backend: BackendKind,
        machine_id: Vec<u8>,
    ) -> Result<Self, RuntimeError> {
        if machine_id.is_empty() || machine_id.len() > 4096 {
            return Err(RuntimeError::RetainedState);
        }
        let root = std::path::absolute(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&root)?;
        }
        #[cfg(windows)]
        fs::create_dir(&root)?;
        #[cfg(unix)]
        File::open(root.parent().ok_or(RuntimeError::RetainedState)?)?.sync_all()?;
        let root = fs::canonicalize(&root)?;
        let owner = lock(&root)?;
        let state = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(root.join("state.disk"))?;
        state.sync_all()?;
        let installation_id = Uuid::new_v4();
        let mut mac = [0; 6];
        mac.copy_from_slice(&installation_id.as_bytes()[..6]);
        mac[0] = (mac[0] & 0xfc) | 2;
        let record = InstallationRecord {
            version: 1,
            installation_id,
            generation: 1,
            revision: 0,
            backend,
            machine_id,
            mac,
            state_identity: file_identity(&state)?,
            phase: StatePhase::Empty,
            boot_nonce: None,
        };
        let journal = Self {
            root,
            _lock: Arc::new(owner),
            state,
            record,
            creation_live: true,
        };
        journal.persist(&journal.record)?;
        Ok(journal)
    }
    pub fn open(root: &Path) -> Result<Self, RuntimeError> {
        let metadata = fs::symlink_metadata(root)?;
        if !metadata.file_type().is_dir() {
            return Err(RuntimeError::RetainedState);
        }
        let root = fs::canonicalize(root)?;
        let owner = lock(&root)?;
        // An interrupted record write needs explicit recovery, not guessed replay.
        if root.join("record.next").exists() {
            return Err(RuntimeError::RetainedState);
        }
        regular(&root.join("record.json"))?;
        let mut bytes = Vec::new();
        File::open(root.join("record.json"))?
            .take(MAX_RECORD + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(RuntimeError::RetainedState);
        }
        let record: InstallationRecord =
            serde_json::from_slice(&bytes).map_err(|_| RuntimeError::RetainedState)?;
        if record.version != 1
            || record.generation == 0
            || record.installation_id.is_nil()
            || record.machine_id.is_empty()
            || record.machine_id.len() > 4096
            || record.mac[0] & 3 != 2
            || record.phase == StatePhase::Preparing
            || (record.phase == StatePhase::Consumed && record.boot_nonce.is_none())
        {
            return Err(RuntimeError::RetainedState);
        }
        regular(&root.join("state.disk"))?;
        let state = OpenOptions::new()
            .read(true)
            .write(true)
            .open(root.join("state.disk"))?;
        let journal = Self {
            root,
            _lock: Arc::new(owner),
            state,
            record,
            creation_live: false,
        };
        journal.verify_state()?;
        if journal.record.phase == StatePhase::Empty && journal.state.metadata()?.len() != 0 {
            return Err(RuntimeError::RetainedState);
        }
        Ok(journal)
    }
    pub fn record(&self) -> &InstallationRecord {
        &self.record
    }
    pub fn verify_state(&self) -> Result<(), RuntimeError> {
        if self.record.phase == StatePhase::Empty && self.state.metadata()?.len() != 0 {
            return Err(RuntimeError::RetainedState);
        }
        regular(&self.root.join("state.disk"))?;
        let current = File::open(self.root.join("state.disk"))?;
        if file_identity(&current)? != self.record.state_identity
            || file_identity(&self.state)? != self.record.state_identity
        {
            return Err(RuntimeError::RetainedState);
        }
        Ok(())
    }
    pub(super) fn spec(&self, nonce: Uuid) -> LaunchSpec {
        LaunchSpec {
            installation_id: self.record.installation_id,
            generation: self.record.generation,
            boot_nonce: nonce,
            ownership: self._lock.clone(),
            root: self.root.clone(),
            state_path: self.root.join("state.disk"),
            state_bytes: STATE_BYTES,
            backend: self.record.backend,
            machine_id: self.record.machine_id.clone(),
            mac: self.record.mac,
        }
    }
    pub(super) fn require_fresh_creation(&self) -> Result<(), RuntimeError> {
        if !self.creation_live {
            return Err(RuntimeError::RetainedState);
        }
        Ok(())
    }
    pub(super) fn finish_preparation(&mut self) -> Result<(), RuntimeError> {
        self.finish_preparation_with(File::sync_all)
    }
    fn finish_preparation_with(
        &mut self,
        sync: impl FnOnce(&File) -> std::io::Result<()>,
    ) -> Result<(), RuntimeError> {
        self.verify_state()?;
        sync(&self.state)?;
        self.set_phase(StatePhase::Prepared)
    }
    pub(super) fn set_phase(&mut self, phase: StatePhase) -> Result<(), RuntimeError> {
        let mut next = self.record.clone();
        next.phase = phase;
        self.commit(next)
    }
    pub(super) fn begin_boot(&mut self) -> Result<LaunchSpec, RuntimeError> {
        self.verify_state()?;
        if !matches!(
            self.record.phase,
            StatePhase::Prepared | StatePhase::Consumed
        ) {
            return Err(RuntimeError::RetainedState);
        }
        if self.record.phase == StatePhase::Prepared {
            self.require_fresh_creation()?;
        }
        let mut next = self.record.clone();
        next.boot_nonce = Some(Uuid::new_v4());
        self.commit(next)?;
        Ok(self.spec(self.record.boot_nonce.unwrap()))
    }
    pub(super) fn consume_creation(
        &mut self,
        spec: &LaunchSpec,
    ) -> Result<StateCreation, RuntimeError> {
        self.verify_state()?;
        if self.record.boot_nonce != Some(spec.boot_nonce)
            || self.record.installation_id != spec.installation_id
            || self.record.generation != spec.generation
        {
            return Err(RuntimeError::RetainedState);
        }
        match self.record.phase {
            StatePhase::Prepared => {
                // Persisted inode/file IDs can be reused after all handles close.
                // Only this live, exclusively-created file can confer New authority.
                self.require_fresh_creation()?;
                self.set_phase(StatePhase::Consumed)?;
                self.creation_live = false;
                Ok(StateCreation::New)
            }
            StatePhase::Consumed => Ok(StateCreation::Existing),
            _ => Err(RuntimeError::RetainedState),
        }
    }
    fn commit(&mut self, mut next: InstallationRecord) -> Result<(), RuntimeError> {
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or(RuntimeError::RetainedState)?;
        self.persist(&next)?;
        self.record = next;
        Ok(())
    }
    fn persist(&self, record: &InstallationRecord) -> Result<(), RuntimeError> {
        let bytes = serde_json::to_vec(record).map_err(|_| RuntimeError::RetainedState)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(RuntimeError::RetainedState);
        }
        let mut pending = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.root.join("record.next"))?;
        pending.write_all(&bytes)?;
        pending.sync_all()?;
        drop(pending);
        replace(
            &self.root.join("record.next"),
            &self.root.join("record.json"),
        )?;
        Ok(())
    }
}
#[cfg(unix)]
fn replace(from: &Path, to: &Path) -> Result<(), RuntimeError> {
    fs::rename(from, to)?;
    File::open(to.parent().unwrap())?.sync_all()?;
    Ok(())
}
#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> Result<(), RuntimeError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (from, to) = (wide(from), wide(to));
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_state_flush_cannot_commit_prepared() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("install");
        let mut journal = Journal::create(&root, BackendKind::Apple, vec![1]).unwrap();
        journal.set_phase(StatePhase::Preparing).unwrap();
        let before = fs::read(root.join("record.json")).unwrap();
        let result = journal
            .finish_preparation_with(|_| Err(std::io::Error::other("simulated failed flush")));
        assert!(matches!(result,Err(RuntimeError::Io(e)) if e.kind()==std::io::ErrorKind::Other));
        assert_eq!(journal.record().phase, StatePhase::Preparing);
        assert_eq!(fs::read(root.join("record.json")).unwrap(), before);
        drop(journal);
        assert!(matches!(
            Journal::open(&root),
            Err(RuntimeError::RetainedState)
        ));
    }
}
