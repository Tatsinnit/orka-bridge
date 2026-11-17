//! Synthetic setup scenarios over the real established Unix control channel.
#![cfg(unix)]

use super::*;
use crate::runtime::credential_store::{NativeCall, StoreError};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    thread::{self, JoinHandle},
    time::Duration,
};

const SECRET: &[u8] = b"synthetic-model-only";

#[path = "setup_model_account_tests.rs"]
mod account_tests;
#[path = "setup_model_key_tests.rs"]
mod key_tests;
#[path = "setup_model_reset_tests.rs"]
mod reset_tests;

fn permissions(root: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(root, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn root_fd_lock_excludes_all_mutations_and_releases_after_return() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_deref().unwrap();
    let owner = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("model-owner.lock"))
        .unwrap();
    owner.try_lock().unwrap();
    let vault = FakeVault::default();
    assert!(setup.associate_secret(install, SECRET, &vault).is_err());
    assert_eq!(vault.counts(), (0, 0, 0));
    drop(owner);
    setup.associate_secret(install, SECRET, &vault).unwrap();
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("model-owner.lock"))
        .unwrap();
    owner.try_lock().unwrap();
    let (mut client, worker) = peer(Uuid::new_v4(), |_| Some(idle()));
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    assert!(setup
        .sign_out_model(install, Some(&mut client), &vault)
        .is_err());
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(vault.counts(), (1, 0, 0));
    drop(owner);
    setup.sign_out_model(install, None, &vault).unwrap();
    setup.associate_secret(install, SECRET, &vault).unwrap();
}

#[test]
fn independent_setup_instance_cannot_enter_live_save_transaction() {
    use std::sync::Barrier;
    let (tmp, setup, install) = selected_setup();
    let entered = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let (entered_save, resume_save) = (entered.clone(), resume.clone());
    let vault = FakeVault {
        observe_save: Some(Box::new(move |_| {
            entered_save.wait();
            resume_save.wait();
        })),
        ..FakeVault::default()
    };
    let worker = thread::spawn(move || setup.associate_secret(install, SECRET, &vault));
    entered.wait();
    let second = SetupStore::new(Some(tmp.path().to_owned()));
    let second_vault = FakeVault::default();
    let association = second.associate_secret(install, SECRET, &second_vault);
    let (mut client, peer_worker) = peer(Uuid::new_v4(), |_| Some(idle()));
    let connect = second.connect_model(install, selection(), &mut client, &second_vault);
    let signout = second.sign_out_model(install, Some(&mut client), &second_vault);
    resume.wait();
    assert!(worker.join().unwrap().is_ok());
    assert!(association.is_err() && connect.is_err() && signout.is_err());
    drop(client);
    peer_worker.join().unwrap().assert_empty();
    assert_eq!(second_vault.counts(), (0, 0, 0));
    second.sign_out_model(install, None, &second_vault).unwrap();
}

#[test]
fn exact_slot_reservation_is_visible_before_os_save() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_ref().unwrap().clone();
    let vault = FakeVault {
        observe_save: Some(Box::new(move |slot| {
            let raw = fs::read(root.join("model-v1")).expect("reservation before OS mutation");
            let binding: serde_json::Value = serde_json::from_slice(&raw).unwrap();
            assert_eq!(binding["slot"], slot.to_string());
            assert_eq!(binding["installation"], install.to_string());
            assert_eq!(binding["phase"], "storing");
            assert!(!raw.windows(SECRET.len()).any(|part| part == SECRET));
        })),
        ..FakeVault::default()
    };
    assert!(
        setup
            .associate_secret(install, SECRET, &vault)
            .unwrap()
            .associated
    );
}

#[test]
fn os_save_apply_then_error_keeps_uuid_for_explicit_host_signout() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault {
        apply_then_error: true,
        ..FakeVault::default()
    };
    assert!(setup.associate_secret(install, SECRET, &vault).is_err());
    let binding = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .expect("uncertain save retains exact slot");
    assert!(vault.slots.lock().unwrap().contains_key(&binding.slot));
    let status = setup.model_binding_status().unwrap();
    assert!(!status.associated && !status.ready && status.unknown);
    let (mut client, worker) = peer(Uuid::new_v4(), |_| Some(idle()));
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    assert!(setup.associate_secret(install, SECRET, &vault).is_err());
    setup
        .sign_out_model(install, Some(&mut client), &vault)
        .unwrap();
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(
        *vault.calls.lock().unwrap(),
        vec![
            ("save", binding.slot),
            ("load", binding.slot),
            ("delete", binding.slot)
        ]
    );
    assert!(vault.slots.lock().unwrap().is_empty());
    assert!(read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .is_none());
}

#[test]
fn reservation_publication_failure_precedes_os_save() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_deref().unwrap();
    fs::write(root.join("model-owner.lock"), b"").unwrap();
    permissions(root, 0o500);
    let vault = FakeVault::default();
    let result = setup.associate_secret(install, SECRET, &vault);
    permissions(root, 0o700);
    assert!(result.is_err());
    assert_eq!(vault.counts(), (0, 0, 0));
    assert!(read_binding(root).unwrap().is_none());
    setup.associate_secret(install, SECRET, &vault).unwrap();
}

#[test]
fn visible_publication_with_directory_sync_error_never_deletes_os_slot() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_deref().unwrap();
    let vault = FakeVault::default();
    *vault.block_publish.lock().unwrap() = Some((root.to_owned(), 0o300));
    let result = setup.associate_secret(install, SECRET, &vault);
    permissions(root, 0o700);
    assert!(result.is_err());
    let binding = read_binding(root).unwrap().unwrap();
    assert_eq!(binding.phase, Phase::Stored);
    assert!(vault.slots.lock().unwrap().contains_key(&binding.slot));
    assert_eq!(vault.counts(), (1, 0, 0));
    assert!(!setup.model_binding_status().unwrap().ready);
    setup.sign_out_model(install, None, &vault).unwrap();
    assert!(vault.slots.lock().unwrap().is_empty());
    assert!(!fs::read_dir(root).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("model-next-")));
}

