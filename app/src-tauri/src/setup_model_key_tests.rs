//! Key-only cases reuse the established-channel and native-store doubles.
use super::*;

const KEY: &str = "sk-ant-synthetic-model-only";
const OPENAI_KEY: &str = "sk-synthetic-model-only";

fn key_selection(vendor: ModelVendor) -> ModelSelection {
    ModelSelection {
        vendor: Some(vendor),
        provider: "bridge-model".into(),
        model: match vendor {
            ModelVendor::Openai => "gpt-4.1",
            ModelVendor::Anthropic => "claude-sonnet-5-5",
        }
        .into(),
    }
}
fn key_credential(generation: Option<Uuid>, installed: bool) -> CredentialState {
    CredentialState {
        profile: 2,
        generation,
        installed,
    }
}
fn key_remote(
    connection: Option<ModelConnection>,
    generation: Uuid,
    phase: ModelPhase,
    code: Option<ModelErrorCode>,
) -> ResultBody {
    ResultBody::ModelState(ModelState {
        retained_cleanup: true,
        connection,
        phase,
        credential: key_credential(Some(generation), true),
        diagnostic: code.map(|code| ModelDiagnostic {
            classification: ModelDiagnosticClass::Exit,
            elapsed_ms: 9,
            exit_code: Some(1),
            stage: Some(ModelStage::Model),
            error_code: Some(code),
            // Deliberately unrelated: category must come from errorCode only.
            stderr: Some(ModelStderr::OperationFailed),
        }),
    })
}
fn assert_no_file_key(root: &Path) {
    for entry in fs::read_dir(root).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        for secret in [KEY, OPENAI_KEY] {
            assert!(!bytes
                .windows(secret.len())
                .any(|part| part == secret.as_bytes()));
        }
    }
}
fn operations(traffic: Traffic) -> Vec<&'static str> {
    traffic
        .frames
        .into_iter()
        .map(|r| match r.body {
            RequestBody::ModelStatus {} => "modelStatus",
            RequestBody::ModelDisconnectRetained { .. } => "retained",
            RequestBody::CredentialStatus {} => "credentialStatus",
            RequestBody::CredentialPublish { .. } => "publish",
            RequestBody::ModelConnect { .. } => "connect",
            _ => panic!("unexpected key Connect operation"),
        })
        .collect()
}

#[test]
fn invalid_keys_and_existing_binding_refuse_before_store_or_guest_io() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("refusal must precede I/O"));
    for (vendor, key) in [
        (ModelVendor::Openai, "".to_owned()),
        (ModelVendor::Openai, "sk-".to_owned()),
        (ModelVendor::Openai, KEY.to_owned()),
        (ModelVendor::Openai, "sk-synthetic\nprivate".to_owned()),
        (ModelVendor::Openai, "sk-synthetic private".to_owned()),
        (ModelVendor::Openai, "sk-é".to_owned()),
        (
            ModelVendor::Openai,
            format!("sk-{}", "x".repeat(MAX_CREDENTIAL)),
        ),
        (ModelVendor::Anthropic, "sk-ant-".to_owned()),
        (ModelVendor::Anthropic, OPENAI_KEY.to_owned()),
    ] {
        let error = setup
            .connect_key_model(install, vendor, &key, &mut client, &vault)
            .unwrap_err();
        assert_eq!(error.kind, "modelKeyInvalid");
        assert_eq!(
            serde_json::to_value(error)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            2
        );
    }
    assert_eq!(vault.counts(), (0, 0, 0));
    setup.associate_secret(install, SECRET, &vault).unwrap();
    assert_eq!(
        setup
            .connect_key_model(install, ModelVendor::Anthropic, KEY, &mut client, &vault)
            .unwrap_err()
            .kind,
        "modelAlreadyAssociated"
    );
    drop(client);
    worker.join().unwrap().assert_empty();
    assert_eq!(vault.counts(), (1, 0, 0));
    let raw = fs::read(setup.root.as_deref().unwrap().join("model-v1")).unwrap();
    let legacy: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert!(legacy.get("kind").is_none() && legacy.get("vendor").is_none());
}

