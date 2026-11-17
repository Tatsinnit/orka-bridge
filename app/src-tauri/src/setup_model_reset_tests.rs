use super::*;

#[test]
fn mismatch_reset_deletes_only_recorded_slot_in_every_phase_without_guest_io() {
    for phase in [
        Phase::Storing,
        Phase::Stored,
        Phase::Pending,
        Phase::Connected,
        Phase::Cleared,
    ] {
        for vendor in [None, Some(KeyVendor::Openai), Some(KeyVendor::Anthropic)] {
            let (_tmp, setup, old) = selected_setup();
            let root = setup.root.as_ref().unwrap();
            let vault = FakeVault::default();
            setup
                .associate_secret_for_vendor(old, SECRET, vendor, None, &vault)
                .unwrap();
            let mut binding = read_binding(root).unwrap().unwrap();
            let slot = binding.slot;
            binding.phase = phase;
            if matches!(phase, Phase::Pending | Phase::Connected | Phase::Cleared) {
                binding.nonce = Some(Uuid::new_v4());
                binding.connection = Some(ModelConnection {
                    id: Uuid::new_v4(),
                    generation: Uuid::new_v4(),
                    selection: default_selection(vendor),
                });
            }
            write_binding(root, &binding).unwrap();
            let unrelated = Uuid::new_v4();
            vault
                .slots
                .lock()
                .unwrap()
                .insert(unrelated, SECRET.to_vec());
            fs::write(root.join("keep-agent"), b"kept").unwrap();
            let current = Uuid::new_v4();
            let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("mismatch contacted guest"));
            let error = setup
                .connect_stored_model(current, &mut client, &vault, || panic!("recaptured"))
                .unwrap_err();
            assert_eq!(error.kind, "modelInstallationMismatch");
            let wire = serde_json::to_value(error).unwrap();
            assert_eq!(wire["recovery"]["savedInstallation"], old.to_string());
            assert_eq!(wire["recovery"]["installation"], current.to_string());
            assert_eq!(
                setup
                    .sign_out_model(current, Some(&mut client), &vault)
                    .unwrap_err()
                    .kind,
                "modelInstallationMismatch"
            );
            assert_eq!(
                setup
                    .connect_saved_key_model(current, KeyVendor::Openai, &mut client, &vault)
                    .unwrap_err()
                    .kind,
                "modelInstallationMismatch"
            );
            assert_eq!(
                setup
                    .connect_key_model(
                        current,
                        KeyVendor::Openai,
                        "sk-synthetic-only",
                        &mut client,
                        &vault
                    )
                    .unwrap_err()
                    .kind,
                "modelInstallationMismatch"
            );
            let result = setup.reset_mismatched_model(current, &vault).unwrap();
            assert!(!result.associated && !result.ready && !result.unknown);
            assert!(!root.join("model-v1").exists());
            assert_eq!(fs::read(root.join("keep-agent")).unwrap(), b"kept");
            assert_eq!(
                vault.calls.lock().unwrap().as_slice(),
                &[("save", slot), ("delete", slot)]
            );
            assert!(vault.slots.lock().unwrap().contains_key(&unrelated));
            // Lost successful IPC reply: absence is safe and performs no additional deletion.
            assert!(
                !setup
                    .reset_mismatched_model(current, &vault)
                    .unwrap()
                    .associated
            );
            assert_eq!(vault.counts(), (1, 0, 1));
            drop(client);
            worker.join().unwrap().assert_empty();
        }
    }
}

#[test]
fn reset_refuses_current_nil_malformed_and_locked_records_without_mutation() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_ref().unwrap();
    let vault = FakeVault::default();
    setup.associate_secret(install, SECRET, &vault).unwrap();
    let bytes = fs::read(root.join("model-v1")).unwrap();
    assert!(setup.reset_mismatched_model(install, &vault).is_err());
    assert!(setup.reset_mismatched_model(Uuid::nil(), &vault).is_err());
    assert_eq!(fs::read(root.join("model-v1")).unwrap(), bytes);
    let owner = model_owner(root).unwrap();
    assert!(setup
        .reset_mismatched_model(Uuid::new_v4(), &vault)
        .is_err());
    drop(owner);
    fs::write(root.join("model-v1"), b"{malformed}").unwrap();
    assert!(setup
        .reset_mismatched_model(Uuid::new_v4(), &vault)
        .is_err());
    assert_eq!(fs::read(root.join("model-v1")).unwrap(), b"{malformed}");
    assert_eq!(vault.counts(), (1, 0, 0));
}