#[test]
fn invalid_selection_refuses_before_actual_wire_or_os_load() {
    let (_tmp, setup, install, vault) = associated();
    let (mut client, worker) = peer(Uuid::new_v4(), |_| Some(idle()));
    for (provider, model) in [
        ("p".repeat(46), "valid".into()),
        ("copilot".into(), "auto".into()),
        ("copilot".into(), "has space".into()),
        ("copilot".into(), "x".repeat(257)),
    ] {
        assert!(setup
            .connect_model(
                install,
                ModelSelection {
                    vendor: None,
                    provider,
                    model
                },
                &mut client,
                &vault
            )
            .is_err());
    }
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(vault.counts(), (1, 0, 0));
}

#[derive(Default)]
struct FakeVault {
    deny_save: bool,
    secret: Option<&'static [u8]>,
    save_error: Option<StoreError>,
    load_error: Option<StoreError>,
    deny_delete: Mutex<bool>,
    block_publish: Mutex<Option<(std::path::PathBuf, u32)>>,
    observe_save: Option<Box<dyn Fn(Uuid) + Send + Sync>>,
    observe_delete: Option<Box<dyn Fn(Uuid) + Send + Sync>>,
    apply_then_error: bool,
    calls: Mutex<Vec<(&'static str, Uuid)>>,
    slots: Mutex<HashMap<Uuid, Vec<u8>>>,
}
impl FakeVault {
    fn counts(&self) -> (usize, usize, usize) {
        let calls = self.calls.lock().unwrap();
        let count = |name| calls.iter().filter(|(op, _)| *op == name).count();
        (count("save"), count("load"), count("delete"))
    }
}
impl CredentialStore for FakeVault {
    fn save(&self, slot: Uuid, secret: &[u8]) -> Result<(), StoreError> {
        assert!(!slot.is_nil());
        assert!(secret == self.secret.unwrap_or(SECRET));
        self.calls.lock().unwrap().push(("save", slot));
        if let Some(error) = self.save_error {
            return Err(error);
        }
        if self.deny_save {
            return Err(StoreError::Unavailable);
        }
        if let Some(observe) = &self.observe_save {
            observe(slot);
        }
        self.slots.lock().unwrap().insert(slot, secret.to_vec());
        if let Some((path, mode)) = self.block_publish.lock().unwrap().take() {
            permissions(&path, mode);
        }
        if self.apply_then_error {
            return Err(StoreError::Unavailable);
        }
        Ok(())
    }
    fn load(&self, slot: Uuid) -> Result<CredentialPayload, StoreError> {
        self.calls.lock().unwrap().push(("load", slot));
        if let Some(error) = self.load_error {
            return Err(error);
        }
        let slots = self.slots.lock().unwrap();
        CredentialPayload::new(slots.get(&slot).ok_or(StoreError::Native {
            call: NativeCall::CredReadW,
            code: 1168,
        })?)
        .map_err(|_| StoreError::Invalid)
    }
    fn delete(&self, slot: Uuid) -> Result<(), StoreError> {
        self.calls.lock().unwrap().push(("delete", slot));
        if *self.deny_delete.lock().unwrap() {
            return Err(StoreError::Unavailable);
        }
        self.slots.lock().unwrap().remove(&slot);
        if let Some(observe) = &self.observe_delete {
            observe(slot);
        }
        Ok(())
    }
}

struct Traffic {
    frames: Vec<WireRequest>,
    bytes: usize,
}
impl Traffic {
    fn publications(&self) -> usize {
        self.frames
            .iter()
            .filter(|r| matches!(r.body, RequestBody::CredentialPublish { .. }))
            .count()
    }
    fn assert_empty(&self) {
        assert_eq!(self.bytes, 0, "refusal must precede an actual wire write");
        assert!(self.frames.is_empty());
    }
}
// None means the peer received the request but lost its acknowledgement.
fn peer(
    nonce: Uuid,
    mut reply: impl FnMut(&WireRequest) -> Option<ResultBody> + Send + 'static,
) -> (VmClient, JoinHandle<Traffic>) {
    let (host, guest) = UnixStream::pair().unwrap();
    for stream in [&host, &guest] {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
    }
    let worker = thread::spawn(move || {
        let mut io = BufReader::new(guest);
        let mut traffic = Traffic {
            frames: Vec::new(),
            bytes: 0,
        };
        loop {
            let mut raw = Vec::new();
            if io.read_until(b'\n', &mut raw).unwrap() == 0 {
                break;
            }
            traffic.bytes += raw.len();
            let frame: Frame = serde_json::from_slice(&raw).unwrap();
            frame.validate().unwrap();
            let Frame::Request(request) = frame else {
                panic!("expected request")
            };
            let response = reply(&request);
            let id = request.request_id;
            let boot_nonce = request.boot_nonce;
            assert_eq!(boot_nonce, nonce);
            traffic.frames.push(request);
            let Some(body) = response else { break };
            let response = Frame::Result(WireResult {
                version: VERSION,
                boot_nonce,
                request_id: id,
                body,
            });
            response.validate().unwrap();
            io.get_mut().write_all(&encode(&response).unwrap()).unwrap();
        }
        traffic
    });
    (
        VmClient::established(Box::new(host), nonce).unwrap(),
        worker,
    )
}
fn selected_setup() -> (tempfile::TempDir, SetupStore, Uuid) {
    let tmp = tempfile::tempdir().unwrap();
    let setup = SetupStore::new(Some(tmp.path().to_owned()));
    setup.select_local().unwrap();
    (tmp, setup, Uuid::new_v4())
}
fn associated() -> (tempfile::TempDir, SetupStore, Uuid, FakeVault) {
    let (tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    setup.associate_secret(install, SECRET, &vault).unwrap();
    (tmp, setup, install, vault)
}
fn selection() -> ModelSelection {
    ModelSelection {
        vendor: None,
        provider: "copilot".into(),
        model: "gpt-5.6-terra".into(),
    }
}
fn credential(generation: Option<Uuid>, installed: bool) -> CredentialState {
    CredentialState {
        profile: CREDENTIAL_PROFILE,
        generation,
        installed,
    }
}
fn idle() -> ResultBody {
    ResultBody::ModelState(ModelState {
        retained_cleanup: true,
        diagnostic: None,
        phase: ModelPhase::Idle,
        connection: None,
        credential: credential(None, false),
    })
}
fn connection() -> ModelConnection {
    ModelConnection {
        id: Uuid::new_v4(),
        selection: selection(),
        generation: Uuid::new_v4(),
    }
}
fn remote(connection: &ModelConnection, phase: ModelPhase) -> ResultBody {
    ResultBody::ModelState(ModelState {
        retained_cleanup: true,
        diagnostic: None,
        phase,
        connection: Some(connection.clone()),
        credential: credential(Some(connection.generation), true),
    })
}

#[test]
fn denied_save_with_confirmed_absence_leaves_no_reservation_or_plaintext_fallback() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault {
        deny_save: true,
        ..FakeVault::default()
    };
    assert!(setup.associate_secret(install, SECRET, &vault).is_err());
    assert!(!setup.model_binding_status().unwrap().associated);
    let root = setup.root.as_deref().unwrap();
    assert!(!setup.model_binding_status().unwrap().unknown);
    assert!(read_binding(root).unwrap().is_none());
    for entry in fs::read_dir(root).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        assert!(!bytes.windows(SECRET.len()).any(|part| part == SECRET));
    }
    let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("no associated secret"));
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(vault.counts(), (1, 1, 0));
    assert!(vault.slots.lock().unwrap().is_empty());
}

