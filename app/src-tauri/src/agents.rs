//! Native document identity and registration. Paths are derived, never imported.
pub use crate::agent_folder::DocumentId;
use crate::agent_folder::{
    child, invalid, options, regular, regular_links, replace, valid_name, AgentFolder, AgentPair,
    DocumentRef,
};
use bridge_runtime_protocol::MAX_FILE;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read},
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[path = "agents_creation.rs"]
mod creation;
use creation::Pending;

#[cfg(test)]
#[path = "agents_creation_tests.rs"]
mod creation_tests;

#[cfg(test)]
mod reader_tests {
    use super::*;
    #[test]
    fn published_reader_keeps_an_unlinked_regular_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        replace(&root, "registry.json", b"old valid snapshot").unwrap();
        let file = root.open_with("registry.json", &options()).unwrap();
        replace(&root, "registry.json", b"new valid snapshot").unwrap();
        assert_eq!(registry_bytes(file).unwrap(), b"old valid snapshot");
    }
}

fn registry_bytes(file: cap_std::fs::File) -> io::Result<Vec<u8>> {
    regular_links(&file, true)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_FILE {
        return Err(invalid());
    }
    Ok(bytes)
}

type Result<T> = std::result::Result<T, &'static str>;
const ERROR: &str = "The agent registrations could not be read or saved.";

/// Native-only observation; pair bytes never cross the Overview IPC boundary.
pub struct InspectedFile {
    pub document: DocumentRef,
    pub pair: Result<Option<AgentPair>>,
}

#[derive(Debug, Serialize)]
pub struct AgentEntry {
    pub id: DocumentId,
    pub name: String,
    pub path: Option<String>,
    pub available: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Storage {
    Bridge,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    id: DocumentId,
    name: String,
    storage: Storage,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u32,
    selected: DocumentId,
    agents: Vec<Registration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<Pending>,
}
impl Default for Registry {
    fn default() -> Self {
        Self {
            version: 1,
            selected: DocumentId::SAMPLE,
            agents: Vec::new(),
            pending: Vec::new(),
        }
    }
}
impl Registry {
    fn save(&self, root: &Dir) -> Result<()> {
        let bytes = serde_json::to_vec(self).map_err(|_| ERROR)?;
        if bytes.len() > MAX_FILE {
            return Err(ERROR);
        }
        replace(root, "registry.json", &bytes).map_err(|_| ERROR)
    }
    fn load(root: Option<&Dir>) -> io::Result<Self> {
        let Some(root) = root else {
            return Ok(Self::default());
        };
        let bytes = match root.symlink_metadata("registry.json") {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e),
            Ok(_) => registry_bytes(root.open_with("registry.json", &options())?)?,
        };
        let value: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        if value.version != 1
            || value.agents.iter().any(|entry| {
                entry.id == DocumentId::SAMPLE
                    || !valid_name(&entry.name)
                    || !ids.insert(entry.id)
                    || !names.insert(&entry.name)
            })
            || (value.selected != DocumentId::SAMPLE && !ids.contains(&value.selected))
            || value
                .pending
                .iter()
                .any(|entry| !entry.valid() || !ids.insert(entry.id) || !names.insert(&entry.name))
        {
            return Err(invalid());
        }
        Ok(value)
    }
}

/// One app owner. The cache shares locks/Arcs, but freshly read metadata authorizes lookup.
pub struct Agents {
    home: Option<PathBuf>,
    app_data: Option<PathBuf>,
    sample: Arc<AgentFolder>,
    access: Mutex<HashMap<DocumentId, (String, Arc<AgentFolder>)>>,
}
impl Agents {
    pub fn new(home: Option<PathBuf>, app_data: Option<PathBuf>) -> Self {
        Self {
            sample: Arc::new(AgentFolder::new(home.clone())),
            home,
            app_data,
            access: Mutex::new(HashMap::new()),
        }
    }
    pub fn sample(&self) -> Arc<AgentFolder> {
        self.sample.clone()
    }