#[test]
fn key_connect_persists_only_metadata_and_reopens_without_github_capture() {
    for (vendor, key) in [
        (ModelVendor::Openai, OPENAI_KEY),
        (ModelVendor::Anthropic, KEY),
    ] {
        let (tmp, setup, install) = selected_setup();
        let vault = FakeVault {
            secret: Some(key.as_bytes()),
            ..FakeVault::default()
        };
        let root = setup.root.as_deref().unwrap();
        let mut previous = None;
        for boot in 0..2 {
            let conn = ModelConnection {
                id: Uuid::new_v4(),
                generation: Uuid::new_v4(),
                selection: key_selection(vendor),
            };
            let expected = conn.clone();
            let retained = previous.clone();
            let mut connected = false;
            let (mut client, worker) = peer(Uuid::new_v4(), move |r| {
                Some(match &r.body {
                    RequestBody::ModelStatus {} if connected => key_remote(
                        Some(expected.clone()),
                        expected.generation,
                        ModelPhase::Ready,
                        None,
                    ),
                    RequestBody::ModelStatus {} => idle(),
                    RequestBody::ModelDisconnectRetained { connection } => {
                        assert_eq!(Some(connection), retained.as_ref());
                        ResultBody::Ready {}
                    }
                    RequestBody::CredentialStatus {} => {
                        ResultBody::CredentialState(credential(None, false))
                    }
                    RequestBody::CredentialPublish {
                        profile,
                        expected_generation,
                        token,
                    } => {
                        assert_eq!((*profile, *expected_generation), (2, None));
                        assert!(token.as_bytes() == key.as_bytes());
                        ResultBody::CredentialState(key_credential(Some(expected.generation), true))
                    }
                    RequestBody::ModelConnect {
                        generation,
                        selection,
                    } => {
                        assert_eq!(*generation, expected.generation);
                        assert_eq!(selection, &expected.selection);
                        connected = true;
                        key_remote(
                            Some(expected.clone()),
                            expected.generation,
                            ModelPhase::Ready,
                            None,
                        )
                    }
                    _ => panic!("unexpected key Connect request"),
                })
            });
            let reopened = SetupStore::new(Some(tmp.path().to_owned()));
            let ready = if boot == 0 {
                setup
                    .connect_key_model(install, vendor, key, &mut client, &vault)
                    .unwrap();
                reopened
                    .connect_stored_model(install, &mut client, &vault, || panic!("no recapture"))
            } else {
                reopened
                    .connect_stored_model(install, &mut client, &vault, || panic!("no recapture"))
            }
            .unwrap();
            assert!(ready.ready);
            assert_eq!(ready.connection, Some(conn.clone()));
            drop(client);
            assert_eq!(
                operations(worker.join().unwrap()),
                if boot == 0 {
                    vec![
                        "modelStatus",
                        "modelStatus",
                        "credentialStatus",
                        "publish",
                        "connect",
                        "modelStatus",
                    ]
                } else {
                    vec![
                        "modelStatus",
                        "retained",
                        "modelStatus",
                        "credentialStatus",
                        "publish",
                        "connect",
                    ]
                }
            );
            assert_eq!(vault.counts(), (1, boot + 1, 0));
            let metadata: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join("model-v1")).unwrap()).unwrap();
            assert_eq!(metadata["kind"], "key");
            assert_eq!(metadata["vendor"], vendor.as_str());
            assert_no_file_key(root);
            previous = Some(conn);
        }
    }
}