const WRITE_FAILURE: StoreError = StoreError::Native {
    call: NativeCall::CredWriteW,
    code: 8,
};
const READ_FAILURE: StoreError = StoreError::Native {
    call: NativeCall::CredReadW,
    code: 1168,
};
const STORE_MESSAGE: &str = "The native credential store could not save or load the sign-in.";

// Diagnostics are built only from finite typed data, so no slot, target or secret can enter.
fn assert_redacted(error: &SetupError, slot: Uuid, install: Uuid) {
    let wire = serde_json::to_string(error).unwrap();
    let text = format!("{wire} {error:?}");
    for private in [
        std::str::from_utf8(SECRET).unwrap().to_owned(),
        slot.to_string(),
        install.to_string(),
        "ai.orka.bridge".to_owned(),
    ] {
        assert!(!text.contains(&private), "diagnostic leaked {private}");
    }
    let value: serde_json::Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(value["kind"], "modelStore");
    assert_eq!(value["message"], STORE_MESSAGE);
}

#[test]
fn failed_save_with_confirmed_absence_reports_error_8_and_allows_another_signin() {
    let (_tmp, setup, install) = selected_setup();
    let mut vault = FakeVault {
        save_error: Some(WRITE_FAILURE),
        ..FakeVault::default()
    };
    let error = setup.associate_secret(install, SECRET, &vault).unwrap_err();
    assert_eq!(error.kind, "modelStore");
    assert_eq!(
        serde_json::to_value(&error).unwrap()["native"],
        serde_json::json!({"call": "CredWriteW", "code": 8})
    );
    let slot = vault.calls.lock().unwrap()[0].1;
    assert_redacted(&error, slot, install);
    assert!(read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .is_none());
    assert!(!setup.model_binding_status().unwrap().unknown);
    assert_eq!(vault.counts(), (1, 1, 0));
    vault.save_error = None;
    assert!(
        setup
            .associate_secret(install, SECRET, &vault)
            .unwrap()
            .associated
    );
    let binding = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(binding.phase, Phase::Stored);
    assert_ne!(binding.slot, slot);
    assert_eq!(
        vault.counts(),
        (2, 1, 0),
        "only the explicit retry saves again"
    );
}

#[test]
fn native_load_failure_is_diagnosed_before_any_guest_mutation() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault {
        load_error: Some(READ_FAILURE),
        ..FakeVault::default()
    };
    setup.associate_secret(install, SECRET, &vault).unwrap();
    let (mut client, worker) = peer(Uuid::new_v4(), |_| Some(idle()));
    let error = setup
        .connect_model(install, selection(), &mut client, &vault)
        .unwrap_err();
    assert_eq!(error.kind, "modelStore");
    assert_eq!(
        serde_json::to_value(&error).unwrap()["native"],
        serde_json::json!({"call": "CredReadW", "code": 1168})
    );
    let binding = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(binding.phase, Phase::Stored);
    assert_redacted(&error, binding.slot, install);
    drop(client);
    assert_eq!(worker.join().unwrap().publications(), 0);
    assert_eq!(vault.counts(), (1, 1, 0));
}

#[test]
fn non_native_store_failures_and_other_errors_have_only_their_fixed_message() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault {
        deny_save: true,
        ..FakeVault::default()
    };
    let error = setup.associate_secret(install, SECRET, &vault).unwrap_err();
    assert_eq!(error.kind, "modelStore");
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        serde_json::json!({"kind": "modelStore", "message": STORE_MESSAGE})
    );
}

#[test]
fn wrong_installation_never_writes_to_guest_or_loads_owned_slot() {
    let (_tmp, setup, install, vault) = associated();
    let slot = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap()
        .slot;
    let wrong = Uuid::new_v4();
    assert_ne!(wrong, install);
    let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("wrong installation"));
    assert!(setup
        .connect_model(wrong, selection(), &mut client, &vault)
        .is_err());
    assert!(setup
        .sign_out_model(wrong, Some(&mut client), &vault)
        .is_err());
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(vault.counts(), (1, 0, 0));
    assert!(vault.slots.lock().unwrap().contains_key(&slot));
    assert_eq!(
        read_binding(setup.root.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .phase,
        Phase::Stored
    );
}

#[test]
fn older_peer_framed_unavailable_precedes_secret_load_and_delivery() {
    let (_tmp, setup, install, vault) = associated();
    let (mut client, worker) = peer(Uuid::new_v4(), |request| {
        assert!(matches!(request.body, RequestBody::ModelStatus {}));
        Some(ResultBody::Error {
            code: ErrorCode::Unavailable,
        })
    });
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    drop(client);
    let traffic = worker.join().unwrap();
    assert_eq!(traffic.frames.len(), 1);
    assert!(traffic.bytes > 0);
    assert_eq!(traffic.publications(), 0);
    assert_eq!(vault.counts(), (1, 0, 0));
    assert_eq!(
        read_binding(setup.root.as_deref().unwrap())
            .unwrap()
            .unwrap()
            .phase,
        Phase::Stored
    );
}

