use bridge_runtime_protocol::*;
use std::{
    io::{self, Read},
    path::{Component, Path, PathBuf},
};
pub struct AgentDocument {
    pub agent: PathBuf,
    pub tests: PathBuf,
}
pub struct Snapshot {
    agent: Vec<u8>,
    tests: Vec<u8>,
}
fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}
// Each lookup is relative to an already-held directory, including on Windows.
fn open_parent(
    root: &cap_std::fs::Dir,
    relative: &Path,
) -> io::Result<(cap_std::fs::Dir, std::ffi::OsString)> {
    use cap_fs_ext::DirExt;
    let mut directory = root.try_clone()?;
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(invalid());
        };
        if parts.peek().is_none() {
            return Ok((directory, name.to_os_string()));
        }
        directory = directory.open_dir_nofollow(name)?;
    }
    Err(invalid())
}
fn read(root: &cap_std::fs::Dir, path: &Path) -> io::Result<Vec<u8>> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    let (parent, name) = open_parent(root, path)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let mut file = parent.open_with(name, &options)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE {
        return Err(invalid());
    }
    Ok(bytes)
}
impl Snapshot {
    pub fn capture(root: &Path, doc: &AgentDocument) -> io::Result<Self> {
        let root = cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
        Self::collect(|slot| {
            read(
                &root,
                match slot {
                    FileSlot::Agent => &doc.agent,
                    FileSlot::Tests => &doc.tests,
                },
            )
        })
    }
    fn collect(mut read: impl FnMut(FileSlot) -> io::Result<Vec<u8>>) -> io::Result<Self> {
        let agent = read(FileSlot::Agent)?;
        let tests = read(FileSlot::Tests)?;
        if agent != read(FileSlot::Agent)? || tests != read(FileSlot::Tests)? {
            return Err(invalid());
        }
        Ok(Self { agent, tests })
    }
    pub fn id(&self) -> SnapshotId {
        SnapshotId::of(&self.agent, &self.tests)
    }
    pub fn file(&self, slot: FileSlot) -> &[u8] {
        match slot {
            FileSlot::Agent => &self.agent,
            FileSlot::Tests => &self.tests,
        }
    }
}
#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    #[test]
    fn opened_parent_handle_survives_a_directory_symlink_swap() {
        use cap_std::{ambient_authority, fs::Dir};
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("sub")).unwrap();
        fs::write(temp.path().join("sub/a"), b"inside").unwrap();
        fs::write(outside.path().join("a"), b"outside").unwrap();
        let root = Dir::open_ambient_dir(temp.path(), ambient_authority()).unwrap();
        let (parent, name) = open_parent(&root, Path::new("sub/a")).unwrap();
        fs::rename(temp.path().join("sub"), temp.path().join("old")).unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("sub")).unwrap();
        assert_eq!(parent.read(name).unwrap(), b"inside");
        assert!(open_parent(&root, Path::new("sub/a")).is_err());
    }
    use super::*;
    #[test]
    fn edited_during_capture_is_rejected() {
        let mut n = 0;
        assert!(Snapshot::collect(|_| {
            n += 1;
            Ok(vec![n])
        })
        .is_err());
    }
}
