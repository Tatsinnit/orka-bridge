use bridge_engine::agents::Agents;
#[cfg(unix)]
use bridge_runtime_protocol::MAX_FILE;
use bridge_runtime_protocol::{FileSlot, SnapshotId, Uuid};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

fn owner(home: &Path, data: &Path) -> Agents {
    Agents::new(Some(home.into()), Some(data.into()))
}
fn checkout(path: &Path) -> git2::Repository {
    git2::Repository::init(path).unwrap()
}
fn tree(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(path, path, &mut files);
    files
}
fn registry(data: &Path) -> PathBuf {
    data.join("agents-v1/registry.json")
}
fn pending(data: &Path, repo: &Path, staged: bool) -> bridge_engine::agents::DocumentId {
    let mut value: Value = serde_json::from_slice(&fs::read(registry(data)).unwrap()).unwrap();
    let record = value["agents"].as_array_mut().unwrap().remove(0);
    let id = record["id"].as_str().unwrap();
    let name = record["name"].as_str().unwrap();
    let stage = format!(".bridge-create-{id}");
    if staged {
        fs::rename(repo.join(name), repo.join(&stage)).unwrap();
    }
    value["pending"] = json!([{"id":id,"name":name,"stage":stage,
        "identity":record["storage"]["repo"]["child"],"storage":record["storage"]}]);
    fs::write(registry(data), serde_json::to_vec(&value).unwrap()).unwrap();
    serde_json::from_value(record["id"].clone()).unwrap()
}

#[test]
fn repo_save_without_expected_source_rejects_external_edit_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    folder.open().unwrap();
    fs::write(path.join("helper/helper.agent.md"), b"external edit").unwrap();
    let before = tree(&path);

    let result = folder.save("local draft".into());

    assert!(result.is_err(), "repo save must require an expected source");
    assert_eq!(tree(&path), before);
}

#[test]
fn repo_save_rejects_stale_agent_source_without_writing_files_or_git() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let original = folder.open().unwrap();
    fs::write(path.join("helper/helper.agent.md"), b"external edit").unwrap();
    let before = tree(&path);

    let error = folder
        .save_with_source("local draft".into(), Some(original.source))
        .err()
        .unwrap();

    assert_eq!(error, "The agent file changed on disk.");
    assert_eq!(tree(&path), before);
}

#[test]
fn repo_save_requires_expected_source_even_when_content_matches() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let pair = folder.open().unwrap();
    let before = tree(&path);

    for content in [pair.agent, "local draft".into()] {
        assert_eq!(
            folder.save_with_source(content, None).err().unwrap(),
            "The agent file changed on disk."
        );
        assert_eq!(tree(&path), before);
    }
}

#[test]
fn repo_save_rejects_invalid_snapshot_ids_even_when_content_matches() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let pair = folder.open().unwrap();
    let before = tree(&path);

    for invalid in [String::new(), "a".repeat(65), "A".repeat(64)] {
        for source in [
            SnapshotId {
                agent: invalid.clone(),
                tests: pair.source.tests.clone(),
            },
            SnapshotId {
                agent: pair.source.agent.clone(),
                tests: invalid.clone(),
            },
        ] {
            assert_eq!(
                folder
                    .save_with_source(pair.agent.clone(), Some(source))
                    .err()
                    .unwrap(),
                "The agent file changed on disk."
            );
            assert_eq!(tree(&path), before);
        }
    }
}

#[test]
fn repo_save_accepts_fresh_agent_source_without_touching_git() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let original = folder.open().unwrap();
    let git = tree(&path.join(".git"));

    let saved = folder
        .save_with_source("local draft".into(), Some(original.source.clone()))
        .unwrap();

    assert_eq!(saved.agent, "local draft");
    assert_ne!(saved.source.agent, original.source.agent);
    assert_eq!(saved.source.tests, original.source.tests);
    assert_eq!(saved.tests, original.tests);
    assert_eq!(
        fs::read(path.join("helper/helper.agent.md")).unwrap(),
        b"local draft"
    );
    assert_eq!(folder.open().unwrap().source, saved.source);
    assert_eq!(tree(&path.join(".git")), git);
}