#[test]
fn saved_key_retry_without_an_association_never_captures_or_touches_guest() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault::default();
    let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("no guest IO"));
    let error = setup
        .connect_saved_key_model(install, ModelVendor::Anthropic, &mut client, &vault)
        .unwrap_err();
    assert_eq!(error.kind, "modelKeyInvalid");
    assert_eq!(vault.counts(), (0, 0, 0));
    drop(client);
    assert!(worker.join().unwrap().frames.is_empty());
}
#[test]
fn reopened_key_failure_carries_only_the_saved_company_for_recovery() {
    let (tmp, setup, install) = selected_setup();
    let vault = FakeVault {
        secret: Some(KEY.as_bytes()),
        ..FakeVault::default()
    };
    setup
        .associate_secret_for_vendor(
            install,
            KEY.as_bytes(),
            Some(ModelVendor::Anthropic),
            &vault,
        )
        .unwrap();
    let reopened = SetupStore::new(Some(tmp.path().to_owned()));
    let (mut client, worker) = peer(Uuid::new_v4(), |r| {
        assert!(matches!(r.body, RequestBody::ModelStatus {}));
        Some(key_remote(None, Uuid::new_v4(), ModelPhase::Unknown, None))
    });
    let error = reopened
        .connect_stored_model(install, &mut client, &vault, || panic!("must not capture"))
        .unwrap_err();
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(wire["kind"], "modelGuest");
    assert_eq!(wire["vendor"], "anthropic");
    assert!(!wire.to_string().contains(KEY));
    drop(client);
    assert_eq!(worker.join().unwrap().frames.len(), 1);
    assert_eq!(vault.counts(), (1, 0, 0));
}
#[test]
fn persisted_kind_vendor_mismatch_cannot_authorize_guest_or_vault_io() {
    let (_tmp, setup, install, vault) = associated();
    let root = setup.root.as_deref().unwrap();
    let mut raw = serde_json::to_value(read_binding(root).unwrap().unwrap()).unwrap();
    raw["phase"] = "connected".into();
    raw["nonce"] = Uuid::new_v4().to_string().into();
    raw["connection"] = serde_json::to_value(ModelConnection {
        selection: key_selection(ModelVendor::Anthropic),
        ..connection()
    })
    .unwrap();
    for (kind, vendor) in [
        ("copilot", serde_json::Value::Null),
        ("key", "openai".into()),
        ("key", serde_json::Value::Null),
    ] {
        raw["kind"] = kind.into();
        raw["vendor"] = vendor;
        fs::write(root.join("model-v1"), serde_json::to_vec(&raw).unwrap()).unwrap();
        let (mut client, worker) = peer(Uuid::new_v4(), |_| panic!("invalid metadata"));
        assert!(setup.model_binding_status().is_err());
        assert!(setup
            .connect_stored_model(install, &mut client, &vault, || panic!("invalid metadata"))
            .is_err());
        assert!(setup
            .sign_out_model(install, Some(&mut client), &vault)
            .is_err());
        drop(client);
        worker.join().unwrap().assert_empty();
    }
    assert_eq!(vault.counts(), (1, 0, 0));
}

#[test]
fn definitive_key_failures_disconnect_clear_delete_and_cleanup_uncertainty_wins() {
    // Missing identity, lost Disconnect/Clear, and failed vault delete retain the slot.
    for (code, kind) in [
        (
            ModelErrorCode::CredentialRejected,
            "modelCredentialRejected",
        ),
        (
            ModelErrorCode::ProviderUnreachable,
            "modelProviderUnreachable",
        ),
        (ModelErrorCode::ModelUnavailable, "modelModelUnavailable"),
    ] {
        for failure in 0..5 {
            let (_tmp, setup, install) = selected_setup();
            let vault = Arc::new(FakeVault {
                secret: Some(KEY.as_bytes()),
                deny_delete: Mutex::new(failure == 4),
                ..FakeVault::default()
            });
            let observed = vault.clone();
            let generation = Uuid::new_v4();
            let conn = ModelConnection {
                id: Uuid::new_v4(),
                generation,
                selection: key_selection(ModelVendor::Anthropic),
            };
            let cleared = Uuid::new_v4();
            let mut step = 0;
            let (mut client, worker) = peer(Uuid::new_v4(), move |r| {
                step += 1;
                Some(match (step, &r.body) {
                    (1 | 2, RequestBody::ModelStatus {}) => idle(),
                    (3, RequestBody::CredentialStatus {}) => {
                        ResultBody::CredentialState(credential(None, false))
                    }
                    (4, RequestBody::CredentialPublish { profile: 2, .. }) => {
                        ResultBody::CredentialState(key_credential(Some(generation), true))
                    }
                    (5, RequestBody::ModelConnect { .. }) => key_remote(
                        (failure != 1).then(|| conn.clone()),
                        generation,
                        ModelPhase::Unknown,
                        Some(code),
                    ),
                    (6, RequestBody::ModelDisconnect { connection }) => {
                        assert_eq!(*connection, conn.id);
                        assert_eq!(observed.counts(), (1, 1, 0));
                        if failure == 2 {
                            return None;
                        }
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            phase: ModelPhase::Idle,
                            connection: None,
                            credential: key_credential(Some(generation), true),
                            diagnostic: None,
                        })
                    }
                    (7, RequestBody::CredentialStatus {}) => {
                        ResultBody::CredentialState(key_credential(Some(generation), true))
                    }
                    (
                        8,
                        RequestBody::CredentialClear {
                            expected_generation,
                        },
                    ) => {
                        assert_eq!(*expected_generation, generation);
                        assert_eq!(observed.counts(), (1, 1, 0));
                        if failure == 3 {
                            return None;
                        }
                        ResultBody::CredentialState(key_credential(Some(cleared), false))
                    }
                    _ => panic!("unexpected cleanup request at step {step}"),
                })
            });
            let error = setup
                .connect_key_model(install, ModelVendor::Anthropic, KEY, &mut client, &*vault)
                .unwrap_err();
            assert_eq!(
                error.kind,
                if failure == 0 {
                    kind
                } else if failure == 1 {
                    "modelConnect"
                } else {
                    "modelKeyCleanup"
                }
            );
            let wire = serde_json::to_value(error).unwrap();
            assert!(wire.get("diagnostic").is_none());
            assert!(!wire.to_string().contains(KEY));
            drop(client);
            assert_eq!(
                worker.join().unwrap().frames.len(),
                match failure {
                    1 => 5,
                    2 => 6,
                    _ => 8,
                }
            );
            assert_eq!(
                vault.counts(),
                (1, 1, usize::from(failure == 0 || failure == 4))
            );
            assert_eq!(vault.slots.lock().unwrap().is_empty(), failure == 0);
            assert_eq!(
                read_binding(setup.root.as_deref().unwrap())
                    .unwrap()
                    .is_none(),
                failure == 0
            );
            assert_no_file_key(setup.root.as_deref().unwrap());
        }
    }
}

