//! Verified GitHub account metadata stays bound to the exact saved sign-in slot.
use super::*;

fn captured(account: Option<&str>) -> CapturedGithubToken {
    let token = CapturedGithubToken::try_from(SECRET).unwrap();
    match account {
        Some(login) => token.with_account(login),
        None => token,
    }
}
fn account_file(setup: &SetupStore) -> std::path::PathBuf {
    setup.root.as_deref().unwrap().join("model-account-v1")
}
// Reconciles a same-lease Ready acknowledgment, so each call reports one ready state.
fn connected(setup: &SetupStore, install: Uuid, vault: &FakeVault) -> ModelSetupState {
    let root = setup.root.as_deref().unwrap();
    let nonce = Uuid::new_v4();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(nonce);
    binding.connection = None;
    write_binding(root, &binding).unwrap();
    let expected = connection();
    let (mut client, worker) = peer(nonce, move |request| {
        assert!(matches!(request.body, RequestBody::ModelStatus {}));
        Some(remote(&expected, ModelPhase::Ready))
    });
    let state = setup
        .connect_model(install, selection(), &mut client, vault)
        .unwrap();
    drop(client);
    worker.join().unwrap();
    assert!(state.ready);
    state
}
fn stored(setup: &SetupStore) {
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Stored;
    binding.nonce = None;
    binding.connection = None;
    write_binding(root, &binding).unwrap();
}

#[test]
fn captured_account_is_reported_only_for_its_connected_slot_and_cleared_with_removal() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    let state = setup
        .associate_captured(install, &captured(Some("octo-cat1")), &vault)
        .unwrap();
    assert_eq!(state.account, None, "not ready is not a connected identity");
    let slot = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap()
        .slot;
    let record = fs::read_to_string(account_file(&setup)).unwrap();
    assert!(record.contains(&slot.to_string()) && record.contains("octo-cat1"));
    assert!(!record.contains("synthetic-model-only"));
    assert_eq!(
        connected(&setup, install, &vault).account.as_deref(),
        Some("octo-cat1")
    );
    // Binding without the new field stays readable by the existing strict schema.
    let binding = fs::read_to_string(setup.root.as_deref().unwrap().join("model-v1")).unwrap();
    assert!(!binding.contains("octo-cat1"));
    // Unacknowledged removal keeps the metadata with its retained slot.
    stored(&setup);
    *vault.deny_delete.lock().unwrap() = true;
    assert!(setup.sign_out_model(install, None, &vault).is_err());
    assert!(account_file(&setup).exists());
    *vault.deny_delete.lock().unwrap() = false;
    setup.sign_out_model(install, None, &vault).unwrap();
    assert!(!account_file(&setup).exists());
}

#[test]
fn legacy_or_unverified_sign_in_reports_the_account_as_unavailable() {
    let (_tmp, setup, install, vault) = associated();
    assert_eq!(connected(&setup, install, &vault).account, None);
    stored(&setup);
    setup.sign_out_model(install, None, &vault).unwrap();
    setup
        .associate_captured(install, &captured(None), &vault)
        .unwrap();
    assert!(!account_file(&setup).exists());
    assert_eq!(connected(&setup, install, &vault).account, None);
}

#[test]
fn mismatched_or_malformed_account_metadata_is_unavailable() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    setup
        .associate_captured(install, &captured(Some("octocat")), &vault)
        .unwrap();
    let slot = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap()
        .slot;
    let path = account_file(&setup);
    let valid = fs::read(&path).unwrap();
    for raw in [
        serde_json::json!({"slot": Uuid::new_v4(), "login": "octocat"}).to_string(),
        serde_json::json!({"slot": slot, "login": "two words"}).to_string(),
        serde_json::json!({"slot": slot, "login": "octocat", "token": "x"}).to_string(),
        format!(
            "{}{}",
            String::from_utf8(valid.clone()).unwrap(),
            " ".repeat(1024)
        ),
    ] {
        fs::write(&path, raw).unwrap();
        assert_eq!(connected(&setup, install, &vault).account, None);
    }
    let elsewhere = path.with_extension("elsewhere");
    fs::write(&elsewhere, &valid).unwrap();
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    assert_eq!(connected(&setup, install, &vault).account, None);
    fs::remove_file(&path).unwrap();
    fs::write(&path, &valid).unwrap();
    assert_eq!(
        connected(&setup, install, &vault).account.as_deref(),
        Some("octocat")
    );
}

#[test]
fn account_metadata_failure_never_blocks_saving_or_connecting() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    fs::create_dir(account_file(&setup)).unwrap();
    setup
        .associate_captured(install, &captured(Some("octocat")), &vault)
        .unwrap();
    assert_eq!(connected(&setup, install, &vault).account, None);
}

#[test]
fn key_bindings_never_report_a_github_account() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    setup
        .associate_secret_for_vendor(install, SECRET, Some(KeyVendor::Anthropic), None, &vault)
        .unwrap();
    let root = setup.root.as_deref().unwrap();
    let binding = read_binding(root).unwrap().unwrap();
    fs::write(
        account_file(&setup),
        serde_json::json!({"slot": binding.slot, "login": "octocat"}).to_string(),
    )
    .unwrap();
    assert_eq!(state(root, Some(&binding), true).account, None);
}
