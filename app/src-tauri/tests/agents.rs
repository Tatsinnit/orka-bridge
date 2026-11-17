use bridge_engine::agents::{Agents, DocumentId};
use bridge_runtime_protocol::{Uuid, MAX_FILE};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
};

fn id() -> DocumentId {
    serde_json::from_value(json!(Uuid::new_v4().to_string())).unwrap()
}
fn root(home: &Path, name: &str) -> PathBuf {
    home.join("Orka").join("agents").join(name)
}
fn fixture(home: &Path, name: &str, id: DocumentId) -> PathBuf {
    let root = root(home, name);
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
        root.join(".bridge-agent-folder"),
        format!(
            "Bridge agent folder v1\n{}\n",
            serde_json::to_value(id).unwrap().as_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(
        root.join(format!("{name}.agent.md")),
        b"identical native draft",
    )
    .unwrap();
    fs::write(
        root.join(format!("{name}.agent.test.md")),
        b"identical check",
    )
    .unwrap();
    root
}
// This integration-test binary has no runtime descriptor-window fixtures.
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
fn owner(home: &Path, data: &Path) -> Agents {
    Agents::new(Some(home.into()), Some(data.into()))
}
fn registry(data: &Path) -> PathBuf {
    data.join("agents-v1/registry.json")
}
fn metadata(selected: DocumentId, records: &[(DocumentId, &str)]) -> Value {
    json!({"version":1, "selected": selected, "agents": records.iter().map(|(id,name)| json!({"id":id,"name":name,"storage":"bridge"})).collect::<Vec<_>>()})
}
fn write_registry(data: &Path, bytes: &[u8]) {
    fs::create_dir_all(data.join("agents-v1")).unwrap();
    fs::write(registry(data), bytes).unwrap();
}
#[test]
fn registrations_and_selection_survive_restart() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let helper = id();
    let other = id();
    fixture(home.path(), "helper", helper);
    fixture(home.path(), "other", other);
    let agents = owner(home.path(), data.path());
    assert_eq!(agents.register_created("helper").unwrap(), helper);
    assert_eq!(agents.register_created("helper").unwrap(), helper);
    assert_eq!(agents.register_created("other").unwrap(), other);
    assert_ne!(helper, other); // Identical bytes do not confer document identity.
    agents
        .get(helper)
        .unwrap()
        .save("saved helper".into())
        .unwrap();
    let helper_root = root(home.path(), "helper");
    assert_eq!(
        git(&helper_root, &["show", "HEAD:helper.agent.md"]),
        b"saved helper"
    );
    assert_eq!(
        git(&helper_root, &["show", "HEAD:helper.agent.test.md"]),
        b"identical check"
    );
    git(&helper_root, &["fsck", "--full"]);
    assert!(git(&helper_root, &["status", "--porcelain"]).is_empty());
    agents.select(helper).unwrap();
    let restarted = owner(home.path(), data.path());
    assert_eq!(restarted.selected().unwrap(), helper);
    assert_eq!(
        restarted.get(helper).unwrap().open().unwrap().agent,
        "saved helper"
    );
    let entries = restarted.list().unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.id, e.name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (DocumentId::SAMPLE, "web-reader"),
            (helper, "helper"),
            (other, "other")
        ]
    );
    assert!(entries[1].available);
    assert_eq!(
        entries[1].path.as_deref().map(std::path::Path::new),
        Some(
            root(home.path(), "helper")
                .join("helper.agent.md")
                .as_path()
        )
    );
    assert!(restarted.get(id()).is_err());
    assert!(restarted.select(id()).is_err());
    fixture(home.path(), "alias", helper);
    assert!(restarted.register_created("alias").is_err());
    fs::write(
        root(home.path(), "helper").join(".bridge-agent-folder"),
        format!(
            "Bridge agent folder v1\n{}\n",
            serde_json::to_value(other).unwrap().as_str().unwrap()
        ),
    )
    .unwrap();
    assert!(restarted.register_created("helper").is_err());
}
#[test]
fn reply_identity_is_not_the_pair_hash() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = owner(home.path(), data.path());
    let a = id();
    let b = id();
    fixture(home.path(), "alpha", a);
    fixture(home.path(), "beta", b);
    agents.register_created("alpha").unwrap();
    agents.register_created("beta").unwrap();
    let left = serde_json::to_value(agents.get(a).unwrap().open().unwrap()).unwrap();
    let right = serde_json::to_value(agents.get(b).unwrap().open().unwrap()).unwrap();
    assert_eq!(left["documentId"], json!(a));
    assert_eq!(right["documentId"], json!(b));
    assert_eq!(left["name"], "alpha");
    assert_eq!(left["source"], right["source"]);
    agents.get(a).unwrap().save("only alpha".into()).unwrap();
    assert_eq!(
        fs::read(root(home.path(), "beta").join("beta.agent.md")).unwrap(),
        b"identical native draft"
    );
    assert_eq!(
        serde_json::to_value(agents.sample().open().unwrap()).unwrap()["documentId"],
        json!(DocumentId::SAMPLE)
    );
}
#[test]
fn reads_remain_available_during_other_owner_publication() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let helper = id();
    fixture(home.path(), "helper", helper);
    let writer = owner(home.path(), data.path());
    writer.register_created("helper").unwrap();
    let reader = owner(home.path(), data.path());
    let thread = std::thread::spawn(move || {
        for _ in 0..200 {
            writer.select(helper).unwrap();
        }
    });
    for _ in 0..600 {
        assert_eq!(reader.list().unwrap().len(), 2);
        assert!(
            matches!(reader.selected().unwrap(), DocumentId::SAMPLE)
                || reader.selected().unwrap() == helper
        );
        assert!(reader.get(helper).is_ok());
    }
    thread.join().unwrap();
}
#[test]
fn concurrent_owners_preserve_both_registrations() {
    // Fresh directories repeatedly exercise both sides of first-lock creation.
    for _ in 0..16 {
        concurrent_registration_round();
    }
}
fn concurrent_registration_round() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let helper = id();
    let other = id();
    fixture(home.path(), "helper", helper);
    fixture(home.path(), "other", other);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = ["helper", "other"]
        .into_iter()
        .map(|name| {
            let agents = owner(home.path(), data.path());
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                agents.register_created(name).unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join()).collect();
    for result in results {
        result.unwrap();
    }
    let agents = owner(home.path(), data.path());
    assert_eq!(agents.list().unwrap().len(), 3);
    // A concurrent selection cannot be lost to a registry update made by another owner.
    let third = id();
    fixture(home.path(), "third", third);
    let barrier = Arc::new(Barrier::new(2));
    let a = owner(home.path(), data.path());
    let b = owner(home.path(), data.path());
    let waiting = barrier.clone();
    let thread = std::thread::spawn(move || {
        waiting.wait();
        a.select(helper).unwrap();
    });
    barrier.wait();
    b.register_created("third").unwrap();
    thread.join().unwrap();
    assert_eq!(agents.selected().unwrap(), helper);
    assert_eq!(agents.list().unwrap().len(), 4);
}
#[test]
fn corrupt_registry_preserves_bytes_and_sample_access() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let helper = id();
    fixture(home.path(), "helper", helper);
    let duplicate = metadata(DocumentId::SAMPLE, &[(helper, "helper"), (helper, "other")]);
    let cases = [
        b"not json".to_vec(),
        serde_json::to_vec(&duplicate).unwrap(),
        serde_json::to_vec(&metadata(
            DocumentId::SAMPLE,
            &[(helper, "helper"), (id(), "helper")],
        ))
        .unwrap(),
        serde_json::to_vec(&json!({"version":2,"selected":DocumentId::SAMPLE,"agents":[]}))
            .unwrap(),
        serde_json::to_vec(
            &json!({"version":1,"selected":DocumentId::SAMPLE,"agents":[],"unknown":1}),
        )
        .unwrap(),
        serde_json::to_vec(&metadata(id(), &[])).unwrap(),
        serde_json::to_vec(&metadata(DocumentId::SAMPLE, &[(helper, "web-reader")])).unwrap(),
        serde_json::to_vec(&metadata(
            DocumentId::SAMPLE,
            &[(DocumentId::SAMPLE, "helper")],
        ))
        .unwrap(),
        vec![b' '; MAX_FILE + 1],
    ];
    for corrupt in cases {
        write_registry(data.path(), &corrupt);
        let agents = owner(home.path(), data.path());
        assert!(agents.list().is_err());
        assert!(agents.selected().is_err());
        assert!(agents.get(helper).is_err());
        assert!(agents.select(DocumentId::SAMPLE).is_err());
        assert!(agents.register_created("helper").is_err());
        assert!(Arc::ptr_eq(
            &agents.sample(),
            &agents.get(DocumentId::SAMPLE).unwrap()
        ));
        assert!(agents.sample().open().is_ok());
        assert!(agents
            .sample()
            .save("sample remains writable".into())
            .is_ok());
        assert_eq!(fs::read(registry(data.path())).unwrap(), corrupt);
    }
}
#[test]
fn list_is_observation_and_lookup_shares_owner() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let absent = data.path().join("not-created");
    let agents = owner(home.path(), &absent);
    assert_eq!(agents.selected().unwrap(), DocumentId::SAMPLE);
    assert!(!agents.list().unwrap()[0].available);
    assert!(Arc::ptr_eq(
        &agents.sample(),
        &agents.get(DocumentId::SAMPLE).unwrap()
    ));
    assert!(!home.path().join("Orka").exists());
    assert!(!absent.exists());
    let agents = owner(home.path(), data.path());
    agents.list().unwrap();
    agents.selected().unwrap();
    agents.get(DocumentId::SAMPLE).unwrap();
    assert_eq!(fs::read_dir(data.path()).unwrap().count(), 0);
    let helper = id();
    let root = fixture(home.path(), "helper", helper);
    agents.register_created("helper").unwrap();
    assert!(Arc::ptr_eq(
        &agents.get(helper).unwrap(),
        &agents.get(helper).unwrap()
    ));
    assert_eq!(fs::read(root.join(".git/HEAD")).unwrap(), b"");
    fs::write(
        root.join("helper.agent.md"),
        b"name: editable-not-helper\nexternal draft",
    )
    .unwrap();
    assert!(agents.list().unwrap()[1].available);
    assert_eq!(agents.register_created("helper").unwrap(), helper);
    assert_eq!(fs::read(root.join(".git/HEAD")).unwrap(), b"");
    // Current metadata, not the populated cache, authorizes future lookups.
    write_registry(
        data.path(),
        &serde_json::to_vec(&metadata(DocumentId::SAMPLE, &[])).unwrap(),
    );
    assert!(agents.get(helper).is_err());
}
#[test]
fn registered_missing_or_replaced_folder_stays_unavailable() {
    for replacement in ["missing", "marker", "git", "head", "filenames"] {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let helper = id();
        let root = fixture(home.path(), "helper", helper);
        let agents = owner(home.path(), data.path());
        agents.register_created("helper").unwrap();
        agents.select(helper).unwrap();
        let cached = agents.get(helper).unwrap();
        match replacement {
            "missing" => fs::remove_dir_all(&root).unwrap(),
            "marker" => {
                fs::remove_dir_all(&root).unwrap();
                fixture(home.path(), "helper", id());
            }
            "git" => fs::remove_dir_all(root.join(".git")).unwrap(),
            "head" => fs::write(root.join(".git/HEAD"), b"invalid history").unwrap(),
            "filenames" => {
                fs::rename(root.join("helper.agent.md"), root.join("other.agent.md")).unwrap()
            }
            _ => unreachable!(),
        }
        let restarted = owner(home.path(), data.path());
        assert_eq!(restarted.selected().unwrap(), helper);
        assert!(!restarted.list().unwrap()[1].available, "{replacement}");
        for document in [cached, restarted.get(helper).unwrap()] {
            assert!(document.open().is_err(), "{replacement}");
            assert!(document.save("must not write".into()).is_err());
            assert!(document.capture().is_err());
        }
        if replacement == "missing" {
            assert!(!root.exists());
        }
        assert!(!home.path().join("Orka/agents/web-reader").exists());
    }
}
#[test]
fn invalid_names_and_noncanonical_markers_are_not_registered() {
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let agents = owner(home.path(), data.path());
    for name in ["../escape", "web-reader", "con", "a/b", "Helper"] {
        assert!(agents.register_created(name).is_err());
    }
    let helper: DocumentId =
        serde_json::from_value(json!("f9207adc-c4ca-4bdb-b6e5-3d58c43d2cf0")).unwrap();
    let root = fixture(home.path(), "helper", helper);
    let uuid = serde_json::to_value(helper)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    let sample = serde_json::to_value(DocumentId::SAMPLE)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    for marker in [
        format!("Bridge agent folder v1\n{}\n", uuid.to_uppercase()),
        format!("Bridge agent folder v1\n{}\n", uuid.replace('-', "")),
        format!("Bridge agent folder v1\n{uuid}"),
        format!("Bridge agent folder v1\n{sample}\n"),
    ] {
        fs::write(root.join(".bridge-agent-folder"), &marker).unwrap();
        assert!(agents.register_created("helper").is_err());
        assert_eq!(
            fs::read(root.join(".bridge-agent-folder")).unwrap(),
            marker.as_bytes()
        );
    }
}
#[test]
fn hardlinked_registry_and_lock_never_change_outside_bytes() {
    for name in ["registry.json", "registry.lock"] {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        fixture(home.path(), "helper", id());
        let bytes = serde_json::to_vec(&metadata(DocumentId::SAMPLE, &[])).unwrap();
        write_registry(data.path(), &bytes);
        let target = data.path().join("agents-v1").join(name);
        if name == "registry.lock" {
            fs::write(&target, b"lock bytes").unwrap();
        }
        let before = fs::read(&target).unwrap();
        let outside = data.path().join("outside");
        fs::hard_link(&target, &outside).unwrap();
        let agents = owner(home.path(), data.path());
        assert!(agents.register_created("helper").is_err(), "{name}");
        assert!(agents.select(DocumentId::SAMPLE).is_err());
        assert_eq!(fs::read(&outside).unwrap(), before);
        if name == "registry.json" {
            assert!(agents.list().is_err());
        }
    }
}
#[cfg(unix)]
#[test]
fn symlinked_registry_directories_files_and_locks_never_redirect() {
    use std::os::unix::fs::symlink;
    for name in [
        "agents-v1",
        "agents-v1/registry.json",
        "agents-v1/registry.lock",
    ] {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fixture(home.path(), "helper", id());
        let bytes = serde_json::to_vec(&metadata(DocumentId::SAMPLE, &[])).unwrap();
        write_registry(data.path(), &bytes);
        let link = data.path().join(name);
        if name == "agents-v1" {
            fs::remove_dir_all(&link).unwrap();
            symlink(outside.path(), &link).unwrap();
        } else {
            if link.exists() {
                fs::remove_file(&link).unwrap();
            }
            fs::write(outside.path().join("target"), &bytes).unwrap();
            symlink(outside.path().join("target"), &link).unwrap();
        }
        let agents = owner(home.path(), data.path());
        assert!(agents.register_created("helper").is_err());
        assert!(agents.select(DocumentId::SAMPLE).is_err());
        if name == "agents-v1" {
            assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
            assert!(agents.list().is_err());
        } else {
            assert_eq!(fs::read(outside.path().join("target")).unwrap(), bytes);
        }
    }
}
