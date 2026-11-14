use super::*;
use std::{fs, path::Path};

fn owner(home: &Path, data: &Path) -> Agents {
    Agents::new(Some(home.into()), Some(data.into()))
}
fn folder(home: &Path, name: &str) -> PathBuf {
    home.join("Orka/agents").join(name)
}
fn registry(data: &Path) -> PathBuf {
    data.join("agents-v1/registry.json")
}
fn pending(home: &Path, data: &Path, name: &str, staged: bool) -> DocumentId {
    let agents = owner(home, data);
    let id = agents.create(name, None).unwrap();
    let path = folder(home, name);
    let dir = Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();
    let identity = super::creation::Identity::read(&dir).unwrap();
    drop(dir); // Windows directory handles prevent the fixture rename while retained.
    let stage = format!(".bridge-create-{}", id.0);
    if staged {
        fs::rename(&path, folder(home, &stage)).unwrap();
    }
    let value = serde_json::json!({"version":1,"selected":DocumentId::SAMPLE,"agents":[],
        "pending":[{"id":id,"name":name,"stage":stage,"identity":identity}]});
    fs::write(registry(data), serde_json::to_vec(&value).unwrap()).unwrap();
    id
}

#[test]
fn creation_uses_shipped_pair_and_commits_it_without_editing_sample() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = owner(home.path(), data.path());
    agents.sample().open().unwrap();
    agents
        .sample()
        .save("creator's edited sample".into())
        .unwrap();
    let sample_head = fs::read(folder(home.path(), "web-reader").join(".git/HEAD")).unwrap();
    let id = agents.create("helper", None).unwrap();
    let pair = agents.get(id).unwrap().inspect().unwrap().unwrap();
    assert_eq!(pair.name, "helper");
    assert_eq!(
        pair.agent,
        include_str!("../example-agent/web-reader.agent.md").replacen(
            "name: web-reader\n",
            "name: \"helper\"\n",
            1
        )
    );
    assert_eq!(
        pair.tests,
        include_str!("../example-agent/web-reader.agent.test.md").replacen(
            "# web-reader tests",
            "# helper tests",
            1
        )
    );
    let root = folder(home.path(), "helper");
    let repo = git2::Repository::open(&root).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.author().name().unwrap(), "Bridge");
    let tree = commit.tree().unwrap();
    for (name, bytes) in [
        ("helper.agent.md", pair.agent.as_bytes()),
        ("helper.agent.test.md", pair.tests.as_bytes()),
    ] {
        let entry = tree.get_name(name).unwrap();
        assert_eq!(repo.find_blob(entry.id()).unwrap().content(), bytes);
    }
    assert_eq!(repo.remotes().unwrap().len(), 0);
    let statuses = repo
        .statuses(Some(git2::StatusOptions::new().include_ignored(false)))
        .unwrap();
    assert!(
        statuses.is_empty(),
        "{:?}",
        statuses
            .iter()
            .map(|s| (s.path().unwrap().to_owned(), s.status()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        agents.sample().open().unwrap().agent,
        "creator's edited sample"
    );
    assert_eq!(
        fs::read(folder(home.path(), "web-reader").join(".git/HEAD")).unwrap(),
        sample_head
    );
    assert_eq!(agents.selected().unwrap(), DocumentId::SAMPLE);
    agents.select(id).unwrap();
    assert_eq!(owner(home.path(), data.path()).selected().unwrap(), id);
    assert_eq!(
        agents.creation_root().unwrap(),
        home.path().join("Orka").join("agents").to_str().unwrap()
    );
}

#[test]
fn valid_names_remain_yaml_strings_in_starter_metadata() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = owner(home.path(), data.path());
    for name in ["null", "true", "123", "helper"] {
        let id = agents.create(name, None).unwrap();
        let pair = agents.get(id).unwrap().open().unwrap();
        let yaml = pair
            .agent
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0;
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(value["name"].as_str(), Some(name), "{name}");
    }
}

#[test]
fn catalog_model_is_validated_and_serialized_as_one_yaml_scalar() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = owner(home.path(), data.path());
    for (i, model) in [
        "provider/model",
        "#tag:{quoted}",
        "'quoted'",
        "null",
        "true",
        "[model]",
        "quote\"and\\slash",
        &"m".repeat(256),
    ]
    .into_iter()
    .enumerate()
    {
        let id = agents.create(&format!("helper-{i}"), Some(model)).unwrap();
        let pair = agents.get(id).unwrap().open().unwrap();
        let yaml = pair
            .agent
            .strip_prefix("---\n")
            .unwrap()
            .split_once("\n---\n")
            .unwrap()
            .0;
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(value["model"].as_str(), Some(model));
    }
    for model in [
        "",
        "auto",
        "AuTo",
        "has space",
        "line\nbreak",
        "é",
        &"a".repeat(257),
    ] {
        assert!(
            agents.create("invalid-model", Some(model)).is_err(),
            "{model:?}"
        );
    }
    assert!(!folder(home.path(), "invalid-model").exists());
}