#[test]
fn repo_save_preserves_independent_test_edit_without_rejecting_agent_draft() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let original = folder.open().unwrap();
    let git = tree(&path.join(".git"));
    fs::write(
        path.join("helper/helper.agent.test.md"),
        b"external test edit",
    )
    .unwrap();

    let saved = folder
        .save_with_source("local draft".into(), Some(original.source.clone()))
        .unwrap();

    assert_eq!(saved.agent, "local draft");
    assert_eq!(saved.tests, "external test edit");
    assert_ne!(saved.source.tests, original.source.tests);
    assert_eq!(
        fs::read(path.join("helper/helper.agent.md")).unwrap(),
        b"local draft"
    );
    assert_eq!(
        fs::read(path.join("helper/helper.agent.test.md")).unwrap(),
        b"external test edit"
    );
    assert_eq!(tree(&path.join(".git")), git);
}

#[test]
fn repo_save_lost_ack_retry_is_idempotent_but_different_draft_still_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let original = folder.open().unwrap();
    folder
        .save_with_source("local draft".into(), Some(original.source.clone()))
        .unwrap(); // Its ACK is lost; retry still carries the original source.
    fs::write(
        path.join("helper/helper.agent.test.md"),
        b"external test edit",
    )
    .unwrap();
    let before = tree(&path);

    let retried = folder
        .save_with_source("local draft".into(), Some(original.source.clone()))
        .unwrap();

    assert_eq!(retried.agent, "local draft");
    assert_eq!(retried.tests, "external test edit");
    assert_eq!(retried.source, folder.open().unwrap().source);
    assert_eq!(tree(&path), before);
    assert_eq!(
        folder
            .save_with_source("different draft".into(), Some(original.source))
            .err()
            .unwrap(),
        "The agent file changed on disk."
    );
    assert_eq!(tree(&path), before);
}

#[test]
fn repo_save_compares_source_after_waiting_for_cross_process_lock() {
    use std::{sync::mpsc, time::Duration};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let data = temp.path().join("data");
    let agents = owner(temp.path(), &data);
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let source = folder.open().unwrap().source;
    let lock_name = format!(
        "agents-v1/repo-{}.lock",
        serde_json::to_value(id).unwrap().as_str().unwrap()
    );
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join(lock_name))
        .unwrap();
    lock.lock().unwrap();
    let (started, ready) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        done.send(
            folder
                .save_with_source("local draft".into(), Some(source))
                .err(),
        )
        .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let blocked = completed.recv_timeout(Duration::from_millis(50)).is_err();
    fs::write(
        path.join("helper/helper.agent.md"),
        b"external edit while waiting",
    )
    .unwrap();
    let before = tree(&path);
    drop(lock);

    assert!(blocked);
    assert_eq!(
        completed.recv_timeout(Duration::from_secs(5)).unwrap(),
        Some("The agent file changed on disk.")
    );
    writer.join().unwrap();
    assert_eq!(tree(&path), before);
}