    fn root(&self, create: bool) -> io::Result<Option<Dir>> {
        let path = self.app_data.as_ref().ok_or_else(invalid)?;
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                // Only the native app-data root is trusted; all descendants use
                // retained no-follow capabilities, never recursive path creation.
                std::fs::create_dir_all(path)?;
            }
            Err(e) => return Err(e),
            Ok(meta) if !meta.file_type().is_dir() => return Err(invalid()),
            _ => (),
        }
        if !std::fs::symlink_metadata(path)?.file_type().is_dir() {
            return Err(invalid());
        }
        let parent = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        if create {
            return child(&parent, "agents-v1").map(Some);
        }
        match parent.symlink_metadata("agents-v1") {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
            Ok(_) => parent.open_dir_nofollow("agents-v1").map(Some),
        }
    }
    fn registry(&self) -> Result<Registry> {
        let registry =
            Registry::load(self.root(false).map_err(|_| ERROR)?.as_ref()).map_err(|_| ERROR)?;
        if registry.pending.is_empty() {
            return Ok(registry);
        }
        // Recovery is best effort for readers: an unfinished operation must not
        // make the last valid registration snapshot depend on write access.
        self.locked(|registry, _| Ok(registry.clone()))
            .or(Ok(registry))
    }
    fn mutate<T>(&self, work: impl FnOnce(&mut Registry, &Dir) -> Result<T>) -> Result<T> {
        self.locked(|registry, root| {
            let result = work(registry, root)?;
            registry.save(root)?;
            Ok(result)
        })
    }
    fn locked<T>(&self, work: impl FnOnce(&mut Registry, &Dir) -> Result<T>) -> Result<T> {
        // No document lock is held here. Other processes serialize on the same
        // regular file, and every writer reloads after acquiring that file lock.
        let _access = self.access.lock().map_err(|_| ERROR)?;
        let root = self.root(true).map_err(|_| ERROR)?.ok_or(ERROR)?;
        let mut opts = options();
        opts.write(true).create_new(true);
        // Darwin can return ENOENT when relative openat(O_CREAT) loses a create
        // race. Exclusive creation + an existing-only open avoids that path.
        let lock = match root.open_with("registry.lock", &opts) {
            Ok(lock) => lock,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                opts.create_new(false);
                root.open_with("registry.lock", &opts).map_err(|_| ERROR)?
            }
            Err(_) => return Err(ERROR),
        };
        regular(&lock).map_err(|_| ERROR)?;
        let lock = lock.into_std();
        lock.lock().map_err(|_| ERROR)?;
        let mut registry = Registry::load(Some(&root)).map_err(|_| ERROR)?;
        self.reconcile(&mut registry, &root)?;
        work(&mut registry, &root)
    }
    /// One registry snapshot and one inspection per owned folder; no selection writes.
    pub fn inspect_files<T>(
        &self,
        mut project: impl FnMut(InspectedFile) -> Option<T>,
    ) -> Result<Vec<T>> {
        let registry = self.registry()?;
        let mut files = Vec::new();
        // Project before reading the next pair so prompt/test bodies are not accumulated.
        let mut observe = |folder: &AgentFolder| {
            if let Some(file) = project(InspectedFile {
                document: folder.document(),
                pair: folder.inspect(),
            }) {
                files.push(file);
            }
        };
        observe(&self.sample);
        for entry in registry.agents {
            let folder = AgentFolder::created(self.home.clone(), &entry.name, entry.id)?;
            observe(&folder);
        }
        Ok(files)
    }
    pub fn list(&self) -> Result<Vec<AgentEntry>> {
        let registry = self.registry()?;
        let mut entries = vec![AgentEntry {
            id: DocumentId::SAMPLE,
            name: "web-reader".into(),
            path: self.sample.display_path(),
            available: self.sample.available(),
        }];
        for entry in registry.agents {
            let folder = AgentFolder::created(self.home.clone(), &entry.name, entry.id)?;
            entries.push(AgentEntry {
                id: entry.id,
                name: entry.name,
                path: folder.display_path(),
                available: folder.available(),
            });
        }
        Ok(entries)
    }
    pub fn get(&self, id: DocumentId) -> Result<Arc<AgentFolder>> {
        if id == DocumentId::SAMPLE {
            return Ok(self.sample());
        }
        let registry = self.registry()?;
        let mut cache = self.access.lock().map_err(|_| ERROR)?;
        let entry = registry
            .agents
            .iter()
            .find(|entry| entry.id == id)
            .ok_or("The agent is not registered.")?;
        if let Some((name, folder)) = cache.get(&id) {
            if name == &entry.name {
                return Ok(folder.clone());
            }
        }
        let folder = Arc::new(AgentFolder::created(self.home.clone(), &entry.name, id)?);
        cache.insert(id, (entry.name.clone(), folder.clone()));
        Ok(folder)
    }
    pub fn selected(&self) -> Result<DocumentId> {
        Ok(self.registry()?.selected)
    }
    pub fn select(&self, id: DocumentId) -> Result<()> {
        self.mutate(|registry, _| {
            if id != DocumentId::SAMPLE && !registry.agents.iter().any(|entry| entry.id == id) {
                return Err("The agent is not registered.");
            }
            registry.selected = id;
            Ok(())
        })
    }
    /// Trusted native seam for an already-published owned folder; no creation or import.
    pub fn register_created(&self, name: &str) -> Result<DocumentId> {
        if !valid_name(name) {
            return Err("The agent name is invalid.");
        }
        self.mutate(|registry, _| {
            let id = AgentFolder::created_id(self.home.clone(), name)?;
            if registry
                .pending
                .iter()
                .any(|entry| entry.id == id || entry.name == name)
            {
                return Err("The agent has an unfinished creation. Choose another name.");
            }
            if let Some(entry) = registry
                .agents
                .iter()
                .find(|entry| entry.id == id || entry.name == name)
            {
                return if entry.id == id && entry.name == name {
                    Ok(id)
                } else {
                    Err("The agent name or identity is already registered.")
                };
            }
            registry.agents.push(Registration {
                id,
                name: name.into(),
                storage: Storage::Bridge,
            });
            Ok(id)
        })
    }
}
