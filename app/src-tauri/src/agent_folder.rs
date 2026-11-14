//! One native-owned document. Paths are outbound display data, never write inputs.
use crate::runtime::snapshot::{AgentDocument, Snapshot};
use bridge_runtime_protocol::{FileSlot, SnapshotId, Uuid, MAX_FILE};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, MetadataExt, OpenOptions};
use std::{
    io::{self, Read, Seek, Write},
    path::PathBuf,
    sync::Mutex,
};
#[path = "agent_folder_git.rs"]
mod git;
const AGENT: &str = "web-reader.agent.md";
const TESTS: &str = "web-reader.agent.test.md";
const MARKER: &str = ".bridge-agent-folder";
const OWNER: &[u8] = b"Bridge web-reader folder v1\n";
type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct DocumentId(pub(crate) Uuid);
impl DocumentId {
    pub const SAMPLE: Self = Self(Uuid::from_u128(0x9b2b93c4_375c_4815_9717_39852a96e411));
}

pub(crate) fn valid_name(name: &str) -> bool {
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && alnum(bytes[0])
        && alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|b| alnum(*b) || *b == b'-')
        && !matches!(name, "web-reader" | "con" | "prn" | "aux" | "nul")
        && !(name.len() == 4
            && (name.starts_with("com") || name.starts_with("lpt"))
            && matches!(bytes[3], b'1'..=b'9'))
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRef {
    pub document_id: DocumentId,
    pub name: String,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPair {
    pub document_id: DocumentId,
    pub name: String,
    pub agent: String,
    pub tests: String,
    pub source: SnapshotId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
pub struct AgentFolder {
    home: Option<PathBuf>,
    name: String,
    agent: String,
    tests: String,
    owner: Vec<u8>,
    sample: bool,
    id: DocumentId,
    lock: Mutex<()>,
}
pub(crate) fn invalid() -> io::Error {
    io::ErrorKind::InvalidData.into()
}
pub(crate) fn child(parent: &Dir, name: &str) -> io::Result<Dir> {
    match parent.create_dir(name) {
        Ok(()) => (),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let directory = parent.open_dir_nofollow(name)?;
    sync(parent)?; // Also retry a prior mkdir's failed parent flush.
    Ok(directory)
}
pub(crate) fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    options
}
pub(crate) fn regular(file: &File) -> io::Result<()> {
    regular_links(file, false)
}
// Only immutable registry observations may retain an atomically unlinked inode.
pub(crate) fn regular_links(file: &File, unlinked: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(invalid());
    }
    #[cfg(unix)]
    if meta.nlink() > 1 || (!unlinked && meta.nlink() == 0) {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.nNumberOfLinks > 1
            || (!unlinked && info.nNumberOfLinks == 0)
            || meta.file_attributes() & 0x400 != 0
        {
            return Err(invalid());
        }
    }
    Ok(())
}
fn shared_marker(root: &Dir) -> io::Result<std::fs::File> {
    let marker = root.open_with(MARKER, &options())?;
    regular(&marker)?;
    let marker = marker.into_std();
    // Windows byte-range locks are mandatory: observe through the same shared-
    // locked handle rather than racing an exclusive document writer's marker.
    marker.lock_shared()?;
    Ok(marker)
}
pub(crate) fn read(dir: &Dir, name: &str, limit: usize) -> io::Result<Vec<u8>> {
    let file = dir.open_with(name, &options())?;
    regular(&file)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid());
    }
    Ok(bytes)
}
pub(crate) fn sync(dir: &Dir) -> io::Result<()> {
    #[cfg(unix)]
    let file = dir.open(".")?;
    #[cfg(windows)]
    let file = {
        use cap_std::fs::OpenOptionsExt;
        let mut opts = options();
        opts.write(true)
            .custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS);
        dir.open_with(".", &opts)?
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // Apple full-file flush is for regular files, not directory namespaces.
        if unsafe { libc::fsync(file.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(windows)]
    file.sync_all()
}
// All destinations are fixed single leaves on retained no-follow capabilities.
// A fresh inode + rename cannot truncate a hardlinked or symlinked destination.
pub(crate) fn replace(dir: &Dir, name: &str, bytes: &[u8]) -> io::Result<()> {
    let temp = format!(".bridge-{}", Uuid::new_v4());
    let mut options = options();
    options.write(true).create_new(true);
    let mut file = dir.open_with(&temp, &options)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        dir.rename(&temp, dir, name)?;
        sync(dir)
    })();
    if result.is_err() {
        let _ = dir.remove_file(&temp);
    }
    result
}
impl AgentFolder {
    pub fn document_id(&self) -> DocumentId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn document(&self) -> DocumentRef {
        DocumentRef {
            document_id: self.id,
            name: self.name.clone(),
        }
    }
    pub fn new(home: Option<PathBuf>) -> Self {
        Self {
            home,
            name: "web-reader".into(),
            agent: AGENT.into(),
            tests: TESTS.into(),
            owner: OWNER.into(),
            sample: true,
            id: DocumentId::SAMPLE,
            lock: Mutex::new(()),
        }
    }
    pub(crate) fn created(home: Option<PathBuf>, name: &str, id: DocumentId) -> Result<Self> {
        if !valid_name(name) || id == DocumentId::SAMPLE {
            return Err("The agent name or identity is invalid.");
        }
        Ok(Self {
            home,
            name: name.into(),
            agent: format!("{name}.agent.md"),
            tests: format!("{name}.agent.test.md"),
            owner: format!("Bridge agent folder v1\n{}\n", id.0).into_bytes(),
            sample: false,
            id,
            lock: Mutex::new(()),
        })
    }
    pub(crate) fn seed_created(&self, root: &Dir, model: Option<&str>) -> io::Result<()> {
        // JSON string quoting is also YAML scalar quoting, including YAML keywords.
        let name = serde_json::to_string(&self.name).map_err(|_| invalid())?;
        let mut agent = include_str!("../example-agent/web-reader.agent.md").replacen(
            "name: web-reader\n",
            &format!("name: {name}\n"),
            1,
        );
        if let Some(model) = model {
            let scalar = serde_json::to_string(model).map_err(|_| invalid())?;
            agent = agent.replacen("model: gpt-5.6-terra\n", &format!("model: {scalar}\n"), 1);
        }
        let tests = include_str!("../example-agent/web-reader.agent.test.md").replacen(
            "# web-reader tests",
            &format!("# {} tests", self.name),
            1,
        );
        replace(root, MARKER, &self.owner)?;
        replace(root, &self.agent, agent.as_bytes())?;
        replace(root, &self.tests, tests.as_bytes())?;
        Self::init_history(root)?;
        git::commit(root, &self.pair(root)?, &self.agent, &self.tests)?;
        sync(root)
    }
    fn init_history(root: &Dir) -> io::Result<()> {
        let git = child(root, ".git")?;
        child(&git, "objects")?;
        child(&git, "refs")?;
        let info = child(&git, "info")?;
        replace(&info, "exclude", b"/.bridge-agent-folder\n")?;
        replace(
            &git,
            "config",
            b"[core]\nrepositoryformatversion = 0\nbare = false\n",
        )?;
        replace(&git, "HEAD", b"")
    }
    pub(crate) fn verify_created(&self, root: &Dir) -> io::Result<()> {
        let mut marker = shared_marker(root)?;
        let mut ownership = Vec::new();
        (&mut marker).take(129).read_to_end(&mut ownership)?;
        if ownership != self.owner {
            return Err(invalid());
        }
        self.pair(root)?;
        git::inspect(root)
    }
    pub(crate) fn created_id(home: Option<PathBuf>, name: &str) -> Result<DocumentId> {
        // Constructing an owner does not grant authority: validate the native marker
        // and the bound pair/Git layout before the registry can publish its identity.
        if !valid_name(name) {
            return Err("The agent name is invalid.");
        }
        let root = Self::open_root(&home, name).map_err(|_| "The created agent is unavailable.")?;
        let mut marker =
            shared_marker(&root).map_err(|_| "The created agent marker is unavailable.")?;
        let mut bytes = Vec::new();
        (&mut marker)
            .take(129)
            .read_to_end(&mut bytes)
            .map_err(|_| "The created agent marker is unavailable.")?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| "The created agent marker is invalid.")?;
        let uuid = text
            .strip_prefix("Bridge agent folder v1\n")
            .and_then(|s| s.strip_suffix('\n'))
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or("The created agent marker is invalid.")?;
        let id = DocumentId(uuid);
        if id == DocumentId::SAMPLE
            || bytes != format!("Bridge agent folder v1\n{uuid}\n").as_bytes()
        {
            return Err("The created agent marker is invalid.");
        }
        let folder = Self::created(home, name, id)?;
        if folder.pair(&root).is_err() || git::inspect(&root).is_err() {
            return Err("The created agent is unavailable.");
        }
        Ok(id)
    }
    fn root(&self) -> io::Result<Dir> {
        Self::open_root(&self.home, &self.name)
    }
    fn open_root(home: &Option<PathBuf>, name: &str) -> io::Result<Dir> {
        let home = home.as_ref().ok_or_else(invalid)?;
        if std::fs::symlink_metadata(home)?.file_type().is_symlink() {
            return Err(invalid());
        }
        Dir::open_ambient_dir(home, cap_std::ambient_authority())?
            .open_dir_nofollow("Orka")?
            .open_dir_nofollow("agents")?
            .open_dir_nofollow(name)
    }
    /// Observe the owned pair without seeding, reconciling or committing it.
    pub fn inspect(&self) -> Result<Option<AgentPair>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "The saved file is unreadable.")?;
        let root = match self.root() {
            Ok(root) => root,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("The saved file is unreadable."),
        };
        (|| -> io::Result<AgentPair> {
            let mut marker = shared_marker(&root)?;
            let mut ownership = Vec::new();
            (&mut marker).take(129).read_to_end(&mut ownership)?;
            if ownership != self.owner {
                return Err(invalid());
            }
            let pair = self.pair(&root)?;
            git::inspect(&root)?;
            Ok(pair)
        })()
        .map(Some)
        .map_err(|_| "The saved file is unreadable.")
    }
    pub(crate) fn available(&self) -> bool {
        (|| -> io::Result<()> {
            let root = self.root()?;
            let mut marker = shared_marker(&root)?;
            let mut ownership = Vec::new();
            (&mut marker).take(129).read_to_end(&mut ownership)?;
            if ownership != self.owner {
                return Err(invalid());
            }
            self.pair(&root)?;
            git::inspect(&root)
        })()
        .is_ok()
    }
    fn with<T>(&self, work: impl FnOnce(&Dir) -> io::Result<T>) -> Result<T> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "The local agent folder is unavailable.")?;
        let operation = || -> io::Result<T> {
            if !self.sample {
                let root = self.root()?;
                let mut opts = options();
                opts.write(true);
                let marker = root.open_with(MARKER, &opts)?;
                regular(&marker)?;
                let mut marker = marker.into_std();
                marker.lock()?;
                let mut ownership = Vec::new();
                (&mut marker).take(129).read_to_end(&mut ownership)?;
                if ownership != self.owner {
                    return Err(invalid());
                }
                sync(&root)?;
                return work(&root);
            }
            let home = self.home.as_ref().ok_or_else(invalid)?;
            if std::fs::symlink_metadata(home)?.file_type().is_symlink() {
                return Err(invalid());
            }
            let home = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
            let agents = child(&child(&home, "Orka")?, "agents")?;
            let created = match agents.create_dir("web-reader") {
                Ok(()) => true,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
                Err(e) => return Err(e),
            };
            let root = agents.open_dir_nofollow("web-reader")?;
            let mut opts = options();
            opts.write(true).create_new(created);
            let marker = root.open_with(MARKER, &opts)?;
            regular(&marker)?;
            let mut marker = marker.into_std();
            // Also serializes other native owners/processes. Never wait on renderer work.
            marker.lock()?;
            if created {
                marker.write_all(OWNER)?;
                marker.sync_all()?;
                replace(
                    &root,
                    AGENT,
                    include_bytes!("../example-agent/web-reader.agent.md"),
                )?;
                replace(
                    &root,
                    TESTS,
                    include_bytes!("../example-agent/web-reader.agent.test.md"),
                )?;
                Self::init_history(&root)?;
                sync(&root)?;
            } else {
                marker.rewind()?;
                let mut ownership = Vec::new();
                (&mut marker)
                    .take(OWNER.len() as u64 + 1)
                    .read_to_end(&mut ownership)?;
                if ownership != OWNER {
                    return Err(invalid());
                }
            }
            sync(&agents)?;
            // Retry a prior working-file rename's failed namespace flush even
            // when the visible bytes already match the next save/open.
            sync(&root)?;
            work(&root)
        };
        operation().map_err(|_| "The local agent folder could not be read or saved. Check that it is a Bridge-managed folder with regular files, then retry.")
    }
    pub(crate) fn file_path(&self) -> Option<PathBuf> {
        self.home.as_ref().map(|home| {
            home.join("Orka")
                .join("agents")
                .join(&self.name)
                .join(&self.agent)
        })
    }
    pub(crate) fn display_path(&self) -> Option<String> {
        self.file_path()
            .and_then(|path| path.into_os_string().into_string().ok())
    }
    fn pair(&self, root: &Dir) -> io::Result<AgentPair> {
        let agent = String::from_utf8(read(root, &self.agent, MAX_FILE)?).map_err(|_| invalid())?;
        let tests = String::from_utf8(read(root, &self.tests, MAX_FILE)?).map_err(|_| invalid())?;
        let source = SnapshotId::of(agent.as_bytes(), tests.as_bytes());
        Ok(AgentPair {
            document_id: self.id,
            name: self.name.clone(),
            agent,
            tests,
            source,
            path: self.display_path(),
        })
    }
    pub fn open(&self) -> Result<AgentPair> {
        self.with(|root| {
            let pair = self.pair(root)?;
            git::commit(root, &pair, &self.agent, &self.tests)?; // Dirty reopen is committed, never silently reported Saved.
            Ok(pair)
        })
    }
    pub fn save(&self, content: String) -> Result<AgentPair> {
        if content.len() > MAX_FILE {
            return Err("The agent file is too large to save.");
        }
        self.with(|root| {
            let mut pair = self.pair(root)?;
            git::validate(root)?;
            if pair.agent != content {
                // Preserve edits from another editor before replacing the disk version.
                git::commit(root, &pair, &self.agent, &self.tests)?;
                replace(root, &self.agent, content.as_bytes())?;
                pair.agent = content;
                pair.source = SnapshotId::of(pair.agent.as_bytes(), pair.tests.as_bytes());
            }
            git::commit(root, &pair, &self.agent, &self.tests)?; // Failure leaves draft bytes; retry reconciles disk with HEAD.
            Ok(pair)
        })
    }
    /// Trusted native Send seam only; never exposed through IPC.
    pub fn source(&self) -> Result<(PathBuf, AgentDocument)> {
        self.open()?;
        Ok((
            self.home
                .as_ref()
                .unwrap()
                .join("Orka/agents")
                .join(&self.name),
            AgentDocument {
                agent: self.agent.clone().into(),
                tests: self.tests.clone().into(),
            },
        ))
    }
    /// Capture confirmed saved agent bytes without requiring executable tests.
    pub fn capture_agent(&self) -> Result<Vec<u8>> {
        self.with(|root| {
            let pair = self.pair(root)?;
            git::commit(root, &pair, &self.agent, &self.tests)?;
            let bytes = read(root, &self.agent, MAX_FILE)?;
            if bytes != pair.agent.as_bytes() {
                return Err(invalid());
            }
            Ok(bytes)
        })
    }
    pub fn capture(&self) -> Result<Snapshot> {
        self.with(|root| {
            let pair = self.pair(root)?;
            git::commit(root, &pair, &self.agent, &self.tests)?;
            let snapshot = Snapshot::capture_dir(
                root,
                &AgentDocument {
                    agent: self.agent.clone().into(),
                    tests: self.tests.clone().into(),
                },
            )?;
            if snapshot.file(FileSlot::Agent) != pair.agent.as_bytes()
                || snapshot.file(FileSlot::Tests) != pair.tests.as_bytes()
            {
                return Err(invalid());
            }
            Ok(snapshot)
        })
    }
}

#[cfg(test)]
#[path = "agent_folder_tests.rs"]
mod tests;

#[cfg(all(test, unix))]
mod display_path_tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn unrepresentable_display_path_is_omitted() {
        // Test the projection itself: APFS cannot create a non-UTF-8 directory.
        let home = std::ffi::OsStr::from_bytes(b"/creator-\xff");
        assert!(AgentFolder::new(Some(home.into())).display_path().is_none());
    }
}