#[test]
fn pending_key_disconnect_validation_failure_never_reports_credential_removal() {
    for code in [
        ModelErrorCode::CredentialRejected,
        ModelErrorCode::ProviderUnreachable,
        ModelErrorCode::ModelUnavailable,
    ] {
        let (_tmp, setup, install) = selected_setup();
        let vault = FakeVault {
            secret: Some(KEY.as_bytes()),
            ..FakeVault::default()
        };
        setup
            .associate_secret_for_vendor(
                install,
                KEY.as_bytes(),
                Some(ModelVendor::Anthropic),
                &vault,
            )
            .unwrap();
        let conn = ModelConnection {
            selection: key_selection(ModelVendor::Anthropic),
            ..connection()
        };
        let nonce = Uuid::new_v4();
        let root = setup.root.as_ref().unwrap();
        let mut binding = read_binding(root).unwrap().unwrap();
        binding.phase = Phase::Pending;
        binding.nonce = Some(nonce);
        binding.connection = Some(conn.clone());
        write_binding(root, &binding).unwrap();
        let (mut client, worker) = peer(nonce, move |r| {
            Some(match &r.body {
                RequestBody::ModelStatus {} => key_remote(
                    Some(conn.clone()),
                    conn.generation,
                    ModelPhase::Unknown,
                    None,
                ),
                RequestBody::ModelDisconnect { .. } => key_remote(
                    Some(conn.clone()),
                    conn.generation,
                    ModelPhase::Unknown,
                    Some(code),
                ),
                _ => panic!("failed Disconnect cannot clear or publish credentials"),
            })
        });
        let error = setup
            .connect_saved_key_model(install, ModelVendor::Anthropic, &mut client, &vault)
            .unwrap_err();
        assert_eq!(error.kind, "modelKeyCleanup");
        assert_eq!(error.vendor, Some(ModelVendor::Anthropic));
        assert!(error.diagnostic.is_some());
        assert_eq!(vault.counts(), (1, 0, 0));
        assert_eq!(read_binding(root).unwrap().unwrap().phase, Phase::Pending);
        assert!(vault.slots.lock().unwrap().contains_key(&binding.slot));
        drop(client);
        assert_eq!(worker.join().unwrap().frames.len(), 2);
    }
}