#[test]
fn invalid_names_do_not_create_files_or_registry() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = owner(home.path(), data.path());
    for name in [
        "",
        "Upper",
        "../escape",
        "a/b",
        "a\\b",
        "a.b",
        "-a",
        "a-",
        "con",
        "prn",
        "aux",
        "nul",
        "com1",
        "lpt9",
        "web-reader",
        &"a".repeat(64),
    ] {
        assert!(agents.create(name, None).is_err(), "{name:?}");
    }
    assert!(!home.path().join("Orka").exists());
    assert!(!registry(data.path()).exists());
    assert!(Agents::new(None, Some(data.path().into()))
        .creation_root()
        .is_none());
}

#[test]
fn collisions_never_merge_even_empty_or_unregistered_destinations() {
    for existing in ["empty", "creator-file"] {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let root = folder(home.path(), "helper");
        fs::create_dir_all(&root).unwrap();
        if existing == "creator-file" {
            fs::write(root.join("keep"), b"creator bytes").unwrap();
        }
        assert!(owner(home.path(), data.path())
            .create("helper", None)
            .is_err());
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            usize::from(existing == "creator-file")
        );
        if existing == "creator-file" {
            assert_eq!(fs::read(root.join("keep")).unwrap(), b"creator bytes");
        }
    }
}

#[test]
fn racing_publication_primitive_does_not_replace_an_empty_directory() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
    dir.create_dir("stage").unwrap();
    dir.write("stage/keep", b"staged bytes").unwrap();
    // Destination appears after staging and before the atomic publish.
    dir.create_dir("destination").unwrap();
    assert!(super::creation::publish_no_replace(&dir, "stage", "destination").is_err());
    assert_eq!(dir.read("stage/keep").unwrap(), b"staged bytes");
    assert_eq!(dir.entries().unwrap().count(), 2);
    assert_eq!(
        dir.open_dir("destination")
            .unwrap()
            .entries()
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn restart_recovers_complete_staged_or_published_exact_directory() {
    for (staged, trigger) in [
        (false, "list"),
        (true, "list"),
        (false, "selected"),
        (true, "create"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let id = pending(home.path(), data.path(), "helper", staged);
        let restarted = owner(home.path(), data.path());
        match trigger {
            "selected" => {
                restarted.selected().unwrap();
            }
            "create" => {
                restarted.create("other", None).unwrap();
            }
            _ => (),
        }
        let entries: Vec<_> = restarted
            .list()
            .unwrap()
            .into_iter()
            .filter(|e| e.name != "other")
            .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].id, id);
        assert!(entries[1].available);
        assert_eq!(restarted.selected().unwrap(), DocumentId::SAMPLE);
        assert!(restarted.get(id).unwrap().open().is_ok());
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(registry(data.path())).unwrap()).unwrap();
        assert!(value.get("pending").is_none());
        assert!(restarted.create("helper", None).is_err());
    }
}

#[test]
fn recovery_rejects_replaced_directory_even_with_copied_marker() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let id = pending(home.path(), data.path(), "helper", false);
    let root = folder(home.path(), "helper");
    let retained = folder(home.path(), "retained");
    fs::rename(&root, &retained).unwrap();
    fs::create_dir(&root).unwrap();
    for entry in fs::read_dir(&retained)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().unwrap().is_file())
    {
        fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
    }
    fs::rename(retained.join(".git"), root.join(".git")).unwrap();
    assert!(AgentFolder::created(Some(home.path().into()), "helper", id)
        .unwrap()
        .available());
    let restarted = owner(home.path(), data.path());
    assert_eq!(restarted.list().unwrap().len(), 1);
    assert_eq!(restarted.selected().unwrap(), DocumentId::SAMPLE);
    assert!(restarted.get(id).is_err());
    assert!(restarted.create("helper", None).is_err());
    assert!(restarted.register_created("helper").is_err());
    assert_eq!(restarted.list().unwrap().len(), 1);
    assert!(retained.join("helper.agent.md").exists());
    assert!(root.join("helper.agent.md").exists());
    assert!(restarted.create("other", None).is_ok());
}

#[cfg(unix)]
#[test]
fn unrecoverable_pending_reads_do_not_rewrite_the_registry() {
    use std::os::unix::fs::MetadataExt;
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    pending(home.path(), data.path(), "helper", false);
    fs::rename(folder(home.path(), "helper"), folder(home.path(), "moved")).unwrap();
    let path = registry(data.path());
    let before = fs::metadata(&path).unwrap().ino();
    let restarted = owner(home.path(), data.path());
    assert_eq!(restarted.list().unwrap().len(), 1);
    assert_eq!(fs::metadata(&path).unwrap().ino(), before);
    assert_eq!(restarted.selected().unwrap(), DocumentId::SAMPLE);
    assert_eq!(fs::metadata(path).unwrap().ino(), before);
}

#[test]
fn concurrent_creators_have_one_winner_without_clobbering() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let owners = [
        owner(home.path(), data.path()),
        owner(home.path(), data.path()),
    ];
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = owners
        .into_iter()
        .map(|agents| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                agents.create("helper", None)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    let restarted = owner(home.path(), data.path());
    assert_eq!(restarted.list().unwrap().len(), 2);
    fs::remove_dir_all(folder(home.path(), "helper")).unwrap();
    restarted.select(winner).unwrap();
    assert_eq!(restarted.selected().unwrap(), winner);
    assert!(!restarted.list().unwrap()[1].available);
    assert!(restarted.get(winner).unwrap().open().is_err());
}