#[test]
fn malformed_connection_identity_never_offers_mismatch_reset_or_touches_credentials() {
    for case in 0..3 {
        let (_tmp, setup, old) = selected_setup();
        let root = setup.root.as_ref().unwrap();
        let vault = FakeVault::default();
        setup.associate_secret(old, SECRET, &vault).unwrap();
        let mut b = read_binding(root).unwrap().unwrap();
        b.phase = Phase::Connected;
        b.nonce = Some(Uuid::new_v4());
        let mut connection = ModelConnection {
            id: Uuid::new_v4(),
            generation: Uuid::new_v4(),
            selection: selection(),
        };
        match case {
            0 => connection.id = Uuid::nil(),
            1 => connection.generation = Uuid::nil(),
            _ => connection.selection.model.clear(),
        }
        b.connection = Some(connection);
        write_binding(root, &b).unwrap();
        let current = Uuid::new_v4();
        let (mut client, worker) = peer(Uuid::new_v4(), |_| {
            panic!("malformed binding contacted guest")
        });
        let error = setup
            .connect_stored_model(current, &mut client, &vault, || panic!("captured"))
            .unwrap_err();
        assert_eq!(error.kind, "modelUnavailable");
        assert!(error.recovery.is_none());
        let error = setup
            .sign_out_model(current, Some(&mut client), &vault)
            .unwrap_err();
        assert!(error.recovery.is_none());
        assert!(setup.reset_mismatched_model(current, &vault).is_err());
        assert_eq!(vault.counts(), (1, 0, 0));
        drop(client);
        worker.join().unwrap().assert_empty();
    }
}

#[test]
fn reset_delete_failure_retains_exact_record_then_retries_same_slot() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_ref().unwrap();
    let vault = FakeVault::default();
    setup.associate_secret(install, SECRET, &vault).unwrap();
    let before = fs::read(root.join("model-v1")).unwrap();
    *vault.deny_delete.lock().unwrap() = true;
    let current = Uuid::new_v4();
    let error = setup.reset_mismatched_model(current, &vault).unwrap_err();
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(
        wire["recovery"]["resetStage"],
        "credentialRemovalUnconfirmed"
    );
    assert_eq!(fs::read(root.join("model-v1")).unwrap(), before);
    *vault.deny_delete.lock().unwrap() = false;
    setup.reset_mismatched_model(current, &vault).unwrap();
    assert_eq!(vault.counts(), (1, 0, 2));
}

#[test]
fn reset_record_removal_failure_reports_deleted_credential_and_can_retry_absent_slot() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_ref().unwrap().clone();
    let blocked = root.clone();
    let vault = FakeVault {
        observe_delete: Some(Box::new(move |_| permissions(&blocked, 0o500))),
        ..FakeVault::default()
    };
    setup.associate_secret(install, SECRET, &vault).unwrap();
    let before = fs::read(root.join("model-v1")).unwrap();
    let current = Uuid::new_v4();
    let result = setup.reset_mismatched_model(current, &vault);
    permissions(&root, 0o700);
    let error = result.unwrap_err();
    assert_eq!(
        serde_json::to_value(error).unwrap()["recovery"]["resetStage"],
        "recordRemovalUnconfirmed"
    );
    assert_eq!(fs::read(root.join("model-v1")).unwrap(), before);
    assert!(vault.slots.lock().unwrap().is_empty());
    // The second delete sees an absent slot and must remain idempotent.
    let retry = FakeVault::default();
    setup.reset_mismatched_model(current, &retry).unwrap();
    assert_eq!(retry.counts(), (0, 0, 1));
}
