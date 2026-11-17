//! Native-selected checkout capabilities. No Git operation here reads the index or writes Git.
use super::{creation::Identity, Agents, Result};
use crate::agent_folder::{invalid, options, regular};
use bridge_runtime_protocol::Uuid;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

const ERROR: &str =
    "Choose an existing Git working checkout, then retry. Existing files have been kept.";

#[derive(Serialize)]
pub struct RepoSelection {
    pub id: Uuid,
    pub path: String,
}
pub(super) struct Selection {
    pub(super) id: Uuid,
    pub(super) path: PathBuf,
    pub(super) identity: Identity,
    pub(super) root: Dir,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    pub(super) path: PathBuf,
    pub(super) parent: Identity,
    pub(super) child: Identity,
}
impl Binding {
    pub(super) fn valid(&self) -> bool {
        self.path.is_absolute() && self.path.to_str().is_some()
    }
    pub(super) fn open_parent(&self) -> io::Result<Dir> {
        let root = open(&self.path)?;
        if Identity::read(&root)? != self.parent {
            return Err(invalid());
        }
        Ok(root)
    }
}
fn open(path: &Path) -> io::Result<Dir> {
    if !path.is_absolute() || !std::fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(invalid());
    }
    Dir::open_ambient_dir(path, cap_std::ambient_authority())
}
fn checkout(path: &Path, root: &Dir) -> io::Result<()> {
    let repo = git2::Repository::open_ext(
        path,
        git2::RepositoryOpenFlags::NO_SEARCH,
        &[] as &[&std::ffi::OsStr],
    )
    .map_err(|_| invalid())?;
    if repo.is_bare()
        || Identity::read(&open(repo.workdir().ok_or_else(invalid)?)?)? != Identity::read(root)?
    {
        return Err(invalid());
    }
    Ok(())
}
impl Selection {
    pub(super) fn revalidate(&self) -> io::Result<()> {
        if Identity::read(&open(&self.path)?)? != self.identity
            || Identity::read(&self.root)? != self.identity
        {
            return Err(invalid());
        }
        checkout(&self.path, &self.root)
    }
}
impl Agents {
    /// Trusted native picker completion only. IPC accepts a UUID, never this path.
    pub fn bind_repo(&self, path: &Path) -> Result<RepoSelection> {
        if !std::fs::symlink_metadata(path)
            .map_err(|_| ERROR)?
            .file_type()
            .is_dir()
        {
            return Err(ERROR);
        }
        let path = path.canonicalize().map_err(|_| ERROR)?;
        let root = open(&path).map_err(|_| ERROR)?;
        checkout(&path, &root).map_err(|_| ERROR)?;
        let display = path.to_str().ok_or(ERROR)?.to_owned();
        let selection = Selection {
            id: Uuid::new_v4(),
            path,
            identity: Identity::read(&root).map_err(|_| ERROR)?,
            root,
        };
        let result = RepoSelection {
            id: selection.id,
            path: display,
        };
        *self.selection.lock().map_err(|_| ERROR)? = Some(selection);
        Ok(result)
    }
}

/// The locks live in app data, never in the creator's checkout. Reopen only the
/// recorded parent identity; a temporarily missing folder can return without repair.
pub(crate) struct Access {
    pub(crate) binding: Binding,
    locks: Dir,
}
impl Access {
    pub(crate) fn new(binding: Binding, locks: Dir) -> Self {
        Self { binding, locks }
    }
    pub(crate) fn root(&self, name: &str) -> io::Result<Dir> {
        let parent = self.binding.open_parent()?;
        let root = parent.open_dir_nofollow(name)?;
        if Identity::read(&root)? != self.binding.child {
            return Err(invalid());
        }
        Ok(root)
    }
    pub(crate) fn file_path(&self, name: &str) -> PathBuf {
        self.binding.path.join(name)
    }
    pub(crate) fn lock(&self, id: super::DocumentId, shared: bool) -> io::Result<std::fs::File> {
        let name = format!("repo-{}.lock", id.0);
        let mut opts = options();
        opts.write(true).create_new(true);
        let file = match self.locks.open_with(&name, &opts) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                opts.create_new(false);
                self.locks.open_with(&name, &opts)?
            }
            Err(e) => return Err(e),
        };
        regular(&file)?;
        let file = file.into_std();
        if shared {
            file.lock_shared()?;
        } else {
            file.lock()?;
        }
        Ok(file)
    }
}
