use super::*;
use std::{fs, path::Path};

fn fixture(home: &Path, name: &str, id: DocumentId) -> PathBuf {
    let root = home.join("Orka/agents").join(name);
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::create_dir(root.join(".git/refs")).unwrap();
    fs::create_dir(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/info/exclude"), b"/.bridge-agent-folder\n").unwrap();
    fs::write(
        root.join(".git/config"),
        b"[core]\nrepositoryformatversion = 0\nbare = false\n",
    )
    .unwrap();
    fs::write(root.join(".git/HEAD"), b"").unwrap();
    fs::write(
        root.join(MARKER),
        format!("Bridge agent folder v1\n{}\n", id.0),
    )
    .unwrap();
    fs::write(root.join(format!("{name}.agent.md")), b"original helper").unwrap();
    fs::write(root.join(format!("{name}.agent.test.md")), b"helper check").unwrap();
    root
}
fn id() -> DocumentId {
    DocumentId(Uuid::new_v4())
}
fn committed(root: &Path, name: &str) -> Vec<u8> {
    let repository = git2::Repository::open(root).unwrap();
    let tree = repository
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .tree()
        .unwrap();
    let entry = tree.get_name(name).unwrap();
    let blob = repository.find_blob(entry.id()).unwrap();
    blob.content().to_vec()
}
#[test]
fn marker_observation_shares_the_document_lock() {
    let home = tempfile::tempdir().unwrap();
    let root = fixture(home.path(), "helper", id());
    let directory = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
    let mut observation = shared_marker(&directory).unwrap();
    let writer = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(MARKER))
        .unwrap();
    assert!(writer.try_lock().is_err());
    let mut bytes = Vec::new();
    observation.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, fs::read(root.join(MARKER)).unwrap());
    drop(observation);
    writer.try_lock().unwrap();
}
#[test]
fn created_document_uses_its_own_git_names() {
    let home = tempfile::tempdir().unwrap();
    let id = id();
    let root = fixture(home.path(), "helper", id);
    assert_eq!(
        AgentFolder::created_id(Some(home.path().into()), "helper").unwrap(),
        id
    );
    assert_eq!(fs::read(root.join(".git/HEAD")).unwrap(), b"");
    let owner = AgentFolder::created(Some(home.path().into()), "helper", id).unwrap();
    assert!(owner.available());
    owner.save("new helper draft".into()).unwrap();
    assert_eq!(committed(&root, "helper.agent.md"), b"new helper draft");
    assert_eq!(committed(&root, "helper.agent.test.md"), b"helper check");
    let repository = git2::Repository::open(&root).unwrap();
    assert_eq!(
        repository
            .index()
            .unwrap()
            .iter()
            .map(|entry| entry.path)
            .collect::<Vec<_>>(),
        [
            b"helper.agent.md".to_vec(),
            b"helper.agent.test.md".to_vec()
        ]
    );
    let (path, document) = owner.source().unwrap();
    assert_eq!(path, root);
    assert_eq!(document.agent, Path::new("helper.agent.md"));
    assert_eq!(
        owner.capture().unwrap().file(FileSlot::Agent),
        b"new helper draft"
    );
}
#[test]
fn missing_and_wrong_marker_documents_never_write() {
    let home = tempfile::tempdir().unwrap();
    let missing = AgentFolder::created(Some(home.path().into()), "missing", id()).unwrap();
    assert!(!missing.available());
    assert!(missing.open().is_err());
    assert!(missing.save("no".into()).is_err());
    assert!(missing.capture().is_err());
    assert!(!home.path().join("Orka").exists());
    let root = fixture(home.path(), "helper", id());
    let before = fs::read(root.join(MARKER)).unwrap();
    let owner = AgentFolder::created(Some(home.path().into()), "helper", id()).unwrap();
    assert!(!owner.available());
    assert!(owner.open().is_err());
    assert!(owner.save("no".into()).is_err());
    assert!(owner.capture().is_err());
    assert_eq!(fs::read(root.join(MARKER)).unwrap(), before);
    assert_eq!(
        fs::read(root.join("helper.agent.md")).unwrap(),
        b"original helper"
    );
    assert_eq!(fs::read(root.join(".git/HEAD")).unwrap(), b"");
}
#[test]
fn created_names_reject_paths_reserved_sample_and_windows_devices() {
    for name in [
        "",
        ".",
        "../helper",
        "a/b",
        "a\\b",
        "Helper",
        "-a",
        "a-",
        "a.b",
        "web-reader",
        "con",
        "prn",
        "aux",
        "nul",
        "com1",
        "com9",
        "lpt1",
        "lpt9",
        &"a".repeat(64),
    ] {
        assert!(AgentFolder::created(None, name, id()).is_err(), "{name}");
    }
    for name in ["a", "0", "my-helper", "com0", "lpt0", &"a".repeat(63)] {
        assert!(AgentFolder::created(None, name, id()).is_ok(), "{name}");
    }
    assert!(AgentFolder::created(None, "helper", DocumentId::SAMPLE).is_err());
}
#[cfg(unix)]
#[test]
fn created_redirected_roots_and_ancestors_never_write() {
    use std::os::unix::fs::symlink;
    for leaf in ["Orka", "Orka/agents", "Orka/agents/helper"] {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let id = id();
        let root = fixture(home.path(), "helper", id);
        let moved = outside.path().join("moved");
        let link = home.path().join(leaf);
        fs::rename(&link, &moved).unwrap();
        symlink(&moved, &link).unwrap();
        let owner = AgentFolder::created(Some(home.path().into()), "helper", id).unwrap();
        assert!(!owner.available());
        assert!(owner.save("no".into()).is_err());
        assert_eq!(
            fs::read(root.join("helper.agent.md")).unwrap(),
            b"original helper"
        );
    }
}
