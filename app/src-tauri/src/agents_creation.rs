//! Durable intent precedes publication. Recovery never adopts by name or marker alone.
use super::*;
use crate::agent_folder::sync;
use bridge_runtime_protocol::Uuid;

const CREATE_ERROR: &str = "The agent could not be created. Check the selected folder is writable, then retry. Existing files have been kept.";
const COLLISION: &str =
    "That agent name is already in use or has an unfinished creation. Choose another name.";

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Identity([u64; 2]);
impl Identity {
    pub(crate) fn read(dir: &Dir) -> io::Result<Self> {
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
    #[serde(default, skip_serializing_if = "Storage::is_bridge")]
    pub(super) storage: Storage,
}
impl Pending {
    pub(super) fn valid(&self) -> bool {
        self.storage.valid_name(&self.name)
            && self.id != DocumentId::SAMPLE
            && self.stage == format!(".bridge-create-{}", self.id.0)
    }
    fn matches(&self, parent: &Dir, leaf: &str, agents: &Agents) -> io::Result<()> {
        let root = parent.open_dir_nofollow(leaf)?;
        if Identity::read(&root)? != self.identity {
            return Err(invalid());
        }
        agents
            .folder(&self.registration())
            .map_err(|_| invalid())?
            .verify_created(&root)
    }
    fn registration(&self) -> Registration {
        Registration {
            id: self.id,
            name: self.name.clone(),
            storage: self.storage.clone(),
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
        let mut recovered = Vec::new();
        for pending in &registry.pending {
            let parent = match &pending.storage {
                Storage::Bridge => self.creation_dir(false),
                Storage::Repo(binding) => binding.open_parent(),
            };
            let Ok(parent) = parent else { continue }; // Missing/replaced parents confer no authority.
            if matches!(parent.symlink_metadata(&pending.name), Err(e) if e.kind() == io::ErrorKind::NotFound)
                && pending.matches(&parent, &pending.stage, self).is_ok()
            {
                // No fallback to replacing rename, even on an unsupported filesystem.
                let _ = publish_no_replace(&parent, &pending.stage, &pending.name);
            }
            if pending.matches(&parent, &pending.name, self).is_ok() {
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
        self.create_with_repo(name, model, None)
    }
    pub fn create_with_repo(
        &self,
        name: &str,
        model: Option<&str>,
        repo_selection: Option<Uuid>,
    ) -> Result<DocumentId> {
        if !valid_name(name) || (repo_selection.is_none() && name == "web-reader") {
            return Err("Use 1–63 lowercase letters, digits or hyphens, beginning and ending with a letter or digit. Reserved device names are unavailable; web-reader is reserved only in Bridge\'s folder.");
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
        let mut selection = self.selection.lock().map_err(|_| CREATE_ERROR)?;
        let selected = match repo_selection {
            None => None,
            Some(id) => Some(
                selection
                    .as_ref()
                    .filter(|s| s.id == id)
                    .ok_or("Choose your repo again before creating the agent.")?,
            ),
        };
        let result = self.mutate(|registry, data| {
            let destination = selected.map(|s| &s.identity);
            if registry
                .agents
                .iter()
                .any(|r| r.name == name && r.storage.destination() == destination)
                || registry
                    .pending
                    .iter()
                    .any(|p| p.name == name && p.storage.destination() == destination)
            {
                return Err(COLLISION);
            }
            let parent = if let Some(selected) = selected {
                selected.revalidate().map_err(|_| CREATE_ERROR)?;
                selected.root.try_clone().map_err(|_| CREATE_ERROR)?
            } else {
                self.creation_dir(true).map_err(|_| CREATE_ERROR)?
            };
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
            let identity = Identity::read(&root).map_err(|_| CREATE_ERROR)?;
            let storage = selected.map_or(Storage::Bridge, |s| {
                Storage::Repo(repo::Binding {
                    path: s.path.clone(),
                    parent: s.identity.clone(),
                    child: identity.clone(),
                })
            });
            let folder = self.folder(&Registration {
                id,
                name: name.into(),
                storage: storage.clone(),
            })?;
            folder
                .seed_created(&root, model)
                .map_err(|_| CREATE_ERROR)?;
            sync(&parent).map_err(|_| CREATE_ERROR)?;
            let pending = Pending {
                id,
                name: name.into(),
                stage,
                identity,
                storage,
            };
            registry.pending.push(pending.clone());
            registry.save(data)?; // Must be durable before the destination can become visible.

            // cap-std Windows directory handles do not share DELETE access.
            drop(root);
            if let Some(selected) = selected {
                selected.revalidate().map_err(|_| CREATE_ERROR)?;
            }
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
                .matches(&parent, name, self)
                .map_err(|_| CREATE_ERROR)?;
            registry.agents.push(pending.registration());
            registry.pending.retain(|p| p.id != id);
            Ok(id)
        });
        if result.is_ok() && repo_selection.is_some() {
            // A failed create stays retryable; success releases the selected
            // directory handle (which otherwise prevents Windows repo moves).
            *selection = None;
        }
        result
    }
}

// Reuse the image installer's retained-parent, replacement-disabled platform rename.
pub(super) use crate::runtime::image_cache::fs::publish_named as publish_no_replace;
