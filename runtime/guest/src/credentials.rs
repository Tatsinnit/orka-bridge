//! Fixed endpoint for the authenticated host channel only. Callers must finish
//! Disconnect before clear/replacement: truncation does not revoke cached tokens.
use bridge_runtime_protocol::{Uuid, CREDENTIAL_PROFILE, MODEL_API_KEY_PROFILE};
#[cfg(test)]
use std::path::PathBuf;
use std::{
    ffi::{CStr, CString},
    fs::{File, Metadata},
    io::{self, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::Path,
};
fn endpoint_name(profile: u16) -> io::Result<&'static CStr> {
    match profile {
        CREDENTIAL_PROFILE => Ok(c"copilot-github-token"),
        MODEL_API_KEY_PROFILE => Ok(c"model-api-key"),
        _ => Err(io::ErrorKind::InvalidInput.into()),
    }
}
const FLAGS: libc::c_ulong = libc::ST_NOSUID | libc::ST_NODEV | libc::ST_NOEXEC;

#[derive(Debug)]
pub struct State {
    pub profile: u16,
    pub generation: Option<Uuid>,
    pub installed: bool,
}
pub struct CredentialStore {
    profile: u16,
    generation: Option<Uuid>,
    installed: bool,
    handles: Option<(File, File)>,
    known: [Option<File>; 2],
    temporary: Option<(CString, File)>,
    failed: bool,
    #[cfg(test)]
    layout: Option<(PathBuf, u32, u32, libc::c_ulong)>,
}
impl Default for CredentialStore {
    fn default() -> Self {
        Self {
            profile: CREDENTIAL_PROFILE,
            generation: None,
            installed: false,
            handles: None,
            known: [None, None],
            temporary: None,
            failed: false,
            #[cfg(test)]
            layout: None,
        }
    }
}
fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}
fn safe(e: io::Error) -> io::Error {
    e.kind().into()
}
fn syscall(result: libc::c_int) -> io::Result<()> {
    if result == -1 {
        Err(safe(io::Error::last_os_error()))
    } else {
        Ok(())
    }
}
fn open(at: libc::c_int, name: &CStr, flags: libc::c_int) -> io::Result<File> {
    // SAFETY: name is NUL terminated; ownership of a successful descriptor is transferred once.
    let fd = unsafe {
        libc::openat(
            at,
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY,
            0o600,
        )
    };
    syscall(fd)?;
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn same(a: &File, b: &File) -> io::Result<bool> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}
fn directory(file: &File, uid: u32, gid: Option<u32>, mode: u32) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_dir()
        || m.uid() != uid
        || gid.is_some_and(|g| m.gid() != g)
        || m.mode() & 0o7777 != mode
    {
        return Err(invalid());
    }
    Ok(())
}
fn endpoint(m: &Metadata, uid: u32, gid: u32, installed: bool) -> io::Result<()> {
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != uid
        || m.gid() != gid
        || m.mode() & 0o7777 != 0o640
        || if installed {
            !(1..=16384).contains(&m.len())
        } else {
            m.len() != 0
        }
    {
        return Err(invalid());
    }
    Ok(())
}
fn mount(file: &File, flags: libc::c_ulong) -> io::Result<()> {
    // SAFETY: both initialized output buffers are valid for their respective calls.
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    syscall(unsafe { libc::fstatfs(file.as_raw_fd(), &mut fs) })?;
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    syscall(unsafe { libc::fstatvfs(file.as_raw_fd(), &mut vfs) })?;
    if i128::from(fs.f_type) != i128::from(libc::TMPFS_MAGIC) || vfs.f_flag & flags != flags {
        return Err(invalid());
    }
    Ok(())
}
fn entries(file: &File) -> io::Result<Vec<CString>> {
    let fd = open(file.as_raw_fd(), c".", libc::O_RDONLY | libc::O_DIRECTORY)?;
    // SAFETY: fdopendir takes ownership on success; readdir names are copied before the next call.
    let dir = unsafe { libc::fdopendir(fd.as_raw_fd()) };
    if dir.is_null() {
        return Err(safe(io::Error::last_os_error()));
    }
    std::mem::forget(fd);
    let mut names = Vec::new();
    let result = loop {
        // Linux thread-local errno distinguishes EOF from a failed readdir.
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            break if io::Error::last_os_error().raw_os_error() == Some(0) {
                Ok(names)
            } else {
                Err(safe(io::Error::last_os_error()))
            };
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name != c"." && name != c".." {
            names.push(name.to_owned());
            if names.len() > 3 {
                break Err(invalid());
            }
        }
    };
    let closed = syscall(unsafe { libc::closedir(dir) });
    closed?;
    result
}
impl CredentialStore {
    fn layout(&self) -> (&Path, u32, u32, libc::c_ulong) {
        #[cfg(test)]
        if let Some((path, uid, gid, flags)) = &self.layout {
            return (path, *uid, *gid, *flags);
        }
        (Path::new("/run/bridge"), 0, 65532, FLAGS)
    }
    fn check(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(invalid());
        }
        let result = self.inspect();
        if result.is_err() {
            self.failed = true;
        }
        result.map_err(safe)
    }
    fn inspect(&mut self) -> io::Result<()> {
        let (path, uid, gid, flags) = self.layout();
        let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| invalid())?;
        let parent = open(libc::AT_FDCWD, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        directory(&parent, uid, None, 0o700)?;
        let dir = open(
            parent.as_raw_fd(),
            c"credentials",
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?;
        directory(&dir, uid, Some(gid), 0o750)?;
        mount(&dir, flags)?;
        if let Some((p, d)) = &self.handles {
            if !same(p, &parent)? || !same(d, &dir)? {
                return Err(invalid());
            }
        }
        for profile in [CREDENTIAL_PROFILE, MODEL_API_KEY_PROFILE] {
            if let Some(known) = &self.known[usize::from(profile - 1)] {
                let actual = open(dir.as_raw_fd(), endpoint_name(profile)?, libc::O_RDONLY)?;
                if !same(known, &actual)? {
                    return Err(invalid());
                }
                endpoint(
                    &actual.metadata()?,
                    uid,
                    gid,
                    self.installed && self.profile == profile,
                )?;
            }
        }
        for name in entries(&dir)? {
            if [CREDENTIAL_PROFILE, MODEL_API_KEY_PROFILE]
                .into_iter()
                .any(|profile| {
                    endpoint_name(profile).is_ok_and(|n| n == name.as_c_str())
                        && self.known[usize::from(profile - 1)].is_some()
                })
            {
                continue;
            }
            if let Some((owned_name, owned_file)) = &self.temporary {
                if &name == owned_name {
                    let actual = open(dir.as_raw_fd(), &name, libc::O_RDONLY)?;
                    let m = actual.metadata()?;
                    if !same(owned_file, &actual)?
                        || !m.is_file()
                        || m.nlink() != 1
                        || m.uid() != uid
                    {
                        return Err(invalid());
                    }
                    continue;
                }
            }
            return Err(invalid());
        }
        if let Some((name, file)) = &self.temporary {
            if !same(file, &open(dir.as_raw_fd(), name, libc::O_RDONLY)?)? {
                return Err(invalid());
            }
        }
        if self.handles.is_none() {
            self.handles = Some((parent, dir));
        }
        Ok(())
    }
    fn state(&self) -> State {
        State {
            profile: self.profile,
            generation: self.generation,
            installed: self.installed,
        }
    }
    fn cleanup(&mut self) -> io::Result<()> {
        if self.temporary.is_some() {
            self.check()?;
            let (name, _) = self.temporary.as_ref().ok_or_else(invalid)?;
            let dir = &self.handles.as_ref().ok_or_else(invalid)?.1;
            // SAFETY: check proved this exact name still denotes our retained inode.
            syscall(unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) })?;
            self.temporary = None;
        }
        Ok(())
    }
    pub fn status(&mut self) -> io::Result<State> {
        self.check()?;
        self.cleanup()?;
        Ok(self.state())
    }
    pub fn publish(
        &mut self,
        profile: u16,
        expected: Option<Uuid>,
        token: &[u8],
    ) -> io::Result<State> {
        endpoint_name(profile)?;
        if token.is_empty() || token.len() > 16384 || !token.iter().all(|b| (33..=126).contains(b))
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.check()?;
        if expected != self.generation || self.installed {
            return Err(invalid());
        }
        self.cleanup()?;
        let result = self.publish_inner(profile, token);
        if result.is_err() {
            self.cleanup().map_err(safe)?;
        }
        result.map_err(safe)
    }
    fn publish_inner(&mut self, profile: u16, token: &[u8]) -> io::Result<State> {
        let target = endpoint_name(profile)?;
        let index = usize::from(profile - 1);
        let (_, uid, gid, _) = self.layout();
        let dirfd = self.handles.as_ref().ok_or_else(invalid)?.1.as_raw_fd();
        let name =
            CString::new(format!(".credential-{}", Uuid::new_v4())).map_err(|_| invalid())?;
        let file = open(dirfd, &name, libc::O_RDWR | libc::O_CREAT | libc::O_EXCL)?;
        self.temporary = Some((name, file));
        let (_, file) = self.temporary.as_mut().ok_or_else(invalid)?;
        file.write_all(token)?;
        // SAFETY: the live exclusively created regular file is the only object changed.
        syscall(unsafe { libc::fchown(file.as_raw_fd(), uid, gid) })?;
        syscall(unsafe { libc::fchmod(file.as_raw_fd(), 0o640) })?;
        file.sync_all()?;
        endpoint(&file.metadata()?, uid, gid, true)?;
        self.check()?;
        let (name, file) = self.temporary.as_ref().ok_or_else(invalid)?;
        // Fresh publication must never overwrite an unknown endpoint. Replacement
        // is allowed only for the retained, checked, already-cleared inode.
        if self.known[index].is_none() {
            // musl does not export a renameat2 wrapper. The Linux syscall returns
            // only 0 or -1 here, so narrowing its result preserves errno handling.
            syscall(unsafe {
                libc::syscall(
                    libc::SYS_renameat2,
                    libc::c_long::from(dirfd),
                    name.as_ptr(),
                    libc::c_long::from(dirfd),
                    target.as_ptr(),
                    libc::c_long::from(libc::RENAME_NOREPLACE),
                ) as libc::c_int
            })?;
        } else {
            syscall(unsafe { libc::renameat(dirfd, name.as_ptr(), dirfd, target.as_ptr()) })?;
        }
        let actual = open(dirfd, target, libc::O_RDONLY)?;
        if !same(file, &actual)? {
            self.failed = true;
            return Err(invalid());
        }
        let (_, file) = self.temporary.take().ok_or_else(invalid)?;
        self.known[index] = Some(file);
        self.profile = profile;
        self.installed = true;
        self.check()?;
        self.generation = Some(Uuid::new_v4());
        Ok(self.state())
    }
    /// Requires completed Disconnect; this only clears the checked file inode.
    pub fn clear(&mut self, expected: Uuid) -> io::Result<State> {
        self.check()?;
        if self.generation != Some(expected) {
            return Err(invalid());
        }
        self.cleanup()?;
        self.check()?;
        let file = self.known[usize::from(self.profile - 1)]
            .as_ref()
            .ok_or_else(invalid)?;
        file.set_len(0).map_err(safe)?;
        file.sync_all().map_err(safe)?;
        self.installed = false;
        self.check()?;
        self.generation = Some(Uuid::new_v4());
        Ok(self.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{symlink, MetadataExt, PermissionsExt},
        path::Path,
    };

    const TOKEN: &[u8] = b"synthetic-only-A";
    struct Fixture {
        root: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let root = tempfile::Builder::new()
                .prefix("bridge-credential-test-")
                .tempdir_in("/dev/shm")
                .unwrap();
            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
            fs::create_dir(root.path().join("credentials")).unwrap();
            fs::set_permissions(
                root.path().join("credentials"),
                fs::Permissions::from_mode(0o750),
            )
            .unwrap();
            Self { root }
        }
        fn dir(&self) -> PathBuf {
            self.root.path().join("credentials")
        }
        fn file(&self) -> PathBuf {
            self.dir().join("copilot-github-token")
        }
        fn store(&self) -> CredentialStore {
            // This host lacks noexec: use real observed flags, not privileged qualification.
            use std::os::fd::AsRawFd;
            let file = fs::File::open(self.dir()).unwrap();
            let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
            assert_eq!(unsafe { libc::fstatvfs(file.as_raw_fd(), &mut vfs) }, 0);
            CredentialStore {
                layout: Some((
                    self.root.path().to_owned(),
                    unsafe { libc::geteuid() },
                    unsafe { libc::getegid() },
                    vfs.f_flag & FLAGS,
                )),
                ..Default::default()
            }
        }
        fn seed(&self, bytes: &[u8]) {
            fs::write(self.file(), bytes).unwrap();
            fs::set_permissions(self.file(), fs::Permissions::from_mode(0o640)).unwrap();
        }
    }
    fn identity(path: &Path) -> (u64, u64) {
        let m = fs::metadata(path).unwrap();
        (m.dev(), m.ino())
    }
    fn unchanged(f: &Fixture) {
        assert_eq!(fs::read(f.file()).unwrap(), TOKEN);
    }

    #[test]
    fn profiles_switch_only_after_clear_and_keep_inactive_inodes_empty() {
        let f = Fixture::new();
        let mut s = f.store();
        let key = f.dir().join("model-api-key");
        let first = s.publish(1, None, TOKEN).unwrap();
        let copilot_reader = File::open(f.file()).unwrap();
        assert!(s.publish(2, first.generation, b"synthetic-key").is_err());
        assert!(!key.exists());
        let cleared = s.clear(first.generation.unwrap()).unwrap();
        assert!(s.publish(2, first.generation, b"synthetic-key").is_err());
        let installed = s.publish(2, cleared.generation, b"synthetic-key").unwrap();
        assert_eq!(installed.profile, 2);
        assert_eq!(s.status().unwrap().profile, 2);
        assert_eq!(fs::read(&key).unwrap(), b"synthetic-key");
        assert_eq!(copilot_reader.metadata().unwrap().len(), 0);
        let key_reader = File::open(&key).unwrap();
        let m = key_reader.metadata().unwrap();
        assert_eq!((m.mode() & 0o7777, m.nlink()), (0o640, 1));
        assert_eq!(
            (m.uid(), m.gid()),
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() })
        );
        let zero = s.clear(installed.generation.unwrap()).unwrap();
        let again = s.publish(2, zero.generation, b"synthetic-next").unwrap();
        assert_ne!(again.generation, installed.generation);
        assert_eq!(key_reader.metadata().unwrap().len(), 0);
        let zero = s.clear(again.generation.unwrap()).unwrap();
        let last = s.publish(1, zero.generation, TOKEN).unwrap();
        assert_eq!(last.profile, 1);
        assert_ne!(
            identity(&f.file()),
            (
                copilot_reader.metadata().unwrap().dev(),
                copilot_reader.metadata().unwrap().ino()
            )
        );
        assert_eq!(copilot_reader.metadata().unwrap().len(), 0);
        assert_eq!(fs::read(&key).unwrap(), b"");
        assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 2);
        assert!(
            f.store().status().is_err(),
            "restart must not adopt either endpoint"
        );
    }
    #[test]
    fn api_key_endpoint_and_inactive_file_tampering_are_never_adopted() {
        for tamper in [false, true] {
            let f = Fixture::new();
            let mut s = f.store();
            let key = f.dir().join("model-api-key");
            if tamper {
                let a = s.publish(1, None, TOKEN).unwrap();
                let zero = s.clear(a.generation.unwrap()).unwrap();
                s.publish(2, zero.generation, b"synthetic-key").unwrap();
                fs::write(f.file(), TOKEN).unwrap();
            } else {
                fs::write(&key, TOKEN).unwrap();
            }
            assert!(s.status().is_err());
            assert!(s.publish(2, None, b"replacement").is_err());
            assert!(!fs::read(&key).unwrap().is_empty());
        }
        let f = Fixture::new();
        let mut s = f.store();
        assert!(s.publish(3, None, TOKEN).is_err());
        assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 0);
        let first = s.publish(1, None, TOKEN).unwrap();
        let zero = s.clear(first.generation.unwrap()).unwrap();
        s.publish(2, zero.generation, b"synthetic-key").unwrap();
        fs::write(f.dir().join("unknown"), TOKEN).unwrap();
        assert!(s.status().is_err());
        assert_eq!(fs::read(f.dir().join("unknown")).unwrap(), TOKEN);
    }
    #[test]
    fn publish_clear_and_republish_real_file() {
        let f = Fixture::new();
        let mut s = f.store();
        let initial = s.status().unwrap();
        assert_eq!(initial.generation, None);
        assert!(!initial.installed);
        let first = s.publish(1, None, TOKEN).unwrap();
        unchanged(&f);
        assert!(first.installed);
        assert!(first.generation.is_some());
        let old_reader = fs::File::open(f.file()).unwrap();
        let inode = identity(&f.file());
        let m = fs::metadata(f.file()).unwrap();
        assert_eq!(m.mode() & 0o7777, 0o640);
        assert_eq!(m.nlink(), 1);
        assert_eq!(
            (m.uid(), m.gid()),
            (unsafe { libc::geteuid() }, unsafe { libc::getegid() })
        );
        let cleared = s.clear(first.generation.unwrap()).unwrap();
        assert!(!cleared.installed);
        assert_ne!(cleared.generation, first.generation);
        assert_eq!(identity(&f.file()), inode);
        assert_eq!(old_reader.metadata().unwrap().len(), 0);
        assert_eq!(fs::read(f.file()).unwrap(), b"");
        assert_eq!(s.status().unwrap().generation, cleared.generation);
        let second = s.publish(1, cleared.generation, b"synthetic-B").unwrap();
        assert_ne!(second.generation, cleared.generation);
        assert!(second.installed);
        assert_ne!(identity(&f.file()), inode);
        assert_eq!(old_reader.metadata().unwrap().len(), 0);
        assert_eq!(fs::read(f.file()).unwrap(), b"synthetic-B");
        assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 1);
        drop(s);
        assert_eq!(fs::read(f.file()).unwrap(), b"synthetic-B");
    }
    #[test]
    fn clear_never_adopts_an_unknown_real_file() {
        let f = Fixture::new();
        f.seed(TOKEN);
        let inode = identity(&f.file());
        assert!(f.store().clear(Uuid::new_v4()).is_err());
        unchanged(&f);
        assert_eq!(identity(&f.file()), inode);
    }
    #[test]
    fn generations_and_explicit_clear_gate_all_writes() {
        let f = Fixture::new();
        let mut s = f.store();
        assert!(s.publish(1, Some(Uuid::new_v4()), TOKEN).is_err());
        assert!(!f.file().exists());
        let state = s.publish(1, None, TOKEN).unwrap();
        assert!(s.clear(Uuid::new_v4()).is_err());
        unchanged(&f);
        assert!(s.publish(1, state.generation, b"replacement").is_err());
        unchanged(&f);
        assert!(s.publish(1, None, b"replacement").is_err());
        unchanged(&f);
        let zero = s.clear(state.generation.unwrap()).unwrap();
        assert!(s.publish(1, state.generation, b"replacement").is_err());
        assert!(s.clear(state.generation.unwrap()).is_err());
        assert_eq!(s.status().unwrap().generation, zero.generation);
        assert_eq!(fs::read(f.file()).unwrap(), b"");
    }
    #[test]
    fn token_bounds_and_printable_ascii_precede_creation() {
        let f = Fixture::new();
        let mut s = f.store();
        for invalid in [
            vec![],
            vec![b'x'; 16385],
            b"bad\n".to_vec(),
            vec![32],
            vec![127],
            vec![128],
        ] {
            let e = s.publish(1, None, &invalid).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
            assert!(!e.to_string().contains("bad"));
            assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 0);
        }
        let state = s.publish(1, None, &[33; 16384]).unwrap();
        assert_eq!(fs::metadata(f.file()).unwrap().len(), 16384);
        let zero = s.clear(state.generation.unwrap()).unwrap();
        s.publish(1, zero.generation, &[126]).unwrap();
        assert_eq!(fs::read(f.file()).unwrap(), b"~");
    }
    #[test]
    fn restart_refuses_existing_endpoint_even_when_zero() {
        for zero in [false, true] {
            let f = Fixture::new();
            let mut s = f.store();
            let state = s.publish(1, None, TOKEN).unwrap();
            if zero {
                s.clear(state.generation.unwrap()).unwrap();
            }
            let before = fs::read(f.file()).unwrap();
            drop(s);
            let mut restarted = f.store();
            assert!(restarted.status().is_err());
            assert!(restarted.publish(1, None, b"new").is_err());
            assert_eq!(fs::read(f.file()).unwrap(), before);
        }
    }
    #[test]
    fn initial_unknown_temporary_is_retained_and_refused() {
        let f = Fixture::new();
        let temp = f.dir().join(".credential-unknown");
        fs::write(&temp, TOKEN).unwrap();
        let mut s = f.store();
        assert!(s.status().is_err());
        assert!(s.publish(1, None, TOKEN).is_err());
        assert_eq!(fs::read(temp).unwrap(), TOKEN);
        assert!(!f.file().exists());
    }
    #[test]
    fn symlink_and_fifo_are_rejected_without_following_or_blocking() {
        let f = Fixture::new();
        let outside = f.root.path().join("outside");
        fs::write(&outside, TOKEN).unwrap();
        symlink(&outside, f.file()).unwrap();
        assert!(f.store().publish(1, None, b"new").is_err());
        assert_eq!(fs::read(&outside).unwrap(), TOKEN);
        fs::remove_file(f.file()).unwrap();
        let path = std::ffi::CString::new(f.file().as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o640) }, 0);
        assert!(f.store().status().is_err());
    }
    #[test]
    fn hardlink_and_wrong_mode_block_clear_without_mutation() {
        for hardlink in [false, true] {
            let f = Fixture::new();
            let mut s = f.store();
            let state = s.publish(1, None, TOKEN).unwrap();
            if hardlink {
                fs::hard_link(f.file(), f.root.path().join("alias")).unwrap();
            } else {
                fs::set_permissions(f.file(), fs::Permissions::from_mode(0o600)).unwrap();
            }
            assert!(s.status().is_err());
            assert!(s.clear(state.generation.unwrap()).is_err());
            unchanged(&f);
        }
    }
    #[test]
    fn replaced_file_is_not_cleared_or_overwritten() {
        let f = Fixture::new();
        let mut s = f.store();
        let state = s.publish(1, None, TOKEN).unwrap();
        fs::rename(f.file(), f.root.path().join("old")).unwrap();
        f.seed(TOKEN);
        assert!(s.clear(state.generation.unwrap()).is_err());
        unchanged(&f);
        assert_eq!(fs::read(f.root.path().join("old")).unwrap(), TOKEN);
    }
    #[test]
    fn replaced_directory_is_not_used_for_any_mutation() {
        let f = Fixture::new();
        let mut s = f.store();
        s.status().unwrap();
        let old = f.root.path().join("old");
        fs::rename(f.dir(), &old).unwrap();
        fs::create_dir(f.dir()).unwrap();
        fs::set_permissions(f.dir(), fs::Permissions::from_mode(0o750)).unwrap();
        assert!(s.publish(1, None, TOKEN).is_err());
        assert_eq!(fs::read_dir(old).unwrap().count(), 0);
        assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 0);
    }
    #[test]
    fn replaced_parent_is_rejected() {
        let f = Fixture::new();
        let mut s = f.store();
        let state = s.publish(1, None, TOKEN).unwrap();
        let saved = f.root.path().join("saved");
        // Exchange the held parent with a separately owned real directory.
        let replacement = tempfile::Builder::new()
            .prefix("bridge-replacement-")
            .tempdir_in("/dev/shm")
            .unwrap();
        let old_path = replacement.path().join("old");
        fs::rename(f.root.path(), &old_path).unwrap();
        fs::create_dir(f.root.path()).unwrap();
        fs::set_permissions(f.root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(s.clear(state.generation.unwrap()).is_err());
        assert_eq!(
            fs::read(old_path.join("credentials/copilot-github-token")).unwrap(),
            TOKEN
        );
        assert!(!saved.exists());
    }
    #[test]
    fn changed_directory_permissions_are_not_repaired() {
        for parent in [false, true] {
            let f = Fixture::new();
            let mut s = f.store();
            let state = s.publish(1, None, TOKEN).unwrap();
            let path = if parent {
                f.root.path().to_owned()
            } else {
                f.dir()
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
            assert!(s.clear(state.generation.unwrap()).is_err());
            unchanged(&f);
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o7777, 0o777);
        }
    }
    #[test]
    fn only_exact_retained_temporary_identity_can_be_removed() {
        for replace in [false, true] {
            let f = Fixture::new();
            let mut s = f.store();
            s.status().unwrap();
            let name = CString::new(".credential-owned").unwrap();
            let path = f.dir().join(".credential-owned");
            let file = open(
                s.handles.as_ref().unwrap().1.as_raw_fd(),
                &name,
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
            )
            .unwrap();
            assert_eq!(file.metadata().unwrap().mode() & 0o7777, 0o600);
            s.temporary = Some((name, file));
            if replace {
                fs::rename(&path, f.root.path().join("old-temp")).unwrap();
                fs::write(&path, TOKEN).unwrap();
                assert!(s.status().is_err());
                assert_eq!(fs::read(path).unwrap(), TOKEN);
            } else {
                s.status().unwrap();
                assert!(!path.exists());
                assert!(s.temporary.is_none());
            }
        }
    }
    #[test]
    fn cleared_endpoint_replacement_blocks_publication() {
        let f = Fixture::new();
        let mut s = f.store();
        let installed = s.publish(1, None, TOKEN).unwrap();
        let cleared = s.clear(installed.generation.unwrap()).unwrap();
        fs::rename(f.file(), f.root.path().join("zero")).unwrap();
        f.seed(TOKEN);
        assert!(s.publish(1, cleared.generation, b"synthetic-new").is_err());
        unchanged(&f);
        assert_eq!(fs::metadata(f.root.path().join("zero")).unwrap().len(), 0);
        assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 1);
    }
    #[test]
    fn changed_owned_file_length_is_rejected_before_clear() {
        let f = Fixture::new();
        let mut s = f.store();
        let installed = s.publish(1, None, TOKEN).unwrap();
        fs::write(f.file(), vec![b'x'; 16385]).unwrap();
        assert!(s.clear(installed.generation.unwrap()).is_err());
        assert_eq!(fs::metadata(f.file()).unwrap().len(), 16385);
    }
    #[test]
    fn failed_initial_inspection_is_not_repaired_or_adopted_later() {
        let f = Fixture::new();
        f.seed(TOKEN);
        let mut s = f.store();
        assert!(s.status().is_err());
        fs::remove_file(f.file()).unwrap();
        assert!(s.status().is_err());
        assert!(s.publish(1, None, TOKEN).is_err());
        assert_eq!(fs::read_dir(f.dir()).unwrap().count(), 0);
    }
    #[test]
    fn symlinked_directories_and_missing_layout_fail_without_creation() {
        let f = Fixture::new();
        fs::remove_dir(f.dir()).unwrap();
        let outside = f.root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, f.dir()).unwrap();
        let err = f.store().publish(1, None, TOKEN).unwrap_err();
        assert!(!err.to_string().contains(f.root.path().to_str().unwrap()));
        assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
        let mut missing = f.store();
        fs::remove_file(f.dir()).unwrap();
        assert!(missing.status().is_err());
        assert!(!f.dir().exists());
    }
    #[test]
    fn actual_tmpfs_and_required_flags_are_checked() {
        let f = Fixture::new();
        let m = fs::File::open(f.dir()).unwrap();
        let mut info: libc::statfs = unsafe { std::mem::zeroed() };
        use std::os::fd::AsRawFd;
        assert_eq!(unsafe { libc::fstatfs(m.as_raw_fd(), &mut info) }, 0);
        assert_eq!(info.f_type, libc::TMPFS_MAGIC);
        let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstatvfs(m.as_raw_fd(), &mut vfs) }, 0);
        let flags = libc::ST_NOSUID | libc::ST_NODEV;
        assert_eq!(vfs.f_flag & flags, flags);
        assert!(f.store().status().is_ok());
        assert_eq!(mount(&m, FLAGS).is_ok(), vfs.f_flag & FLAGS == FLAGS);
        assert!(mount(&m, vfs.f_flag | (1 << 31)).is_err());
    }
}