#[test]
fn repo_concurrent_owners_with_same_source_allow_only_one_different_draft() {
    use std::sync::{Arc, Barrier};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let data = temp.path().join("data");
    let agents = owner(temp.path(), &data);
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let first = agents.get(id).unwrap();
    let source = first.open().unwrap().source;
    let second = owner(temp.path(), &data).get(id).unwrap();
    let before = tree(&path.join(".git"));
    let tests = fs::read(path.join("helper/helper.agent.test.md")).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let writers: Vec<_> = [(first.clone(), "first draft"), (second, "second draft")]
        .into_iter()
        .map(|(folder, content)| {
            let barrier = barrier.clone();
            let source = source.clone();
            std::thread::spawn(move || {
                barrier.wait();
                folder.save_with_source(content.into(), Some(source))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let saved = results
        .iter()
        .find_map(|result| result.as_ref().ok())
        .unwrap();
    let error = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert_eq!(*error, "The agent file changed on disk.");
    assert_eq!(first.open().unwrap().agent, saved.agent);
    assert_eq!(
        fs::read(path.join("helper/helper.agent.test.md")).unwrap(),
        tests
    );
    assert_eq!(tree(&path.join(".git")), before);
}

#[test]
fn restored_exact_parent_reopens_without_restarting_cached_document() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let data = temp.path().join("data");
    let id = {
        let agents = owner(temp.path(), &data);
        let selected = agents.bind_repo(&path).unwrap();
        agents
            .create_with_repo("helper", None, Some(selected.id))
            .unwrap()
    };
    let agents = owner(temp.path(), &data);
    let away = temp.path().join("away");
    fs::rename(&path, &away).unwrap();
    let folder = agents.get(id).unwrap();
    assert!(folder.open().is_err());
    fs::rename(&away, &path).unwrap();
    assert!(agents.list().unwrap()[1].available);
    let pair = folder.open().unwrap();
    assert_eq!(pair.document_id, id);
    assert_eq!(
        folder
            .save_with_source("restored draft".into(), Some(pair.source))
            .unwrap()
            .agent,
        "restored draft"
    );
}

#[test]
fn successful_creation_releases_selection_and_does_not_pin_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    folder.open().unwrap();
    assert_eq!(
        agents
            .create_with_repo("other", None, Some(selected.id))
            .unwrap_err(),
        "Choose your repo again before creating the agent."
    );
    let away = temp.path().join("away");
    fs::rename(&path, &away).unwrap(); // Also proves no lifetime FILE_SHARE_DELETE blocker on Windows.
    assert!(folder.open().is_err());
    fs::rename(&away, &path).unwrap();
    assert!(folder.open().is_ok());
}

#[test]
fn bridge_pair_always_identifies_its_storage() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = Agents::new(Some(home.path().into()), Some(data.path().into()));
    let pair = serde_json::to_value(agents.sample().open().unwrap()).unwrap();
    assert_eq!(pair["storage"], json!("bridge"));
}

#[test]
fn repo_pair_roundtrips_all_entry_points_without_touching_git() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir(&home).unwrap();
    let data = temp.path().join("data");
    let path = temp.path().join("repo");
    let repo = checkout(&path);
    let path = path.canonicalize().unwrap();
    fs::write(
        path.join(".git/index"),
        b"not an index; must never be opened",
    )
    .unwrap();
    fs::write(
        path.join(".git/hooks/pre-commit"),
        b"invalid hook must never execute",
    )
    .unwrap();
    repo.config()
        .unwrap()
        .set_str("filter.host.clean", "never-run-host-filter")
        .unwrap();
    fs::write(path.join(".gitattributes"), b"*.md filter=host\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let sentinel = temp.path().join("executed");
        let script = format!("#!/bin/sh\ntouch '{}'\n", sentinel.display());
        for hook in ["pre-commit", "post-commit"] {
            let hook = path.join(".git/hooks").join(hook);
            fs::write(&hook, &script).unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let filter = temp.path().join("filter");
        fs::write(&filter, script).unwrap();
        fs::set_permissions(&filter, fs::Permissions::from_mode(0o700)).unwrap();
        repo.config()
            .unwrap()
            .set_str("filter.host.clean", filter.to_str().unwrap())
            .unwrap();
    }
    let before = tree(&path.join(".git"));
    let agents = owner(&home, &data);
    let selection = agents.bind_repo(&path).unwrap();
    assert!(Uuid::parse_str(
        serde_json::to_value(&selection).unwrap()["id"]
            .as_str()
            .unwrap()
    )
    .is_ok());
    let id = agents
        .create_with_repo("web-reader", Some("provider/model"), Some(selection.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let pair = folder.open().unwrap();
    assert_eq!(serde_json::to_value(&pair).unwrap()["storage"], "repo");
    assert!(pair.agent.contains("model: \"provider/model\""));
    assert_eq!(pair.document_id, id);
    let saved = folder
        .save_with_source(
            pair.agent.replace("fetch", "retrieve"),
            Some(pair.source.clone()),
        )
        .unwrap();
    assert_ne!(saved.source, pair.source);
    assert_eq!(folder.inspect().unwrap().unwrap().source, saved.source);
    assert_eq!(folder.capture_agent().unwrap(), saved.agent.as_bytes());
    let captured = folder.capture().unwrap();
    assert_eq!(captured.file(FileSlot::Agent), saved.agent.as_bytes());
    assert_eq!(captured.file(FileSlot::Tests), saved.tests.as_bytes());
    let (source, document) = folder.source().unwrap();
    assert_eq!(source, path.join("web-reader"));
    assert_eq!(document.agent, PathBuf::from("web-reader.agent.md"));
    assert_eq!(document.tests, PathBuf::from("web-reader.agent.test.md"));
    assert_eq!(fs::read_dir(&source).unwrap().count(), 2);
    assert_eq!(
        agents.list().unwrap()[1].path.as_deref(),
        path.join("web-reader").join("web-reader.agent.md").to_str()
    );
    assert!(agents.list().unwrap()[1].available);
    let inspected = agents
        .inspect_files(|file| file.pair.ok().flatten())
        .unwrap();
    assert_eq!(inspected.len(), 1);
    assert_eq!(inspected[0].document_id, id);
    agents.select(id).unwrap();
    let reopened = owner(&home, &data);
    assert_eq!(reopened.selected().unwrap(), id);
    assert_eq!(
        reopened.get(id).unwrap().open().unwrap().source,
        saved.source
    );
    assert_eq!(tree(&path.join(".git")), before);
    assert!(!temp.path().join("executed").exists());
    // Native selection tokens are session-only, not replayable after restart.
    assert!(reopened
        .create_with_repo("other", None, Some(selection.id))
        .is_err());
    assert!(!home.join("Orka").exists());
}

#[test]
fn names_are_destination_scoped_and_tokens_latest_only() {
    let temp = tempfile::tempdir().unwrap();
    let agents = owner(temp.path(), &temp.path().join("data"));
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    checkout(&a);
    checkout(&b);
    let a = a.canonicalize().unwrap();
    let b = b.canonicalize().unwrap();
    let first = agents.bind_repo(&a).unwrap();
    let second = agents.bind_repo(&b).unwrap();
    assert!(agents.bind_repo(&temp.path().join("missing")).is_err());
    assert!(agents
        .create_with_repo("helper", None, Some(first.id))
        .is_err());
    assert!(agents
        .create_with_repo("helper", None, Some(Uuid::new_v4()))
        .is_err());
    let b_id = agents
        .create_with_repo("helper", None, Some(second.id))
        .unwrap();
    assert!(agents
        .create_with_repo("helper", None, Some(second.id))
        .is_err());
    let first = agents.bind_repo(&a).unwrap();
    let a_id = agents
        .create_with_repo("helper", None, Some(first.id))
        .unwrap();
    assert_ne!(a_id, b_id);
    let bridge = agents.create("helper", None).unwrap();
    assert_ne!(a_id, bridge);
    assert!(agents.create("web-reader", None).is_err());
    for name in ["../escape", "con", "a/b", "Upper", "lpt1", "", "bad-"] {
        assert!(agents.create_with_repo(name, None, Some(first.id)).is_err());
    }
    // Full binding cache must not confuse same-named Bridge/repo registrations.
    assert_eq!(
        agents.get(a_id).unwrap().source().unwrap().0,
        a.join("helper")
    );
    assert_eq!(
        agents.get(b_id).unwrap().source().unwrap().0,
        b.join("helper")
    );
    assert_eq!(
        owner(temp.path(), &temp.path().join("data"))
            .list()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn picker_requires_the_selected_working_checkout_not_an_ancestor_or_bare_repo() {
    let temp = tempfile::tempdir().unwrap();
    let agents = owner(temp.path(), &temp.path().join("data"));
    let path = temp.path().join("repo");
    checkout(&path);
    fs::create_dir(path.join("subdir")).unwrap();
    assert!(agents.bind_repo(&path.join("subdir")).is_err());
    assert!(agents.bind_repo(&path.join(".git")).is_err());
    let bare = temp.path().join("bare");
    git2::Repository::init_bare(&bare).unwrap();
    assert!(agents.bind_repo(&bare).is_err());
    let selection = agents.bind_repo(&path).unwrap(); // unborn HEAD is sufficient
    assert!(agents
        .create_with_repo("helper", None, Some(selection.id))
        .is_ok());
}

#[test]
fn linked_worktree_is_valid_without_changing_shared_or_worktree_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    let repo = checkout(&path);
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree_object = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree_object, &[])
        .unwrap();
    let worktree = temp.path().join("linked");
    repo.worktree("linked", &worktree, None).unwrap();
    let before = tree(&path.join(".git"));
    let marker = fs::read(worktree.join(".git")).unwrap();
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&worktree).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    agents.get(id).unwrap().capture().unwrap();
    assert_eq!(tree(&path.join(".git")), before);
    assert_eq!(fs::read(worktree.join(".git")).unwrap(), marker);
}

#[test]
fn repo_collisions_are_never_adopted_and_failure_preserves_retry_selection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    fs::create_dir(path.join("helper")).unwrap();
    fs::write(path.join("helper/keep"), b"creator bytes").unwrap();
    assert!(agents
        .create_with_repo("helper", None, Some(selected.id))
        .is_err());
    assert_eq!(
        fs::read(path.join("helper/keep")).unwrap(),
        b"creator bytes"
    );
    assert!(agents
        .create_with_repo("other", None, Some(selected.id))
        .is_ok());
}

#[test]
fn repo_pending_recovers_only_complete_exact_staged_or_published_pair() {
    for staged in [false, true] {
        for damage in ["none", "missing", "copy", "collision"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("repo");
            checkout(&path);
            let data = temp.path().join("data");
            {
                let agents = owner(temp.path(), &data);
                let selected = agents.bind_repo(&path).unwrap();
                agents
                    .create_with_repo("helper", None, Some(selected.id))
                    .unwrap();
            }
            let id = pending(&data, &path, staged);
            let leaf = if staged {
                format!(
                    ".bridge-create-{}",
                    serde_json::to_value(id).unwrap().as_str().unwrap()
                )
            } else {
                "helper".into()
            };
            let folder = path.join(&leaf);
            match damage {
                "missing" => fs::remove_file(folder.join("helper.agent.test.md")).unwrap(),
                "copy" => {
                    fs::rename(&folder, path.join("retained")).unwrap();
                    fs::create_dir(&folder).unwrap();
                    for name in ["helper.agent.md", "helper.agent.test.md"] {
                        fs::copy(path.join("retained").join(name), folder.join(name)).unwrap();
                    }
                }
                "collision" if staged => {
                    fs::create_dir(path.join("helper")).unwrap();
                    fs::write(path.join("helper/keep"), b"keep").unwrap();
                }
                _ => (),
            }
            let recovered = damage == "none" || (damage == "collision" && !staged);
            let restarted = owner(temp.path(), &data);
            assert_eq!(
                restarted.list().unwrap().len(),
                if recovered { 2 } else { 1 },
                "{staged} {damage}"
            );
            assert_eq!(restarted.get(id).is_ok(), recovered);
            if staged && damage == "collision" {
                assert_eq!(fs::read(path.join("helper/keep")).unwrap(), b"keep");
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn retained_selection_and_registered_handles_reject_replaced_parent_and_child() {
    for replace_parent in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        checkout(&path);
        let data = temp.path().join("data");
        let agents = owner(temp.path(), &data);
        let selected = agents.bind_repo(&path).unwrap();
        let id = agents
            .create_with_repo("helper", None, Some(selected.id))
            .unwrap();
        let folder = agents.get(id).unwrap();
        let source = folder.open().unwrap().source;
        let old = if replace_parent {
            path.clone()
        } else {
            path.join("helper")
        };
        fs::rename(&old, temp.path().join("retained")).unwrap();
        fs::create_dir(&old).unwrap();
        assert!(folder.open().is_err());
        assert!(folder.save_with_source("no".into(), Some(source)).is_err());
        assert!(folder.source().is_err());
        assert!(folder.capture().is_err());
        assert!(folder.capture_agent().is_err());
        assert!(!agents.list().unwrap()[1].available);
        assert!(!owner(temp.path(), &data).list().unwrap()[1].available);
        assert_eq!(fs::read_dir(&old).unwrap().count(), 0);
        if replace_parent {
            assert!(agents
                .create_with_repo("other", None, Some(selected.id))
                .is_err());
        }
    }
}

#[test]
fn pending_parent_replacement_cannot_adopt_even_the_original_staged_child() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let data = temp.path().join("data");
    {
        let agents = owner(temp.path(), &data);
        let selected = agents.bind_repo(&path).unwrap();
        agents
            .create_with_repo("helper", None, Some(selected.id))
            .unwrap();
    }
    let id = pending(&data, &path, true);
    let stage = format!(
        ".bridge-create-{}",
        serde_json::to_value(id).unwrap().as_str().unwrap()
    );
    let retained = temp.path().join("retained");
    fs::rename(&path, &retained).unwrap();
    fs::create_dir(&path).unwrap();
    fs::rename(retained.join(&stage), path.join(&stage)).unwrap();
    let before = fs::read(registry(&data)).unwrap();
    assert_eq!(owner(temp.path(), &data).list().unwrap().len(), 1);
    assert_eq!(fs::read(registry(&data)).unwrap(), before);
    assert!(!path.join("helper").exists());
    assert!(path.join(stage).exists());
}

#[tokio::test]
async fn send_preflight_captures_repo_document_and_revalidates_its_pair() {
    use bridge_engine::{clusters::commands::ClusterSession, send::SendOwner};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let agents = owner(temp.path(), &temp.path().join("data"));
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let source = folder.open().unwrap().source;
    let send = SendOwner::new(agents.sample(), ClusterSession::default());
    let check = send.preflight_document(folder.clone(), "", "").await;
    assert_eq!(check.document_id, id);
    assert_eq!(check.source, Some(source));
    assert!(!check.can_send);
    fs::remove_file(path.join("helper/helper.agent.test.md")).unwrap();
    let check = send.preflight_document(folder, "", "").await;
    assert_eq!(check.document_id, id);
    assert!(check.source.is_none());
    assert!(!check.can_send);
    assert!(!temp.path().join("Orka").exists());
}

#[test]
fn app_data_document_lock_serializes_owners_without_blocking_registry_recovery() {
    use std::{
        sync::{mpsc, Arc},
        time::Duration,
    };
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    checkout(&path);
    let data = temp.path().join("data");
    let agents = Arc::new(owner(temp.path(), &data));
    let selected = agents.bind_repo(&path).unwrap();
    let waiting = agents
        .create_with_repo("waiting", None, Some(selected.id))
        .unwrap();
    let selected = agents.bind_repo(&path).unwrap();
    let id = agents
        .create_with_repo("helper", None, Some(selected.id))
        .unwrap();
    let folder = agents.get(id).unwrap();
    let source = folder.open().unwrap().source;
    pending(&data, &path, true);
    let lock_name = format!(
        "agents-v1/repo-{}.lock",
        serde_json::to_value(id).unwrap().as_str().unwrap()
    );
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data.join(lock_name))
        .unwrap();
    lock.lock().unwrap();
    let (started, ready) = mpsc::channel();
    let (done, completed) = mpsc::channel();
    let writer = std::thread::spawn(move || {
        started.send(()).unwrap();
        let result = folder.save_with_source("changed".into(), Some(source));
        done.send(result.is_ok()).unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(completed.recv_timeout(Duration::from_millis(50)).is_err());
    // A waiting document operation must not hold the registry/access mutex.
    let selector = agents.clone();
    let (selected, selection) = mpsc::channel();
    let registry_writer = std::thread::spawn(move || {
        selected.send(selector.select(id)).unwrap();
    });
    let result = selection.recv_timeout(Duration::from_secs(5));
    drop(lock); // Always unblock the writer before asserting the liveness result.
    assert!(result.unwrap().is_ok());
    assert!(completed.recv_timeout(Duration::from_secs(5)).unwrap());
    writer.join().unwrap();
    registry_writer.join().unwrap();
    assert!(agents.get(waiting).unwrap().open().is_ok());
    let registry: Value = serde_json::from_slice(&fs::read(registry(&data)).unwrap()).unwrap();
    assert!(registry.get("pending").is_none());
}

#[cfg(unix)]
#[test]
fn repo_pair_rejects_symlink_hardlink_and_oversize_content_without_clobbering() {
    use std::os::unix::fs::symlink;
    for damage in ["symlink", "hardlink", "large"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("repo");
        checkout(&path);
        let agents = owner(temp.path(), &temp.path().join("data"));
        let selected = agents.bind_repo(&path).unwrap();
        let id = agents
            .create_with_repo("helper", None, Some(selected.id))
            .unwrap();
        let folder = agents.get(id).unwrap();
        let source = folder.open().unwrap().source;
        let bound = path.join("helper/helper.agent.md");
        fs::remove_file(&bound).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        match damage {
            "symlink" => symlink(&outside, &bound).unwrap(),
            "hardlink" => fs::hard_link(&outside, &bound).unwrap(),
            _ => fs::write(&bound, vec![b'x'; MAX_FILE + 1]).unwrap(),
        }
        assert!(folder.open().is_err());
        assert!(folder.save_with_source("no".into(), Some(source)).is_err());
        assert!(folder.capture().is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    }
}
