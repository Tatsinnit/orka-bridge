//! Durable first-job intent. The renderer supplies text, never a storage path.
use crate::{
    agent_folder::{invalid, read, regular, replace, sync},
    setup::{FirstJobPhase, FirstJobSnapshot},
};
use bridge_runtime_protocol::Uuid;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf, sync::Mutex};

const DIRECTORY: &str = "first-job-v1";
const RECORD: &str = "intent.json";
const OWNER: &str = "owner.lock";
const VERSION: u32 = 1;
const MAX_PROMPT: usize = 8 * 1024;
const MAX_RECORD: usize = 16 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u32,
    job: FirstJobSnapshot,
}

pub struct FirstJobStore {
    app_local_data: Option<PathBuf>,
    lock: Mutex<()>,
}

impl FirstJobStore {
    pub fn new(app_local_data: Option<PathBuf>) -> Self {
        Self {
            app_local_data,
            lock: Mutex::new(()),
        }
    }

    pub fn status(&self) -> io::Result<Option<FirstJobSnapshot>> {
        let _lock = self.lock.lock().map_err(|_| invalid())?;
        self.load()
    }

    pub fn begin(&self, prompt: String) -> io::Result<FirstJobSnapshot> {
        let prompt = normalize(prompt)?;
        let _lock = self.lock.lock().map_err(|_| invalid())?;
        let root = self.root(true)?.ok_or_else(invalid)?;
        let _owner = owner(&root)?;
        if let Some(existing) = Self::load_root(Some(&root))? {
            return if existing.prompt == prompt {
                Ok(existing)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a different first job is already saved",
                ))
            };
        }
        let job = FirstJobSnapshot {
            id: Uuid::new_v4(),
            prompt,
            phase: FirstJobPhase::Setup,
        };
        let bytes = serde_json::to_vec(&Record {
            version: VERSION,
            job: job.clone(),
        })
        .map_err(|_| invalid())?;
        if bytes.len() > MAX_RECORD {
            return Err(invalid());
        }
        replace(&root, RECORD, &bytes)?;
        Ok(job)
    }

    pub fn discard(&self, id: Uuid) -> io::Result<()> {
        if id.is_nil() {
            return Err(invalid());
        }
        let _lock = self.lock.lock().map_err(|_| invalid())?;
        let Some(root) = self.root(false)? else {
            return Err(io::ErrorKind::NotFound.into());
        };
        let _owner = owner(&root)?;
        let current = Self::load_root(Some(&root))?.ok_or(io::ErrorKind::NotFound)?;
        if current.id != id {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        root.remove_file(RECORD)?;
        sync(&root)
    }

    fn load(&self) -> io::Result<Option<FirstJobSnapshot>> {
        Self::load_root(self.root(false)?.as_ref())
    }

    fn load_root(root: Option<&Dir>) -> io::Result<Option<FirstJobSnapshot>> {
        let Some(root) = root else {
            return Ok(None);
        };
        match root.symlink_metadata(RECORD) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        let record: Record =
            serde_json::from_slice(&read(root, RECORD, MAX_RECORD)?).map_err(|_| invalid())?;
        if record.version != VERSION
            || record.job.id.is_nil()
            || record.job.prompt != normalize(record.job.prompt.clone())?
        {
            return Err(invalid());
        }
        Ok(Some(record.job))
    }

    fn root(&self, create: bool) -> io::Result<Option<Dir>> {
        let parent = self.app_local_data.as_ref().ok_or_else(invalid)?;
        match fs::symlink_metadata(parent) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                fs::create_dir_all(parent)?;
            }
            Err(error) => return Err(error),
            Ok(metadata) if !metadata.file_type().is_dir() => return Err(invalid()),
            Ok(_) => {}
        }
        let path = parent.join(DIRECTORY);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    fs::DirBuilder::new().mode(0o700).create(&path)?;
                }
                #[cfg(windows)]
                drop(crate::runtime::windows::security::create_af_directory(
                    &path,
                )?);
            }
            Err(error) => return Err(error),
            Ok(metadata) if !metadata.file_type().is_dir() => return Err(invalid()),
            Ok(_) => {}
        }
        #[cfg(windows)]
        let _private = crate::runtime::windows::security::private_af_directory(&path)?;
        let app_root = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
        if create {
            sync(&app_root)?;
        }
        let root = app_root.open_dir_nofollow(DIRECTORY)?;
        #[cfg(unix)]
        {
            use cap_std::fs::PermissionsExt;
            if root.dir_metadata()?.permissions().mode() & 0o077 != 0 {
                return Err(invalid());
            }
        }
        Ok(Some(root))
    }
}

fn owner(root: &Dir) -> io::Result<std::fs::File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    let file = root.open_with(OWNER, &options)?;
    regular(&file)?;
    let file = file.into_std();
    file.lock()?;
    Ok(file)
}

fn normalize(prompt: String) -> io::Result<String> {
    let prompt = prompt.replace("\r\n", "\n").replace('\r', "\n");
    let prompt = prompt.trim().to_owned();
    if prompt.is_empty()
        || prompt.len() > MAX_PROMPT
        || prompt.chars().any(|character| {
            character == '\0' || character.is_control() && !matches!(character, '\n' | '\t')
        })
    {
        return Err(invalid());
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_is_durable_and_only_same_prompt_retries_are_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let path = Some(temp.path().join("local"));
        let store = FirstJobStore::new(path.clone());
        let first = store
            .begin("  Explain this API\r\nbriefly.  ".into())
            .unwrap();
        assert_eq!(first.prompt, "Explain this API\nbriefly.");
        assert_eq!(store.begin(first.prompt.clone()).unwrap(), first);
        assert_eq!(FirstJobStore::new(path).status().unwrap(), Some(first));
        assert_eq!(
            store
                .begin("Replace the saved job".into())
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn invalid_or_corrupt_intent_never_becomes_replaceable_absence() {
        let temp = tempfile::tempdir().unwrap();
        let store = FirstJobStore::new(Some(temp.path().into()));
        for prompt in ["", " \r\n ", "contains\0nul"] {
            assert!(store.begin(prompt.into()).is_err());
        }
        let root = store.root(true).unwrap().unwrap();
        replace(&root, RECORD, br#"{"version":1,"job":{"id":"00000000-0000-0000-0000-000000000000","prompt":"job","phase":"setup"}}"#).unwrap();
        assert!(store.status().is_err());
        assert!(store.begin("new job".into()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn prompt_directory_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let store = FirstJobStore::new(Some(temp.path().into()));
        store.begin("A local job".into()).unwrap();
        let mode = fs::metadata(temp.path().join(DIRECTORY))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[test]
    fn discard_requires_the_current_native_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = FirstJobStore::new(Some(temp.path().into()));
        let job = store.begin("A local job".into()).unwrap();
        assert_eq!(
            store.discard(Uuid::new_v4()).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(store.status().unwrap(), Some(job.clone()));
        store.discard(job.id).unwrap();
        assert_eq!(store.status().unwrap(), None);
        assert_eq!(
            store.discard(job.id).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}
