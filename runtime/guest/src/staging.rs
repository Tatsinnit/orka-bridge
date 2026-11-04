use bridge_runtime_protocol::*;
use std::{
    fs,
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
pub(crate) fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}
pub(crate) fn directory(path: &Path) -> io::Result<()> {
    if !path.exists() {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(invalid());
    }
    Ok(())
}
pub(crate) fn create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}
fn discard(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(invalid());
    }
    // Remove the committed name durably before unlinking any of its bytes.
    let garbage =
        if Uuid::parse_str(&path.file_name().ok_or_else(invalid)?.to_string_lossy()).is_err() {
            let parent = path.parent().ok_or_else(invalid)?;
            let next = parent.join(Uuid::new_v4().to_string());
            fs::rename(path, &next)?;
            fs::File::open(parent)?.sync_all()?;
            next
        } else {
            path.to_path_buf()
        };
    let path = garbage.as_path();
    for name in ["agent", "tests"] {
        match fs::remove_file(path.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    fs::remove_dir(path)
}
struct Upload {
    id: Uuid,
    snapshot: SnapshotId,
    agent: Vec<u8>,
    tests: Vec<u8>,
}
pub struct Staging {
    root: PathBuf,
    pending: Option<Upload>,
}
impl Staging {
    pub fn new(root: PathBuf) -> io::Result<Self> {
        directory(&root)?;
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            if Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok() {
                discard(&entry.path())?;
            }
        }
        Ok(Self {
            root,
            pending: None,
        })
    }
    pub fn begin(&mut self, id: Uuid, snapshot: SnapshotId) -> io::Result<()> {
        if id.is_nil() || !snapshot.valid() {
            return Err(invalid());
        }
        self.pending = Some(Upload {
            id,
            snapshot,
            agent: Vec::new(),
            tests: Vec::new(),
        });
        Ok(())
    }
    pub fn chunk(
        &mut self,
        id: Uuid,
        slot: FileSlot,
        offset: usize,
        data: &[u8],
    ) -> io::Result<()> {
        let u = self
            .pending
            .as_mut()
            .filter(|u| u.id == id)
            .ok_or_else(invalid)?;
        let bytes = match slot {
            FileSlot::Agent => &mut u.agent,
            FileSlot::Tests => &mut u.tests,
        };
        if offset != bytes.len()
            || data.len() > MAX_CHUNK
            || offset.saturating_add(data.len()) > MAX_FILE
        {
            return Err(invalid());
        }
        bytes.extend_from_slice(data);
        Ok(())
    }
    pub fn contains(&self, id: &SnapshotId) -> io::Result<bool> {
        if !id.valid() {
            return Err(invalid());
        }
        let root = self.root.join(id.key());
        if !root.exists() {
            return Ok(false);
        }
        if !fs::symlink_metadata(&root)?.is_dir() {
            return Err(invalid());
        }
        let a = crate::linux::read_regular(&root.join("agent"), MAX_FILE as u64)?;
        let t = crate::linux::read_regular(&root.join("tests"), MAX_FILE as u64)?;
        Ok(SnapshotId::of(&a, &t) == *id)
    }
    pub fn commit(&mut self, id: Uuid) -> io::Result<SnapshotId> {
        let u = self
            .pending
            .as_ref()
            .filter(|u| u.id == id)
            .ok_or_else(invalid)?;
        if SnapshotId::of(&u.agent, &u.tests) != u.snapshot {
            return Err(invalid());
        }
        let snapshot = u.snapshot.clone();
        if !self.contains(&snapshot)? {
            let mut cached = fs::read_dir(&self.root)?.collect::<io::Result<Vec<_>>>()?;
            cached.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            while cached.len() >= MAX_OPERATIONS {
                let old = cached.remove(0);
                discard(&old.path())?;
            }
            let temp = self.root.join(id.to_string());
            fs::DirBuilder::new().mode(0o700).create(&temp)?;
            create(&temp.join("agent"), &u.agent)?;
            create(&temp.join("tests"), &u.tests)?;
            fs::set_permissions(temp.join("agent"), fs::Permissions::from_mode(0o400))?;
            fs::set_permissions(temp.join("tests"), fs::Permissions::from_mode(0o400))?;
            fs::File::open(&temp)?.sync_all()?;
            fs::rename(&temp, self.root.join(snapshot.key()))?;
            fs::File::open(&self.root)?.sync_all()?;
        }
        self.pending = None;
        Ok(snapshot)
    }
}