#[test]
fn identityless_key_failure_is_uncertain_and_retries_only_the_installed_attempt() {
    let (_tmp, setup, install) = selected_setup();
    let vault = FakeVault {
        secret: Some(KEY.as_bytes()),
        ..FakeVault::default()
    };
    let conn = ModelConnection {
        selection: key_selection(ModelVendor::Anthropic),
        ..connection()
    };
    let expected = conn.clone();
    let mut step = 0;
    let (mut client, worker) = peer(Uuid::new_v4(), move |r| {
        step += 1;
        Some(match (step, &r.body) {
            (1 | 2, RequestBody::ModelStatus {}) => idle(),
            (3, RequestBody::CredentialStatus {}) => {
                ResultBody::CredentialState(credential(None, false))
            }
            (4, RequestBody::CredentialPublish { profile: 2, .. }) => {
                ResultBody::CredentialState(key_credential(Some(expected.generation), true))
            }
            (5, RequestBody::ModelConnect { .. }) => key_remote(
                None,
                expected.generation,
                ModelPhase::Unknown,
                Some(ModelErrorCode::CredentialRejected),
            ),
            (6, RequestBody::ModelStatus {}) => {
                key_remote(None, expected.generation, ModelPhase::Unknown, None)
            }
            (
                7,
                RequestBody::ModelConnect {
                    generation,
                    selection,
                },
            ) => {
                assert_eq!(
                    (*generation, selection),
                    (expected.generation, &expected.selection)
                );
                key_remote(
                    Some(expected.clone()),
                    expected.generation,
                    ModelPhase::Ready,
                    None,
                )
            }
            _ => panic!("identityless retry must not clear, load, publish or guess an identity"),
        })
    });
    let error = setup
        .connect_key_model(install, ModelVendor::Anthropic, KEY, &mut client, &vault)
        .unwrap_err();
    assert_eq!(error.kind, "modelConnect");
    assert_eq!(vault.counts(), (1, 1, 0));
    assert!(setup.model_binding_status().unwrap().unknown);
    assert!(
        setup
            .connect_saved_key_model(install, ModelVendor::Anthropic, &mut client, &vault)
            .unwrap()
            .ready
    );
    assert_eq!(vault.counts(), (1, 1, 0));
    drop(client);
    assert_eq!(worker.join().unwrap().frames.len(), 7);
}

#[test]
fn cleanup_completion_requires_all_acknowledged_evidence() {
    let conn = connection();
    let cleared = ModelState {
        retained_cleanup: true,
        phase: ModelPhase::Idle,
        connection: None,
        credential: key_credential(Some(Uuid::new_v4()), false),
        diagnostic: None,
    };
    assert!(cleanup_acknowledged(&cleared, &conn));
    for variant in 0..5 {
        let mut uncertain = cleared.clone();
        match variant {
            0 => uncertain.phase = ModelPhase::Unknown,
            1 => uncertain.connection = Some(conn.clone()),
            2 => uncertain.credential.installed = true,
            3 => uncertain.credential.generation = Some(conn.generation),
            _ => uncertain.credential.generation = None,
        }
        assert!(
            !cleanup_acknowledged(&uncertain, &conn),
            "variant {variant}"
        );
    }
}