#[test]
fn failed_connect_retains_only_typed_guest_diagnostics_in_setup_details() {
    let (_tmp, setup, install, vault) = associated();
    let expected = connection();
    let diagnostic = ModelDiagnostic {
        classification: ModelDiagnosticClass::Exit,
        elapsed_ms: 9980,
        exit_code: Some(1),
        stage: Some(ModelStage::Model),
        error_code: Some(ModelErrorCode::ExecutionUnknown),
        stderr: Some(ModelStderr::ExecutionUnknown),
    };
    let failure = diagnostic.clone();
    let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
        Some(match &request.body {
            RequestBody::ModelStatus {} => idle(),
            RequestBody::CredentialStatus {} => {
                ResultBody::CredentialState(credential(None, false))
            }
            RequestBody::CredentialPublish { .. } => {
                ResultBody::CredentialState(credential(Some(expected.generation), true))
            }
            RequestBody::ModelConnect { .. } => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                phase: ModelPhase::Unknown,
                connection: Some(expected.clone()),
                credential: credential(Some(expected.generation), true),
                diagnostic: Some(failure.clone()),
            }),
            _ => panic!("unexpected request"),
        })
    });
    let error = setup
        .connect_model(install, selection(), &mut client, &vault)
        .unwrap_err();
    drop(client);
    worker.join().unwrap();
    let detail = error.model_detail().expect("retained diagnostic");
    assert!(detail.contains("execution_unknown"));
    assert!(detail.contains("9980"));
    assert!(detail.contains("The execution outcome is unknown."));
    assert!(!detail.contains(std::str::from_utf8(SECRET).unwrap()));
    assert_eq!(serde_json::to_value(error).unwrap()["kind"], "modelConnect");
}

#[test]
fn failed_connect_keeps_guest_diagnostics_when_binding_persistence_fails() {
    let (tmp, setup, _install, _vault) = associated();
    let mut binding = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap();
    let missing_root = tmp.path().join("missing");
    let conn = connection();
    let remote = ModelState {
        retained_cleanup: true,
        phase: ModelPhase::Unknown,
        credential: credential(Some(conn.generation), true),
        connection: Some(conn),
        diagnostic: Some(ModelDiagnostic {
            classification: ModelDiagnosticClass::Exit,
            elapsed_ms: 12,
            exit_code: Some(1),
            stage: Some(ModelStage::Model),
            error_code: Some(ModelErrorCode::ExecutionUnknown),
            stderr: Some(ModelStderr::ExecutionUnknown),
        }),
    };
    let error = finish_connect(&missing_root, &mut binding, remote).unwrap_err();
    let detail = error
        .model_detail()
        .expect("observed failure evidence survives host I/O failure");
    assert!(detail.contains("execution_unknown"));
    assert!(detail.contains("The execution outcome is unknown."));
    assert!(!missing_root.exists());
}

#[test]
fn pending_ready_ack_is_reconciled_without_republishing_or_loading_sign_in() {
    let (_tmp, setup, install, vault) = associated();
    let nonce = Uuid::new_v4();
    let conn = connection();
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(nonce);
    write_binding(root, &binding).unwrap();
    let expected = conn.clone();
    let (mut client, worker) = peer(nonce, move |request| {
        assert!(matches!(request.body, RequestBody::ModelStatus {}));
        Some(remote(&expected, ModelPhase::Ready))
    });
    let result = setup.connect_model(install, selection(), &mut client, &vault);
    drop(client);
    let traffic = worker.join().unwrap();
    assert!(result.unwrap().ready);
    assert_eq!(traffic.frames.len(), 1);
    assert_eq!(vault.counts(), (1, 0, 0));
    let binding = read_binding(root).unwrap().unwrap();
    assert_eq!(binding.phase, Phase::Connected);
    assert_eq!(binding.connection, Some(conn));
}

#[test]
fn pending_wrong_credential_profile_refuses_before_connect_retry() {
    let (_tmp, setup, install, vault) = associated();
    let nonce = Uuid::new_v4();
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(nonce);
    write_binding(root, &binding).unwrap();
    let (mut client, worker) = peer(nonce, |r| {
        assert!(
            matches!(r.body, RequestBody::ModelStatus {}),
            "wrong profile retried Connect"
        );
        Some(ResultBody::ModelState(ModelState {
            retained_cleanup: true,
            phase: ModelPhase::Unknown,
            connection: None,
            credential: CredentialState {
                profile: MODEL_API_KEY_PROFILE,
                generation: Some(Uuid::new_v4()),
                installed: true,
            },
            diagnostic: None,
        }))
    });
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    drop(client);
    assert_eq!(worker.join().unwrap().frames.len(), 1);
    assert_eq!(vault.counts(), (1, 0, 0));
}

#[test]
fn pending_without_id_retries_only_the_installed_generation_without_capture() {
    let (_tmp, setup, install, vault) = associated();
    let nonce = Uuid::new_v4();
    let conn = connection();
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(nonce);
    write_binding(root, &binding).unwrap();
    let expected = conn.clone();
    let (mut client, worker) = peer(nonce, move |request| {
        Some(match &request.body {
            RequestBody::ModelStatus {} => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                diagnostic: None,
                phase: ModelPhase::Unknown,
                connection: None,
                credential: credential(Some(expected.generation), true),
            }),
            RequestBody::ModelConnect {
                generation,
                selection,
            } => {
                assert_eq!(*generation, expected.generation);
                assert_eq!(*selection, expected.selection);
                remote(&expected, ModelPhase::Ready)
            }
            _ => panic!("retry must not republish a credential"),
        })
    });
    let result =
        setup.connect_stored_model(install, &mut client, &vault, || panic!("no recapture"));
    drop(client);
    let traffic = worker.join().unwrap();
    assert!(result.unwrap().ready);
    assert_eq!(traffic.frames.len(), 2);
    assert_eq!(vault.counts(), (1, 0, 0));
    assert_eq!(read_binding(root).unwrap().unwrap().connection, Some(conn));
}

