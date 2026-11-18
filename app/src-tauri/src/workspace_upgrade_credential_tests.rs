use super::*;
use crate::runtime::credential_store::{CredentialStore, StoreError};
use bridge_runtime_protocol::CredentialPayload;
use std::sync::Mutex;
struct Vault(Mutex<Vec<Uuid>>);
impl CredentialStore for Vault {
    fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
        panic!("disk operation saved a secret")
    }
    fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
        panic!("disk operation loaded a secret")
    }
    fn delete(&self, slot: Uuid) -> Result<(), StoreError> {
        self.0.lock().unwrap().push(slot);
        Ok(())
    }
}
pub(super) fn phase(s: &Scene, phase: &str) -> (Uuid, Vec<u8>) {
    let path = s.data.join("setup-v1/model-v1");
    let mut b: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let slot = serde_json::from_value(b["slot"].clone()).unwrap();
    b["phase"] = phase.into();
    if matches!(phase, "pending" | "connected" | "cleared") {
        b["nonce"] = Uuid::new_v4().to_string().into();
        b["connection"] = serde_json::json!({"id":Uuid::new_v4(),"generation":Uuid::new_v4(),"selection":{"provider":"copilot","model":"gpt-4.1"}});
    }
    let bytes = serde_json::to_vec(&b).unwrap();
    fs::write(path, &bytes).unwrap();
    (slot, bytes)
}
#[test]
fn every_phase_and_interrupted_switch_refuses_ordinary_deletion_and_reservation() {
    for p in ["storing", "stored", "pending", "connected", "cleared"] {
        for boundary in [
            "intent",
            "model-archived",
            "destination-recorded",
            "image-prepared",
        ] {
            let mut s = Scene::new(ImageTarget::WindowsAmd64);
            let (slot, _) = phase(&s, p);
            assert!(s.replace(Some(boundary)).is_err());
            let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
            setup.select_local().unwrap();
            let vault = Vault(Mutex::new(vec![]));
            let token = crate::runtime::credentials::CapturedGithubToken::try_from(
                b"synthetic-model-only".as_slice(),
            )
            .unwrap();
            for id in [s.source_id, Uuid::new_v4(), Uuid::new_v4()] {
                assert!(
                    setup.reset_mismatched_model(id, &vault).is_err(),
                    "{p}/{boundary}"
                );
                assert!(
                    setup.sign_out_model(id, None, &vault).is_err(),
                    "{p}/{boundary}"
                );
                assert!(
                    setup.associate_captured(id, &token, &vault).is_err(),
                    "{p}/{boundary}"
                );
            }
            assert!(vault.0.lock().unwrap().is_empty(), "{slot}");
        }
    }
}
#[test]
fn copilot_reuse_after_transfer_and_restore_reaches_guest_capability_before_loading() {
    use bridge_runtime_protocol::*;
    struct Wire(std::io::Cursor<Vec<u8>>);
    impl Read for Wire {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }
    impl Write for Wire {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for restored in [false, true] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        s.replace(None).unwrap();
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        let intent = owner.read_intent().unwrap().unwrap();
        owner.transfer_credential(intent.transaction).unwrap();
        let installation = if restored {
            owner.restore(intent.transaction).unwrap();
            s.source_id
        } else {
            intent.destination.unwrap().installation
        };
        drop(owner);
        let before = fs::read(s.data.join("setup-v1/model-v1")).unwrap();
        let nonce = Uuid::new_v4();
        let reply = encode(&Frame::Result(WireResult {
            version: VERSION,
            boot_nonce: nonce,
            request_id: 3,
            body: ResultBody::Error {
                code: ErrorCode::Unavailable,
            },
        }))
        .unwrap();
        let mut client = crate::runtime::client::VmClient::established(
            Box::new(Wire(std::io::Cursor::new(reply))),
            nonce,
        )
        .unwrap();
        let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
        setup.select_local().unwrap();
        let error = setup
            .connect_stored_model(
                installation,
                &mut client,
                &Vault(Mutex::new(vec![])),
                || panic!("no recapture"),
            )
            .unwrap_err();
        assert_eq!(
            serde_json::to_value(error).unwrap()["kind"],
            "modelGuest",
            "restored={restored}"
        );
        assert_eq!(fs::read(s.data.join("setup-v1/model-v1")).unwrap(), before);
    }
}
#[test]
fn completed_removal_ends_partial_transfer_constraints_for_a_new_association() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    let slot = owner.slot(&intent).unwrap().unwrap();
    assert!(owner
        .transfer_credential_with(intent.transaction, |p| if p == "model-v1" {
            Err(RuntimeError::Closed)
        } else {
            Ok(())
        })
        .is_err());
    owner
        .remove_credential(intent.transaction, slot, &Vault(Mutex::new(vec![])))
        .unwrap();
    let (new_model, _) =
        super::restore::destination_metadata(&s, intent.destination.unwrap().installation);
    assert!(owner.recover().is_ok());
    owner
        .remove_credential(intent.transaction, slot, &Vault(Mutex::new(vec![])))
        .unwrap();
    assert_eq!(
        fs::read(s.data.join("setup-v1/model-v1")).unwrap(),
        new_model
    );
    owner.restore(intent.transaction).unwrap();
    assert!(!s.data.join("setup-v1/model-v1").exists());
}
#[test]
fn pending_removal_of_a_distinct_archived_destination_slot_blocks_reuse() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    let source = owner.slot(&intent).unwrap().unwrap();
    owner
        .remove_credential(intent.transaction, source, &Vault(Mutex::new(vec![])))
        .unwrap();
    let (model, _) =
        super::restore::destination_metadata(&s, intent.destination.unwrap().installation);
    let destination = SetupStore::upgrade_slot(Some(&model)).unwrap().unwrap();
    owner.restore(intent.transaction).unwrap();
    owner.write_slot_outcome(destination, b"removing").unwrap();
    assert!(owner.guard_model(Some(destination), false).is_err());
    assert!(
        owner.guard_model(None, true).is_err(),
        "pending archived removal still reserves a credential"
    );
}
#[test]
fn definitive_failed_key_connect_cannot_auto_delete_a_retained_shared_slot() {
    use bridge_runtime_protocol::*;
    struct Wire(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for Wire {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }
    impl std::io::Write for Wire {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    struct KeyVault;
    impl CredentialStore for KeyVault {
        fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
            panic!("no recapture")
        }
        fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
            Ok(CredentialPayload::new(b"sk-synthetic-only").unwrap())
        }
        fn delete(&self, _: Uuid) -> Result<(), StoreError> {
            panic!("automatic deletion bypassed retained-slot guard")
        }
    }
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let path = s.data.join("setup-v1/model-v1");
    let mut saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    saved["kind"] = "key".into();
    saved["vendor"] = "openai".into();
    fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    fs::remove_file(s.data.join("setup-v1/model-account-v1")).unwrap();
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    owner.transfer_credential(intent.transaction).unwrap();
    let id = intent.destination.unwrap().installation;
    drop(owner);
    let nonce = Uuid::new_v4();
    let generation = Uuid::new_v4();
    let selection = ModelSelection {
        vendor: Some(ModelVendor::Openai),
        provider: "bridge-model".into(),
        model: "gpt-4.1".into(),
    };
    let credential = |generation, installed| CredentialState {
        profile: 2,
        generation,
        installed,
    };
    let replies = [
        ResultBody::ModelState(ModelState {
            retained_cleanup: true,
            phase: ModelPhase::Idle,
            connection: None,
            credential: credential(None, false),
            diagnostic: None,
        }),
        ResultBody::CredentialState(credential(None, false)),
        ResultBody::CredentialState(credential(Some(generation), true)),
        ResultBody::ModelState(ModelState {
            retained_cleanup: true,
            phase: ModelPhase::Unknown,
            connection: Some(ModelConnection {
                id: Uuid::new_v4(),
                generation,
                selection: selection.clone(),
            }),
            credential: credential(Some(generation), true),
            diagnostic: Some(ModelDiagnostic {
                classification: ModelDiagnosticClass::Exit,
                elapsed_ms: 1,
                exit_code: Some(1),
                stage: Some(ModelStage::Model),
                error_code: Some(ModelErrorCode::CredentialRejected),
                stderr: None,
            }),
        }),
    ];
    let bytes = replies
        .into_iter()
        .enumerate()
        .flat_map(|(n, body)| {
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: nonce,
                request_id: n as u64 + 3,
                body,
            }))
            .unwrap()
        })
        .collect();
    let mut client = crate::runtime::client::VmClient::established(
        Box::new(Wire(std::io::Cursor::new(bytes))),
        nonce,
    )
    .unwrap();
    let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
    setup.select_local().unwrap();
    let error = setup
        .connect_model(id, selection, &mut client, &KeyVault)
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(error).unwrap()["kind"],
        "modelKeyCleanup"
    );
    assert!(path.exists());
}
#[test]
fn absent_and_invalid_metadata_are_distinguished_before_disk_mutation() {
    for case in ["absent", "malformed", "unknown", "wrong-slot", "unowned"] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        let path = s.data.join("setup-v1/model-v1");
        match case {
            "absent" => {
                fs::remove_file(&path).unwrap();
                fs::remove_file(s.data.join("setup-v1/model-account-v1")).unwrap();
            }
            "malformed" => fs::write(&path, b"not-json").unwrap(),
            "unknown" => fs::write(&path, b"{\"schema\":2}").unwrap(),
            "wrong-slot" => fs::write(
                s.data.join("setup-v1/model-account-v1"),
                serde_json::to_vec(&serde_json::json!({"slot":Uuid::new_v4(),"login":"stand-in"}))
                    .unwrap(),
            )
            .unwrap(),
            "unowned" => {
                let mut b: serde_json::Value =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                b["installation"] = Uuid::new_v4().to_string().into();
                fs::write(&path, serde_json::to_vec(&b).unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        let before = installation(&s.local.join("local-vm-v1")).unwrap();
        if case == "absent" {
            s.replace(None).unwrap();
            let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
            let id = owner.read_intent().unwrap().unwrap().transaction;
            assert!(!owner.transfer_credential(id).unwrap());
        } else {
            assert!(s.replace(None).is_err(), "{case}");
            assert_eq!(
                installation(&s.local.join("local-vm-v1")).unwrap(),
                before,
                "{case}"
            );
        }
    }
}
#[test]
fn failed_deletion_retains_the_slot_and_blocks_replacement_until_confirmed_retry() {
    struct Failed;
    impl CredentialStore for Failed {
        fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
            panic!("no save")
        }
        fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
            panic!("no load")
        }
        fn delete(&self, _: Uuid) -> Result<(), StoreError> {
            Err(StoreError::Unavailable)
        }
    }
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let (slot, _) = phase(&s, "pending");
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let id = owner.read_intent().unwrap().unwrap().transaction;
    assert!(matches!(
        owner.remove_credential(id, slot, &Failed),
        Err(RuntimeError::Unavailable)
    ));
    assert_eq!(owner.slot_outcome(slot).unwrap(), Some(false));
    drop(owner);
    let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
    setup.select_local().unwrap();
    let token = crate::runtime::credentials::CapturedGithubToken::try_from(
        b"synthetic-model-only".as_slice(),
    )
    .unwrap();
    assert!(setup
        .associate_captured(Uuid::new_v4(), &token, &Failed)
        .is_err());
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    owner
        .remove_credential(id, slot, &Vault(Mutex::new(vec![])))
        .unwrap();
    assert!(owner.slot_removed(slot).unwrap());
}
#[test]
fn transfer_keeps_only_eligible_slot_and_account_without_old_guest_authority() {
    for p in ["storing", "stored", "pending", "connected", "cleared"] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        let (slot, bytes) = phase(&s, p);
        s.replace(None).unwrap();
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        let id = owner.read_intent().unwrap().unwrap().transaction;
        let eligible = matches!(p, "stored" | "connected");
        assert_eq!(owner.transfer_credential(id).unwrap(), eligible);
        let intent = owner.read_intent().unwrap().unwrap();
        assert_eq!(
            intent.model.as_ref().unwrap(),
            &bytes,
            "source snapshot immutable"
        );
        if eligible {
            let b: serde_json::Value =
                serde_json::from_slice(&fs::read(s.data.join("setup-v1/model-v1")).unwrap())
                    .unwrap();
            assert_eq!(b["slot"], slot.to_string());
            assert_eq!(
                b["installation"],
                intent.destination.unwrap().installation.to_string()
            );
            assert_eq!(b["phase"], "stored");
            assert!(b["nonce"].is_null() && b["connection"].is_null());
            assert!(s.data.join("setup-v1/model-account-v1").exists());
        } else {
            assert!(!s.data.join("setup-v1/model-v1").exists());
        }
        assert_eq!(owner.transfer_credential(id).unwrap(), eligible);
    }
}
#[cfg(any(
    all(target_os = "macos", target_arch = "aarch64"),
    all(windows, target_arch = "x86_64")
))]
#[test]
#[ignore = "explicit native synthetic-slot verification; CI opts in"]
fn native_store_all_phases_keep_guarded_slots_until_explicit_exact_removal() {
    use crate::runtime::credential_store::OsCredentialStore;
    struct NativeSlot(Uuid);
    impl Drop for NativeSlot {
        fn drop(&mut self) {
            let _ = OsCredentialStore.delete(self.0);
        }
    }
    struct NativeRemoval;
    impl CredentialStore for NativeRemoval {
        fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
            panic!("guard saved a credential")
        }
        fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
            panic!("guard loaded a credential")
        }
        fn delete(&self, slot: Uuid) -> Result<(), StoreError> {
            OsCredentialStore.delete(slot)
        }
    }
    for p in ["storing", "stored", "pending", "connected", "cleared"] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        let (slot, _) = phase(&s, p);
        let _owned = NativeSlot(slot);
        OsCredentialStore
            .save(slot, b"synthetic-model-only")
            .unwrap();
        s.replace(None).unwrap();
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        let transaction = owner.read_intent().unwrap().unwrap().transaction;
        owner.transfer_credential(transaction).unwrap();
        drop(owner);
        let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
        setup.select_local().unwrap();
        assert!(setup
            .reset_mismatched_model(Uuid::new_v4(), &NativeRemoval)
            .is_err());
        assert!(setup
            .sign_out_model(s.source_id, None, &NativeRemoval)
            .is_err());
        assert_eq!(
            OsCredentialStore.load(slot).unwrap().as_bytes(),
            b"synthetic-model-only"
        );
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        owner
            .remove_credential(transaction, slot, &NativeRemoval)
            .unwrap();
        // Both stores confirm absence through their idempotent native delete.
        OsCredentialStore.delete(slot).unwrap();
        assert!(OsCredentialStore.load(slot).is_err());
        owner.restore(transaction).unwrap();
        assert!(!s.data.join("setup-v1/model-v1").exists());
    }
}
#[test]
fn partial_transfer_and_removal_boundaries_recover_without_restoring_a_removed_association() {
    for boundary in [
        "transfer-intent",
        "model-v1",
        "model-account-v1",
        "transferred",
    ] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        phase(&s, "connected");
        s.replace(None).unwrap();
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        let transaction = owner.read_intent().unwrap().unwrap().transaction;
        assert!(owner
            .transfer_credential_with(transaction, |p| if p == boundary {
                Err(RuntimeError::Closed)
            } else {
                Ok(())
            })
            .is_err());
        drop(owner);
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        owner.recover().unwrap();
        assert!(owner.transfer_credential(transaction).unwrap());
    }
    for boundary in ["vault-deleted", "removal-recorded", "metadata-removed"] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        let (slot, _) = phase(&s, "connected");
        s.replace(None).unwrap();
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        let transaction = owner.read_intent().unwrap().unwrap().transaction;
        owner.transfer_credential(transaction).unwrap();
        let vault = Vault(Mutex::new(vec![]));
        assert!(owner
            .remove_credential_with(transaction, slot, &vault, |p| if p == boundary {
                Err(RuntimeError::Closed)
            } else {
                Ok(())
            })
            .is_err());
        drop(owner);
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        owner.restore(transaction).unwrap();
        assert!(!s.data.join("setup-v1/model-v1").exists(), "{boundary}");
        owner.remove_credential(transaction, slot, &vault).unwrap();
    }
}
#[test]
fn intent_recorded_removal_also_revokes_replayed_canonical_association() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let (slot, _) = phase(&s, "stored");
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let mut intent = owner.read_intent().unwrap().unwrap();
    owner.transfer_credential(intent.transaction).unwrap();
    intent = owner.read_intent().unwrap().unwrap();
    // Existing intent-v1 removal disposition is authoritative even without a newer slot marker.
    intent.credential = CredentialDisposition::Removed;
    owner.persist(&intent).unwrap();
    assert!(owner.guard_model(Some(slot), false).is_err());
    assert!(owner.transfer_credential(intent.transaction).is_err());
}
#[test]
fn completed_history_keeps_its_slot_guard_and_observes_later_removal() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let (slot, _) = phase(&s, "stored");
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let id = owner.read_intent().unwrap().unwrap().transaction;
    owner.transfer_credential(id).unwrap();
    owner.restore(id).unwrap();
    // Same exact completed copy used when a later fresh transaction replaces the marker.
    let bytes = fs::read(s.local.join(STORE).join("intent.json")).unwrap();
    fs::write(
        s.local.join(STORE).join(format!("completed-{id}.json")),
        &bytes,
    )
    .unwrap();
    fs::remove_file(s.local.join(STORE).join("intent.json")).unwrap();
    drop(owner);
    let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
    setup.select_local().unwrap();
    let vault = Vault(Mutex::new(vec![]));
    assert!(setup
        .reset_mismatched_model(Uuid::new_v4(), &vault)
        .is_err());
    assert!(vault.0.lock().unwrap().is_empty());
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    owner.remove_credential(id, slot, &vault).unwrap();
    // Replayed old metadata is not authority to load the acknowledged-removed slot.
    let archived: Intent = serde_json::from_slice(&bytes).unwrap();
    fs::write(s.data.join("setup-v1/model-v1"), archived.model.unwrap()).unwrap();
    assert!(owner.guard_model(Some(slot), false).is_err());
    owner.recover().unwrap();
    assert!(!s.data.join("setup-v1/model-v1").exists());
}
#[test]
fn shared_slot_blocks_reset_signout_and_new_reservations_then_exact_removal_survives_restore() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let (slot, _) = phase(&s, "connected");
    s.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let id = owner.read_intent().unwrap().unwrap().transaction;
    owner.transfer_credential(id).unwrap();
    let b = owner
        .read_intent()
        .unwrap()
        .unwrap()
        .destination
        .unwrap()
        .installation;
    drop(owner);
    let setup = SetupStore::with_upgrade_root(Some(s.data.clone()), Some(s.local.clone()));
    setup.select_local().unwrap();
    let vault = Vault(Mutex::new(vec![]));
    for wrong in [s.source_id, Uuid::new_v4(), Uuid::new_v4()] {
        assert!(setup.reset_mismatched_model(wrong, &vault).is_err());
    }
    assert!(setup.sign_out_model(b, None, &vault).is_err());
    assert!(vault.0.lock().unwrap().is_empty());
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    assert!(owner.remove_credential(id, Uuid::new_v4(), &vault).is_err());
    owner.remove_credential(id, slot, &vault).unwrap();
    assert_eq!(*vault.0.lock().unwrap(), vec![slot]);
    owner.remove_credential(id, slot, &vault).unwrap();
    assert_eq!(
        *vault.0.lock().unwrap(),
        vec![slot],
        "durable success is not replayed"
    );
    owner.restore(id).unwrap();
    assert!(!s.data.join("setup-v1/model-v1").exists());
    assert!(!s.data.join("setup-v1/model-account-v1").exists());
}
