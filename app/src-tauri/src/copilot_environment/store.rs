use super::*;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 16 * 1024;
pub(super) struct Store {
    root: PathBuf,
    _lock: File,
}
impl Store {
    pub fn new(app_data: &Path) -> Result<Self, Error> {
        let root = app_data.join("copilot-environments-v1");
        fs::create_dir_all(&root).map_err(|_| Error::Storage)?;
        if !fs::symlink_metadata(&root)
            .map_err(|_| Error::Storage)?
            .is_dir()
        {
            return Err(Error::Storage);
        }
        let path = root.join("owner.lock");
        if fs::symlink_metadata(&path).is_ok_and(|m| !m.file_type().is_file()) {
            return Err(Error::Storage);
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| Error::Storage)?;
        lock.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => Error::Busy,
            _ => Error::Storage,
        })?;
        Ok(Self { root, _lock: lock })
    }
    pub fn save(&self, r: &Receipt) -> Result<(), Error> {
        r.validate()?;
        let bytes = serde_json::to_vec(r).map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > LIMIT {
            return Err(Error::Storage);
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(|_| Error::Storage)?;
        file.write_all(&bytes)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| Error::Storage)?;
        file.persist(self.path(r.environment_id))
            .map_err(|_| Error::Storage)?;
        self.sync()
    }
    pub fn load(&self, id: Uuid) -> Result<Receipt, Error> {
        let path = self.path(id);
        let meta = fs::symlink_metadata(&path).map_err(|_| Error::Storage)?;
        if !meta.file_type().is_file() || meta.len() > LIMIT {
            return Err(Error::Storage);
        }
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| Error::Storage)?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > LIMIT {
            return Err(Error::Storage);
        }
        let r: Receipt = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
        r.validate().map_err(|_| Error::Storage)?;
        if r.environment_id != id {
            return Err(Error::Storage);
        }
        Ok(r)
    }
    pub fn all(&self) -> Result<Vec<Receipt>, Error> {
        let mut receipts = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(|_| Error::Storage)? {
            let name = entry.map_err(|_| Error::Storage)?.file_name();
            let Some(name) = name.to_str().and_then(|s| s.strip_suffix(".json")) else {
                continue;
            };
            let id = Uuid::parse_str(name).map_err(|_| Error::Storage)?;
            receipts.push(self.load(id)?);
        }
        Ok(receipts)
    }
    pub fn remove(&self, id: Uuid) -> Result<(), Error> {
        // Read ownership before deleting this exact UUID-named receipt, not a caller path.
        self.load(id)?;
        fs::remove_file(self.path(id)).map_err(|_| Error::Storage)?;
        self.sync()
    }
    fn path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{}.json", id.simple()))
    }
    fn sync(&self) -> Result<(), Error> {
        #[cfg(unix)]
        File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        // A concurrent fork retains this description until exec; closing only our descriptor
        // can leave the next operation spuriously Busy. No backend retains Store ownership.
        let _ = self._lock.unlock();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn drop_releases_ownership_even_if_a_concurrent_spawn_inherits_the_description() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Store::new(dir.path()).unwrap();
        // A fork before exec retains the same open-file description despite CLOEXEC.
        let inherited = owner._lock.try_clone().unwrap();
        drop(owner);
        let reopened = Store::new(dir.path()).expect("parent ownership must be released now");
        drop(reopened);
        drop(inherited);
    }
}