#[test]
fn automatic_key_cleanup_recovers_a_failed_cleared_checkpoint_on_remove() {
    let (_tmp, setup, install) = selected_setup();
    let root = setup.root.as_ref().unwrap().clone();
    let checkpoint = Arc::new(Mutex::new(Vec::new()));
    let observed = checkpoint.clone();
    let vault = FakeVault {
        secret: Some(KEY.as_bytes()),
        ..FakeVault::default()
    };
    let conn = ModelConnection {
        selection: key_selection(ModelVendor::Anthropic),
        ..connection()
    };
    let cleared = Uuid::new_v4();
    let mut step = 0;
    let (mut client, worker) = peer(Uuid::new_v4(), move |r| {
        step += 1;
        Some(match (step, &r.body) {
            (1 | 2, RequestBody::ModelStatus {}) => idle(),
            (3, RequestBody::CredentialStatus {}) => {
                ResultBody::CredentialState(credential(None, false))
            }
            (4, RequestBody::CredentialPublish { profile: 2, .. }) => {
                ResultBody::CredentialState(key_credential(Some(conn.generation), true))
            }
            (5, RequestBody::ModelConnect { .. }) => key_remote(
                Some(conn.clone()),
                conn.generation,
                ModelPhase::Unknown,
                Some(ModelErrorCode::CredentialRejected),
            ),
            (6, RequestBody::ModelDisconnect { .. }) => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                phase: ModelPhase::Idle,
                connection: None,
                credential: key_credential(Some(conn.generation), true),
                diagnostic: None,
            }),
            (7, RequestBody::CredentialStatus {}) => {
                ResultBody::CredentialState(key_credential(Some(conn.generation), true))
            }
            (8, RequestBody::CredentialClear { .. }) => {
                let path = root.join("model-v1");
                *observed.lock().unwrap() = fs::read(&path).unwrap();
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
                ResultBody::CredentialState(key_credential(Some(cleared), false))
            }
            (9, _) => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                phase: ModelPhase::Idle,
                connection: None,
                credential: key_credential(Some(cleared), false),
                diagnostic: None,
            }),
            _ => panic!("completed cleanup must not replay credential operations"),
        })
    });
    assert_eq!(
        setup
            .connect_key_model(install, ModelVendor::Anthropic, KEY, &mut client, &vault)
            .unwrap_err()
            .kind,
        "modelKeyCleanup"
    );
    let path = setup.root.as_ref().unwrap().join("model-v1");
    fs::remove_dir(&path).unwrap();
    fs::write(&path, &*checkpoint.lock().unwrap()).unwrap();
    assert_eq!(
        read_binding(setup.root.as_ref().unwrap())
            .unwrap()
            .unwrap()
            .phase,
        Phase::Pending
    );
    assert!(
        !setup
            .sign_out_model(install, Some(&mut client), &vault)
            .unwrap()
            .associated
    );
    assert_eq!(vault.counts(), (1, 1, 1));
    assert!(!path.exists());
    drop(client);
    let traffic = worker.join().unwrap();
    assert_eq!(traffic.frames.len(), 9);
    assert!(matches!(
        traffic.frames.last().unwrap().body,
        RequestBody::ModelStatus {}
    ));
}

#[test]
fn unknown_key_delivery_or_connect_never_deletes_or_republishes() {
    for lost_at in [4, 5, 6] {
        let (_tmp, setup, install) = selected_setup();
        let vault = FakeVault {
            secret: Some(KEY.as_bytes()),
            ..FakeVault::default()
        };
        let conn = ModelConnection {
            selection: key_selection(ModelVendor::Anthropic),
            ..connection()
        };
        let generation = conn.generation;
        let mut step = 0;
        let (mut client, worker) = peer(Uuid::new_v4(), move |r| {
            step += 1;
            if step == lost_at {
                return None;
            }
            Some(match (step, &r.body) {
                (1 | 2, RequestBody::ModelStatus {}) => idle(),
                (3, RequestBody::CredentialStatus {}) => {
                    ResultBody::CredentialState(credential(None, false))
                }
                (4, RequestBody::CredentialPublish { profile: 2, .. }) => {
                    ResultBody::CredentialState(key_credential(Some(generation), true))
                }
                (5, RequestBody::ModelConnect { .. }) => key_remote(
                    Some(conn.clone()),
                    generation,
                    ModelPhase::Unknown,
                    Some(ModelErrorCode::ExecutionUnknown),
                ),
                _ => panic!("unknown Connect must not attempt cleanup"),
            })
        });
        let error = setup
            .connect_key_model(install, ModelVendor::Anthropic, KEY, &mut client, &vault)
            .unwrap_err();
        assert_eq!(
            error.kind,
            if lost_at == 4 {
                "modelDelivery"
            } else {
                "modelConnect"
            }
        );
        drop(client);
        assert_eq!(worker.join().unwrap().frames.len(), lost_at.min(5));
        assert_eq!(vault.counts(), (1, 1, 0));
        assert_eq!(vault.slots.lock().unwrap().len(), 1);
        let binding = read_binding(setup.root.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(binding.phase, Phase::Pending);
        assert_eq!(binding.connection.is_some(), lost_at == 6);
        assert_no_file_key(setup.root.as_deref().unwrap());
    }
}

#[test]
fn diagnostic_mapping_does_not_infer_category_from_stderr() {
    let error = SetupError::connect(Some(ModelDiagnostic {
        classification: ModelDiagnosticClass::Exit,
        elapsed_ms: 1,
        exit_code: Some(1),
        stage: Some(ModelStage::Gateway),
        error_code: Some(ModelErrorCode::ExecutionUnknown),
        stderr: Some(ModelStderr::CredentialRejected),
    }));
    assert_eq!(error.kind, "modelConnect");
    assert!(serde_json::to_value(error)
        .unwrap()
        .get("diagnostic")
        .is_none());
}