#[test]
fn pending_known_failure_disconnects_and_clears_before_a_fresh_connect() {
    let (_tmp, setup, install, vault) = associated();
    let nonce = Uuid::new_v4();
    let old = connection();
    let new = connection();
    let cleared = Uuid::new_v4();
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(nonce);
    binding.connection = Some(old.clone());
    write_binding(root, &binding).unwrap();
    let expected = new.clone();
    let mut step = 0;
    let (mut client, worker) = peer(nonce, move |request| {
        step += 1;
        Some(match (step, &request.body) {
            (1, RequestBody::ModelStatus {}) => remote(&old, ModelPhase::Unknown),
            (2, RequestBody::ModelDisconnect { connection }) => {
                assert_eq!(*connection, old.id);
                ResultBody::ModelState(ModelState {
                    retained_cleanup: true,
                    diagnostic: None,
                    phase: ModelPhase::Idle,
                    connection: None,
                    credential: credential(Some(old.generation), true),
                })
            }
            (3, RequestBody::CredentialStatus {}) => {
                ResultBody::CredentialState(credential(Some(old.generation), true))
            }
            (
                4,
                RequestBody::CredentialClear {
                    expected_generation,
                },
            ) => {
                assert_eq!(*expected_generation, old.generation);
                ResultBody::CredentialState(credential(Some(cleared), false))
            }
            (5, RequestBody::ModelStatus {}) => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                diagnostic: None,
                phase: ModelPhase::Idle,
                connection: None,
                credential: credential(Some(cleared), false),
            }),
            (6, RequestBody::CredentialStatus {}) => {
                ResultBody::CredentialState(credential(Some(cleared), false))
            }
            (
                7,
                RequestBody::CredentialPublish {
                    profile: CREDENTIAL_PROFILE,
                    expected_generation,
                    token,
                },
            ) => {
                assert_eq!(*expected_generation, Some(cleared));
                assert_eq!(token.as_bytes(), SECRET);
                ResultBody::CredentialState(credential(Some(expected.generation), true))
            }
            (
                8,
                RequestBody::ModelConnect {
                    generation,
                    selection,
                },
            ) => {
                assert_eq!(*generation, expected.generation);
                assert_eq!(*selection, expected.selection);
                remote(&expected, ModelPhase::Ready)
            }
            _ => panic!("unexpected retry step {step}"),
        })
    });
    let result =
        setup.connect_stored_model(install, &mut client, &vault, || panic!("no recapture"));
    drop(client);
    let traffic = worker.join().unwrap();
    assert!(result.unwrap().ready);
    assert_eq!(traffic.frames.len(), 8);
    assert_eq!(vault.counts(), (1, 1, 0));
    assert_eq!(read_binding(root).unwrap().unwrap().connection, Some(new));
}

#[test]
fn pending_after_acknowledged_clear_reconciles_before_reloading_sign_in() {
    let (_tmp, setup, install, vault) = associated();
    let nonce = Uuid::new_v4();
    let old = connection();
    let new = connection();
    let cleared = Uuid::new_v4();
    let root = setup.root.as_deref().unwrap();
    // Disconnect/Clear succeeded, but publishing Stored failed: these are the
    // exact durable bytes left by that interruption, not permission to reset.
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(nonce);
    binding.connection = Some(old);
    write_binding(root, &binding).unwrap();
    let expected = new.clone();
    let mut step = 0;
    let (mut client, worker) = peer(nonce, move |request| {
        step += 1;
        Some(match (step, &request.body) {
            (1 | 2, RequestBody::ModelStatus {}) => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                diagnostic: None,
                phase: ModelPhase::Idle,
                connection: None,
                credential: credential(Some(cleared), false),
            }),
            (3, RequestBody::CredentialStatus {}) => {
                ResultBody::CredentialState(credential(Some(cleared), false))
            }
            (
                4,
                RequestBody::CredentialPublish {
                    profile: CREDENTIAL_PROFILE,
                    expected_generation,
                    token,
                },
            ) => {
                assert_eq!(*expected_generation, Some(cleared));
                assert_eq!(token.as_bytes(), SECRET);
                ResultBody::CredentialState(credential(Some(expected.generation), true))
            }
            (
                5,
                RequestBody::ModelConnect {
                    generation,
                    selection,
                },
            ) => {
                assert_eq!(*generation, expected.generation);
                assert_eq!(*selection, expected.selection);
                remote(&expected, ModelPhase::Ready)
            }
            _ => panic!("unexpected recovery step {step}"),
        })
    });
    let result =
        setup.connect_stored_model(install, &mut client, &vault, || panic!("no recapture"));
    drop(client);
    assert!(result.unwrap().ready);
    assert_eq!(worker.join().unwrap().frames.len(), 5);
    assert_eq!(vault.counts(), (1, 1, 0));
    assert_eq!(read_binding(root).unwrap().unwrap().connection, Some(new));
}

#[test]
fn pending_known_id_needs_an_advanced_cleared_generation_before_host_recovery() {
    for (advanced, installed) in [(false, false), (true, true)] {
        let (_tmp, setup, install, vault) = associated();
        let nonce = Uuid::new_v4();
        let known = connection();
        let observed = if advanced {
            Uuid::new_v4()
        } else {
            known.generation
        };
        let root = setup.root.as_deref().unwrap();
        let mut binding = read_binding(root).unwrap().unwrap();
        binding.phase = Phase::Pending;
        binding.nonce = Some(nonce);
        binding.connection = Some(known);
        write_binding(root, &binding).unwrap();
        let (mut client, worker) = peer(nonce, move |request| {
            assert!(matches!(request.body, RequestBody::ModelStatus {}));
            Some(ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                diagnostic: None,
                phase: ModelPhase::Idle,
                connection: None,
                credential: credential(Some(observed), installed),
            }))
        });
        assert!(setup
            .connect_model(install, selection(), &mut client, &vault)
            .is_err());
        drop(client);
        assert_eq!(worker.join().unwrap().frames.len(), 1);
        assert_eq!(vault.counts(), (1, 0, 0));
        assert_eq!(read_binding(root).unwrap().unwrap().phase, Phase::Pending);
    }
}

