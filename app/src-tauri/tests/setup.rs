use bridge_engine::setup::{SetupChoice, SetupStore};
use std::fs;

#[test]
fn reading_absent_selection_has_no_filesystem_or_runtime_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("not-created");
    let status = SetupStore::new(Some(root.clone())).status().unwrap();
    assert_eq!(
        serde_json::to_value(status).unwrap(),
        serde_json::json!({
            "selected": null, "localVm": "notIntegrated", "localVmError": null, "model": "locked",
            "modelSupported": false
        })
    );
    assert!(!root.exists());
}

#[test]
fn only_the_local_preference_survives_a_new_store_not_readiness() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SetupStore::new(Some(tmp.path().to_owned()));
    assert_eq!(
        store.select_local().unwrap().selected,
        Some(SetupChoice::LocalVm)
    );
    assert_eq!(
        store.select_local().unwrap().selected,
        Some(SetupChoice::LocalVm)
    );
    let retained = SetupStore::new(Some(tmp.path().to_owned()))
        .status()
        .unwrap();
    assert_eq!(
        serde_json::to_value(retained).unwrap(),
        serde_json::json!({
            "selected": "localVm", "localVm": "notIntegrated", "localVmError": null, "model": "locked",
            "modelSupported": false
        })
    );
    let files: Vec<_> = fs::read_dir(tmp.path().join("setup-v1")).unwrap().collect();
    assert_eq!(files.len(), 1);
    assert_eq!(fs::read(tmp.path().join("setup-v1/local-vm")).unwrap(), b"");
}

#[test]
fn unavailable_storage_has_a_fixed_error_without_paths() {
    let error = SetupStore::new(None).select_local().unwrap_err();
    let value = serde_json::to_value(error).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"kind":"storage", "message":"The setup choice could not be read or saved."})
    );
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("SECRET_MARKER");
    fs::write(&root, b"existing data").unwrap();
    let store = SetupStore::new(Some(root.clone()));
    assert!(store.status().is_err());
    assert!(!serde_json::to_string(&store.select_local().unwrap_err())
        .unwrap()
        .contains("SECRET_MARKER"));
    assert_eq!(fs::read(root).unwrap(), b"existing data");
}

#[test]
fn invalid_retained_data_is_not_overwritten_or_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("setup-v1")).unwrap();
    let marker = tmp.path().join("setup-v1/local-vm");
    fs::write(
        &marker,
        b"{\"model\":\"ready\",\"token\":\"SECRET_MARKER\"}",
    )
    .unwrap();
    let store = SetupStore::new(Some(tmp.path().to_owned()));
    assert!(store.status().is_err());
    assert!(store.select_local().is_err());
    assert!(fs::read_to_string(marker)
        .unwrap()
        .contains("SECRET_MARKER"));
}

#[test]
fn a_directory_cannot_be_a_saved_choice() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("setup-v1/local-vm")).unwrap();
    let store = SetupStore::new(Some(tmp.path().to_owned()));
    assert!(store.status().is_err());
    assert!(store.select_local().is_err());
}

#[test]
fn concurrent_selection_is_idempotent_and_creates_only_the_preference() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    SetupStore::new(Some(root.clone())).select_local().unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    });
    assert_eq!(fs::read_dir(root.join("setup-v1")).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn a_symlinked_preference_directory_is_not_used() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, tmp.path().join("setup-v1")).unwrap();
    let store = SetupStore::new(Some(tmp.path().to_owned()));
    assert!(store.status().is_err());
    assert!(store.select_local().is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn a_symlink_is_not_a_saved_choice_and_is_not_followed() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join("setup-v1")).unwrap();
    let target = tmp.path().join("private");
    fs::write(&target, b"SECRET_MARKER").unwrap();
    symlink(&target, tmp.path().join("setup-v1/local-vm")).unwrap();
    let store = SetupStore::new(Some(tmp.path().to_owned()));
    assert!(store.status().is_err());
    assert!(store.select_local().is_err());
    assert_eq!(fs::read(target).unwrap(), b"SECRET_MARKER");
}
