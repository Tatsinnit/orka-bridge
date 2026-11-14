//! Durable intent precedes publication. Recovery never adopts by name or marker alone.
use super::*;
use crate::agent_folder::sync;
use bridge_runtime_protocol::Uuid;

const CREATE_ERROR: &str = "The agent could not be created. Check the Bridge folder is writable, then retry. Existing files have been kept.";
const COLLISION: &str =
    "That agent name is already in use or has an unfinished creation. Choose another name.";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity([u64; 2]);
impl Identity {
    pub(super) fn read(dir: &Dir) -> io::Result<Self> {
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            let meta = dir.dir_metadata()?;
            Ok(Self([meta.dev(), meta.ino()]))
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            };
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            if unsafe { GetFileInformationByHandle(dir.as_raw_handle(), &mut info) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self([
                u64::from(info.dwVolumeSerialNumber),
                u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
            ]))
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pending {
    pub(super) id: DocumentId,
    pub(super) name: String,
    stage: String,
    identity: Identity,
}
impl Pending {
    pub(super) fn valid(&self) -> bool {
        valid_name(&self.name)
            && self.id != DocumentId::SAMPLE
            && self.stage == format!(".bridge-create-{}", self.id.0)
    }
    fn matches(&self, parent: &Dir, leaf: &str, home: &Option<PathBuf>) -> io::Result<()> {
        let root = parent.open_dir_nofollow(leaf)?;
        if Identity::read(&root)? != self.identity {
            return Err(invalid());
        }
        AgentFolder::created(home.clone(), &self.name, self.id)
            .map_err(|_| invalid())?
            .verify_created(&root)
    }
    fn registration(&self) -> Registration {
        Registration {
            id: self.id,
            name: self.name.clone(),
            storage: Storage::Bridge,
        }
    }
}
impl Agents {
    /// Display only; renderer input never selects this root.
    pub fn creation_root(&self) -> Option<String> {
        self.home
            .as_ref()?
            .join("Orka")
            .join("agents")
            .into_os_string()
            .into_string()
            .ok()
    }
    fn creation_dir(&self, create: bool) -> io::Result<Dir> {
        let home = self.home.as_ref().ok_or_else(invalid)?;
        if !std::fs::symlink_metadata(home)?.file_type().is_dir() {
            return Err(invalid());
        }
        let home = Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
        if create {
            child(&child(&home, "Orka")?, "agents")
        } else {
            home.open_dir_nofollow("Orka")?.open_dir_nofollow("agents")
        }
    }
    pub(super) fn reconcile(&self, registry: &mut Registry, data: &Dir) -> Result<()> {
        if registry.pending.is_empty() {
            return Ok(());
        }
        let parent = match self.creation_dir(false) {
            Ok(parent) => parent,
            Err(_) => return Ok(()), // Missing/replaced directories do not confer recovery authority.
        };
        let mut recovered = Vec::new();
        for pending in &registry.pending {
            if matches!(parent.symlink_metadata(&pending.name), Err(e) if e.kind() == io::ErrorKind::NotFound)
                && pending.matches(&parent, &pending.stage, &self.home).is_ok()
            {
                // No fallback to replacing rename, even on an unsupported filesystem.
                let _ = publish_no_replace(&parent, &pending.stage, &pending.name);
            }
            if pending.matches(&parent, &pending.name, &self.home).is_ok() {
                sync(&parent).map_err(|_| CREATE_ERROR)?;
                recovered.push(pending.registration());
            }
        }
        if !recovered.is_empty() {
            registry
                .pending
                .retain(|p| !recovered.iter().any(|r| r.id == p.id));
            registry.agents.extend(recovered);
            registry.save(data)?;
        }
        Ok(())
    }
    pub fn create(&self, name: &str, model: Option<&str>) -> Result<DocumentId> {
        if !valid_name(name) {
            return Err("Use 1–63 lowercase letters, digits or hyphens, beginning and ending with a letter or digit. Reserved device names and web-reader are unavailable.");
        }
        if model.is_some_and(|m| {
            m.is_empty()
                || m.len() > 256
                || m.eq_ignore_ascii_case("auto")
                || !m.bytes().all(|b| (b'!'..=b'~').contains(&b))
        }) {
            return Err(
                "The default model is invalid. Reload the model catalog or retry without it.",
            );
        }
        self.mutate(|registry, data| {
            if registry.agents.iter().any(|r| r.name == name)
                || registry.pending.iter().any(|p| p.name == name)
            {
                return Err(COLLISION);
            }
            let parent = self.creation_dir(true).map_err(|_| CREATE_ERROR)?;
            match parent.symlink_metadata(name) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Ok(_) => return Err(COLLISION),
                Err(_) => return Err(CREATE_ERROR),
            }
            let id = DocumentId(Uuid::new_v4());
            let stage = format!(".bridge-create-{}", id.0);
            let builder = cap_std::fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use cap_std::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            parent
                .create_dir_with(&stage, &builder)
                .map_err(|_| CREATE_ERROR)?;
            let root = parent.open_dir_nofollow(&stage).map_err(|_| CREATE_ERROR)?;
            let folder = AgentFolder::created(self.home.clone(), name, id)?;
            folder
                .seed_created(&root, model)
                .map_err(|_| CREATE_ERROR)?;
            sync(&parent).map_err(|_| CREATE_ERROR)?;
            let pending = Pending {
                id,
                name: name.into(),
                stage,
                identity: Identity::read(&root).map_err(|_| CREATE_ERROR)?,
            };
            registry.pending.push(pending.clone());
            registry.save(data)?; // Must be durable before the destination can become visible.

            // cap-std Windows directory handles do not share DELETE access.
            drop(root);
            publish_no_replace(&parent, &pending.stage, name).map_err(|e| {
                if matches!(
                    e,
                    crate::runtime::image_types::ImageError::Storage(io::ErrorKind::AlreadyExists)
                ) {
                    COLLISION
                } else {
                    CREATE_ERROR
                }
            })?;
            sync(&parent).map_err(|_| CREATE_ERROR)?;
            pending
                .matches(&parent, name, &self.home)
                .map_err(|_| CREATE_ERROR)?;
            registry.agents.push(pending.registration());
            registry.pending.retain(|p| p.id != id);
            Ok(id)
        })
    }
}

// Reuse the image installer's retained-parent, replacement-disabled platform rename.
pub(super) use crate::runtime::image_cache::fs::publish_named as publish_no_replace;