#[test]
fn new_boot_remove_cleans_guest_before_deleting_only_the_owned_slot() {
    let (_tmp, setup, install, vault) = associated();
    let root = setup.root.as_ref().unwrap().clone();
    let mut binding = read_binding(&root).unwrap().unwrap();
    binding.phase = Phase::Connected;
    binding.nonce = Some(Uuid::new_v4());
    binding.connection = Some(connection());
    write_binding(&root, &binding).unwrap();
    let expected = binding.connection.clone().unwrap();
    let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
        Some(match &request.body {
            RequestBody::ModelStatus {} => idle(),
            RequestBody::ModelDisconnectRetained { connection } => {
                assert_eq!(connection, &expected);
                ResultBody::Ready {}
            }
            _ => panic!("removal must not load, publish or clear an absent guest credential"),
        })
    });
    setup
        .sign_out_model(install, Some(&mut client), &vault)
        .unwrap();
    drop(client);
    assert_eq!(worker.join().unwrap().frames.len(), 2);
    assert_eq!(vault.counts(), (1, 0, 1));
    assert!(!setup.model_binding_status().unwrap().associated);
    setup.sign_out_model(install, None, &vault).unwrap();
    assert_eq!(vault.counts(), (1, 0, 1));
}

#[test]
fn older_image_refuses_reconnect_without_poisoning_its_existing_control_channel() {
    let (_tmp, setup, install, vault) = associated();
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Connected;
    binding.nonce = Some(Uuid::new_v4());
    binding.connection = Some(connection());
    write_binding(root, &binding).unwrap();
    let before = fs::read(root.join("model-v1")).unwrap();
    let legacy: ModelState =
        serde_json::from_value(serde_json::json!({"phase":"idle", "connection":null,
        "credential":{"profile":CREDENTIAL_PROFILE,"installed":false,"generation":null}}))
        .unwrap();
    let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
        assert!(
            matches!(request.body, RequestBody::ModelStatus {}),
            "never send a new opcode to an older guest"
        );
        Some(ResultBody::ModelState(legacy.clone()))
    });
    assert_eq!(
        setup
            .connect_stored_model(install, &mut client, &vault, || panic!("no capture"))
            .unwrap_err()
            .kind,
        "modelReconnect"
    );
    assert!(
        client.model_status().is_ok(),
        "existing channel remains usable"
    );
    drop(client);
    assert_eq!(worker.join().unwrap().frames.len(), 2);
    assert_eq!(vault.counts(), (1, 0, 0));
    assert_eq!(fs::read(root.join("model-v1")).unwrap(), before);
}

#[test]
fn unknown_transport_other_is_not_mislabeled_as_a_guest_refusal() {
    let error = std::io::Error::other("synthetic-private-token /private/path");
    assert_eq!(cleanup_error_class(&error), "io");
}
#[test]
fn previous_boot_failures_keep_finite_operation_and_class_without_secrets() {
    for (case, expected) in [
        (0, "modelStatus:unsupported"),
        (1, "modelStatusPreconditions:refused"),
        (2, "disconnectRetainedModel:refused"),
    ] {
        let (_tmp, setup, install, vault) = associated();
        let root = setup.root.as_deref().unwrap();
        let mut binding = read_binding(root).unwrap().unwrap();
        binding.phase = Phase::Connected;
        binding.nonce = Some(Uuid::new_v4());
        binding.connection = Some(connection());
        write_binding(root, &binding).unwrap();
        let before = fs::read(root.join("model-v1")).unwrap();
        let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
            Some(match request.body {
                RequestBody::ModelStatus {} if case == 0 => ResultBody::Error {
                    code: ErrorCode::Unavailable,
                },
                RequestBody::ModelStatus {} if case == 1 => {
                    let ResultBody::ModelState(mut state) = idle() else {
                        unreachable!()
                    };
                    state.retained_cleanup = false;
                    ResultBody::ModelState(state)
                }
                RequestBody::ModelStatus {} => idle(),
                RequestBody::ModelDisconnectRetained { .. } => ResultBody::Error {
                    code: ErrorCode::Unavailable,
                },
                _ => panic!("cleanup failure must not touch credentials"),
            })
        });
        let error = setup
            .sign_out_model(install, Some(&mut client), &vault)
            .unwrap_err();
        assert_eq!(error.kind, "modelReconnect");
        assert_eq!(error.model_detail().as_deref(), Some(expected));
        drop(client);
        worker.join().unwrap();
        assert_eq!(vault.counts(), (1, 0, 0));
        assert_eq!(fs::read(root.join("model-v1")).unwrap(), before);
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains(&binding.slot.to_string()));
    }
}

#[test]
fn new_boot_cleanup_refusal_preserves_slot_and_binding_before_any_secret_load() {
    for reply in [
        idle(),
        ResultBody::Error {
            code: ErrorCode::Unavailable,
        },
        ResultBody::ModelState(ModelState {
            retained_cleanup: true,
            phase: ModelPhase::Idle,
            connection: None,
            credential: credential(Some(Uuid::new_v4()), false),
            diagnostic: None,
        }),
    ] {
        let (_tmp, setup, install, vault) = associated();
        let root = setup.root.as_deref().unwrap();
        let mut binding = read_binding(root).unwrap().unwrap();
        binding.phase = Phase::Connected;
        binding.nonce = Some(Uuid::new_v4());
        binding.connection = Some(connection());
        write_binding(root, &binding).unwrap();
        let before = fs::read(root.join("model-v1")).unwrap();
        let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
            Some(match request.body {
                RequestBody::ModelStatus {} => reply.clone(),
                RequestBody::ModelDisconnectRetained { .. } => ResultBody::Error {
                    code: ErrorCode::Unavailable,
                },
                _ => panic!("no credential operations after cleanup refusal"),
            })
        });
        assert!(setup
            .connect_stored_model(install, &mut client, &vault, || panic!("no capture"))
            .is_err());
        drop(client);
        worker.join().unwrap();
        assert_eq!(vault.counts(), (1, 0, 0));
        assert_eq!(fs::read(root.join("model-v1")).unwrap(), before);
        assert!(vault.slots.lock().unwrap().contains_key(&binding.slot));
    }
}

#[test]
fn new_boot_reconnect_cleans_retained_connection_before_loading_saved_sign_in() {
    for phase in [Phase::Connected, Phase::Pending] {
        let (_tmp, setup, install, vault) = associated();
        let root = setup.root.as_ref().unwrap().clone();
        let mut binding = read_binding(&root).unwrap().unwrap();
        binding.phase = phase;
        binding.nonce = Some(Uuid::new_v4());
        binding.connection = Some(connection());
        write_binding(&root, &binding).unwrap();
        let next = connection();
        let expected = next.clone();
        let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
            Some(match &request.body {
                RequestBody::ModelStatus {} => idle(),
                RequestBody::CredentialStatus {} => {
                    ResultBody::CredentialState(credential(None, false))
                }
                RequestBody::CredentialPublish { .. } => {
                    assert_eq!(read_binding(&root).unwrap().unwrap().phase, Phase::Pending);
                    ResultBody::CredentialState(credential(Some(expected.generation), true))
                }
                RequestBody::ModelConnect { .. } => remote(&expected, ModelPhase::Ready),
                RequestBody::ModelDisconnectRetained { connection: old } => {
                    assert_eq!(old, &binding.connection.clone().unwrap());
                    ResultBody::Ready {}
                }
                _ => panic!("unexpected reconnect request"),
            })
        });
        let result =
            setup.connect_stored_model(install, &mut client, &vault, || panic!("no recapture"));
        drop(client);
        let traffic = worker.join().unwrap();
        assert!(result.unwrap().ready);
        assert_eq!(traffic.publications(), 1);
        assert_eq!(vault.counts(), (1, 1, 0));
        assert_eq!(
            read_binding(setup.root.as_deref().unwrap())
                .unwrap()
                .unwrap()
                .slot,
            binding.slot
        );
    }
}

#[test]
fn pending_wrong_lease_never_probes_or_replays_connect() {
    let (_tmp, setup, install, vault) = associated();
    let root = setup.root.as_deref().unwrap();
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Pending;
    binding.nonce = Some(Uuid::new_v4());
    write_binding(root, &binding).unwrap();
    let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("wrong lease"));
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(vault.counts(), (1, 0, 0));
}

#[test]
fn lost_delivery_ack_is_one_publication_and_durable_pending_even_with_fresh_client() {
    let (_tmp, setup, install, vault) = associated();
    let root = setup.root.as_ref().unwrap().clone();
    let nonce = Uuid::new_v4();
    let (mut client, worker) = peer(nonce, move |request| match &request.body {
        RequestBody::ModelStatus {} => Some(idle()),
        RequestBody::CredentialStatus {} => {
            Some(ResultBody::CredentialState(credential(None, false)))
        }
        RequestBody::CredentialPublish {
            profile: CREDENTIAL_PROFILE,
            token,
            expected_generation,
        } => {
            assert_eq!(token.as_bytes(), SECRET);
            assert_eq!(*expected_generation, None);
            let binding = read_binding(&root).unwrap().unwrap();
            assert_eq!(binding.phase, Phase::Pending);
            assert_eq!(binding.nonce, Some(nonce));
            None
        }
        _ => panic!("no connect after lost publication acknowledgement"),
    });
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    drop(client);
    let traffic = worker.join().unwrap();
    assert_eq!(traffic.frames.len(), 3);
    assert_eq!(traffic.publications(), 1);
    assert!(traffic.bytes > SECRET.len());
    let reopened = SetupStore::new(Some(_tmp.path().to_owned()));
    let (mut fresh, worker) = peer(Uuid::new_v4(), |_| panic!("pending cannot replay"));
    assert!(reopened
        .connect_model(install, selection(), &mut fresh, &vault)
        .is_err());
    drop(fresh);
    worker.join().unwrap().assert_empty();
    let binding = read_binding(setup.root.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(binding.phase, Phase::Pending);
    assert!(binding.connection.is_none());
    assert!(reopened.model_binding_status().unwrap().unknown);
    assert_eq!(vault.counts(), (1, 1, 0));
    assert_eq!(
        vault.slots.lock().unwrap().get(&binding.slot).unwrap(),
        SECRET
    );
}

#[test]
fn unknown_connect_retains_known_id_and_unknown_disconnect_never_clears_or_deletes() {
    let (_tmp, setup, install, vault) = associated();
    let conn = connection();
    let expected = conn.clone();
    let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
        Some(match &request.body {
            RequestBody::ModelStatus {} => idle(),
            RequestBody::CredentialStatus {} => {
                ResultBody::CredentialState(credential(None, false))
            }
            RequestBody::CredentialPublish { token, .. } => {
                assert_eq!(token.as_bytes(), SECRET);
                ResultBody::CredentialState(credential(Some(expected.generation), true))
            }
            RequestBody::ModelConnect {
                generation,
                selection,
            } => {
                assert_eq!(*generation, expected.generation);
                assert_eq!(*selection, expected.selection);
                remote(&expected, ModelPhase::Unknown)
            }
            RequestBody::ModelDisconnect { connection } => {
                assert_eq!(*connection, expected.id);
                remote(&expected, ModelPhase::Unknown)
            }
            _ => panic!("unknown disconnect must not clear"),
        })
    });
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    let root = setup.root.as_deref().unwrap();
    let binding = read_binding(root).unwrap().unwrap();
    assert_eq!(binding.phase, Phase::Pending);
    assert_eq!(binding.connection, Some(conn.clone()));
    assert!(setup
        .sign_out_model(install, Some(&mut client), &vault)
        .is_err());
    drop(client);
    let traffic = worker.join().unwrap();
    assert_eq!(traffic.frames.len(), 6);
    assert_eq!(traffic.publications(), 1);
    assert!(matches!(
        traffic.frames[4].body,
        RequestBody::ModelStatus {}
    ));
    assert!(
        matches!(traffic.frames[5].body, RequestBody::ModelDisconnect { connection } if connection == conn.id)
    );
    assert_eq!(vault.counts(), (1, 1, 0));
    assert!(vault.slots.lock().unwrap().contains_key(&binding.slot));
    assert_eq!(read_binding(root).unwrap().unwrap().connection, Some(conn));
    assert!(setup.model_binding_status().unwrap().unknown);
}

#[test]
fn failed_same_lease_health_check_persists_unknown_without_load_or_republication() {
    let (_tmp, setup, install, vault) = associated();
    let root = setup.root.as_deref().unwrap();
    let nonce = Uuid::new_v4();
    let conn = connection();
    // Seed valid connected metadata: the existing core test covers initial Connect.
    let mut binding = read_binding(root).unwrap().unwrap();
    binding.phase = Phase::Connected;
    binding.nonce = Some(nonce);
    binding.connection = Some(conn.clone());
    write_binding(root, &binding).unwrap();
    let (mut client, worker) = peer(nonce, move |request| {
        assert!(matches!(request.body, RequestBody::ModelStatus {}));
        Some(remote(&conn, ModelPhase::Unknown))
    });
    assert!(setup
        .connect_model(install, selection(), &mut client, &vault)
        .is_err());
    drop(client);
    let traffic = worker.join().unwrap();
    assert_eq!(traffic.frames.len(), 1);
    assert_eq!(traffic.publications(), 0);
    assert_eq!(vault.counts(), (1, 0, 0));
    assert!(vault.slots.lock().unwrap().contains_key(&binding.slot));
    assert_eq!(read_binding(root).unwrap().unwrap().phase, Phase::Pending);
    assert!(setup.model_binding_status().unwrap().unknown);
    let (mut fresh, worker) = peer(nonce, |request| {
        assert!(matches!(request.body, RequestBody::ModelStatus {}));
        None // No observation: explicit retry must still not repeat a mutation.
    });
    assert!(setup
        .connect_model(install, selection(), &mut fresh, &vault)
        .is_err());
    drop(fresh);
    assert_eq!(worker.join().unwrap().frames.len(), 1);
    assert_eq!(vault.counts(), (1, 0, 0));
}

#[test]
fn acknowledged_clear_allows_explicit_host_cleanup_retry_without_guest_or_load() {
    let (_tmp, setup, install, vault) = associated();
    let expected = connection();
    let conn = expected.clone();
    let mut published = false;
    let (mut client, worker) = peer(Uuid::new_v4(), move |request| {
        Some(match &request.body {
            RequestBody::ModelStatus {} => idle(),
            RequestBody::CredentialStatus {} => ResultBody::CredentialState(credential(
                if published {
                    Some(expected.generation)
                } else {
                    None
                },
                published,
            )),
            RequestBody::CredentialPublish { token, .. } => {
                assert_eq!(token.as_bytes(), SECRET);
                published = true;
                ResultBody::CredentialState(credential(Some(expected.generation), true))
            }
            RequestBody::ModelConnect { .. } => remote(&expected, ModelPhase::Ready),
            RequestBody::ModelDisconnect { connection } => {
                assert_eq!(*connection, expected.id);
                ResultBody::ModelState(ModelState {
                    retained_cleanup: true,
                    diagnostic: None,
                    phase: ModelPhase::Idle,
                    connection: None,
                    credential: credential(Some(expected.generation), true),
                })
            }
            RequestBody::CredentialClear { .. } => {
                ResultBody::CredentialState(credential(Some(Uuid::new_v4()), false))
            }
            _ => panic!("unexpected cleanup request"),
        })
    });
    setup
        .connect_model(install, conn.selection, &mut client, &vault)
        .unwrap();
    *vault.deny_delete.lock().unwrap() = true;
    assert!(setup
        .sign_out_model(install, Some(&mut client), &vault)
        .is_err());
    assert!(setup.model_binding_status().unwrap().associated);
    assert!(!setup.model_binding_status().unwrap().ready);
    *vault.deny_delete.lock().unwrap() = false;
    assert!(
        setup.sign_out_model(install, None, &vault).is_ok(),
        "acknowledged clear must permit exact host-only cleanup retry"
    );
    drop(client);
    let traffic = worker.join().unwrap();
    assert_eq!(traffic.frames.len(), 7);
    assert_eq!(traffic.publications(), 1);
    assert_eq!(vault.counts(), (1, 1, 2));
    assert!(!setup.model_binding_status().unwrap().associated);
}

#[test]
fn publication_failure_retains_exact_new_slot_not_unowned_slot_or_file() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    let unowned = Uuid::new_v4();
    vault
        .slots
        .lock()
        .unwrap()
        .insert(unowned, b"unowned-synthetic".to_vec());
    let root = setup.root.as_deref().unwrap();
    let sentinel = b"unowned-file-unchanged";
    fs::write(root.join("unowned-note"), sentinel).unwrap();
    *vault.block_publish.lock().unwrap() = Some((root.to_owned(), 0o500));
    assert!(setup.associate_secret(install, SECRET, &vault).is_err());
    permissions(root, 0o700);
    {
        let calls = vault.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let generated = calls[0].1;
        assert_ne!(generated, unowned);
        assert_eq!(*calls, vec![("save", generated)]);
        assert_eq!(read_binding(root).unwrap().unwrap().slot, generated);
        let slots = vault.slots.lock().unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(slots.get(&unowned).unwrap(), b"unowned-synthetic");
    }
    assert_eq!(fs::read(root.join("unowned-note")).unwrap(), sentinel);
    assert!(root.join("model-v1").is_file());
    assert!(
        !fs::read_dir(root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("model-next")),
        "failed publication must clean only its owned temporary"
    );
    setup.sign_out_model(install, None, &vault).unwrap();
    assert!(
        setup
            .associate_secret(install, SECRET, &vault)
            .unwrap()
            .associated
    );
}

#[test]
fn unknown_crash_temporary_does_not_block_or_get_deleted_by_new_publication() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    let root = setup.root.as_deref().unwrap();
    let sentinel = b"unknown-crash-metadata";
    fs::write(root.join("model-next"), sentinel).unwrap();
    assert!(
        setup
            .associate_secret(install, SECRET, &vault)
            .unwrap()
            .associated
    );
    assert_eq!(fs::read(root.join("model-next")).unwrap(), sentinel);
}
