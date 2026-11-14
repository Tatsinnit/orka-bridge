use bridge_runtime_protocol::*;
#[test]
fn vendor_is_explicit_closed_and_retained_in_connection_identity() {
    let old = serde_json::json!({"provider":"workspace-model","model":"gpt-4.1"});
    let legacy: ModelSelection = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(legacy).unwrap(), old);
    for vendor in ["openai", "anthropic"] {
        let value =
            serde_json::json!({"provider":"workspace-model","model":"default","vendor":vendor});
        let selection: ModelSelection = serde_json::from_value(value.clone()).unwrap();
        assert!(selection.valid());
        assert_eq!(serde_json::to_value(selection).unwrap(), value);
    }
    for vendor in ["", "copilot", "azure-openai", "OPENAI", "../other"] {
        assert!(serde_json::from_value::<ModelSelection>(serde_json::json!({
            "provider":"workspace-model","model":"default","vendor":vendor
        }))
        .is_err());
    }
    for (code, message) in [
        (
            "credential_rejected",
            "The model company rejected the credential.",
        ),
        (
            "provider_unreachable",
            "The workspace could not reach the model company.",
        ),
        (
            "model_unavailable",
            "The selected model is not advertised for this credential.",
        ),
    ] {
        let error: ModelErrorCode = serde_json::from_value(serde_json::json!(code)).unwrap();
        assert_eq!(serde_json::to_value(error).unwrap(), code);
        let stderr: ModelStderr = serde_json::from_value(serde_json::json!(message)).unwrap();
        assert_eq!(serde_json::to_value(stderr).unwrap(), message);
    }
}

#[test]
fn native_model_selection_rejects_kmx_preflight_errors_before_delivery() {
    for provider in ["", "UPPER", "with.dot", "-start", "end-", &"x".repeat(46)] {
        assert!(!ModelSelection {
            vendor: None,
            provider: provider.into(),
            model: "gpt-4.1".into()
        }
        .valid());
    }
    for model in [
        "",
        "auto",
        "AUTO",
        "white space",
        "line\n",
        &"x".repeat(257),
    ] {
        assert!(!ModelSelection {
            vendor: None,
            provider: "copilot".into(),
            model: model.into()
        }
        .valid());
    }
    assert!(ModelSelection {
        vendor: None,
        provider: "copilot-1".into(),
        model: "gpt-4.1".into()
    }
    .valid());
}
#[test]
fn model_support_can_be_checked_without_loading_or_sending_a_secret() {
    let j = serde_json::json!({"type":"request","body":{"version":VERSION,"bootNonce":Uuid::new_v4(),"requestId":3,"body":{"op":"modelStatus","body":{}}}});
    assert!(
        serde_json::from_value::<Frame>(j).is_ok(),
        "model capability must precede secret access"
    );
}
#[test]
fn retained_disconnect_validates_all_nonsecret_connection_fields() {
    let good = ModelConnection {
        id: Uuid::new_v4(),
        generation: Uuid::new_v4(),
        selection: ModelSelection {
            vendor: None,
            provider: "copilot".into(),
            model: "gpt-4.1".into(),
        },
    };
    for case in 0..4 {
        let mut connection = good.clone();
        match case {
            1 => connection.id = Uuid::nil(),
            2 => connection.generation = Uuid::nil(),
            3 => connection.selection.provider = "../other".into(),
            _ => {}
        }
        let frame = Frame::Request(WireRequest {
            version: VERSION,
            boot_nonce: Uuid::new_v4(),
            request_id: 1,
            body: RequestBody::ModelDisconnectRetained { connection },
        });
        assert_eq!(encode(&frame).is_ok(), case == 0);
    }
}
#[test]
fn model_diagnostics_accept_old_omission_but_refuse_unbounded_vocabulary() {
    let old = serde_json::json!({"phase":"unknown", "connection":null,
        "credential":{"profile":CREDENTIAL_PROFILE,"installed":true,"generation":Uuid::new_v4()}});
    let state: ModelState = serde_json::from_value(old.clone()).unwrap();
    assert!(serde_json::to_value(state)
        .unwrap()
        .get("diagnostic")
        .is_none());
    let mut new = old.clone();
    new["diagnostic"] = serde_json::json!({"classification":"timeout","elapsedMs":60001,
        "exitCode":null,"stage":"gateway","errorCode":"deadline_exceeded","stderr":"Stderr withheld."});
    let state: ModelState = serde_json::from_value(new.clone()).unwrap();
    let frame = Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: Uuid::new_v4(),
        request_id: 1,
        body: ResultBody::ModelState(state),
    });
    assert!(encode(&frame).is_ok());
    for key in ["classification", "stage", "errorCode", "stderr"] {
        let mut bad = new.clone();
        bad["diagnostic"][key] = serde_json::json!("synthetic-private-output");
        assert!(serde_json::from_value::<ModelState>(bad).is_err(), "{key}");
    }
    let mut bad = new.clone();
    bad["diagnostic"]["raw"] = serde_json::json!("synthetic-private-output");
    assert!(serde_json::from_value::<ModelState>(bad).is_err());
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OldState {
        phase: ModelPhase,
        connection: Option<ModelConnection>,
        credential: CredentialState,
    }
    let prior: OldState = serde_json::from_value(old).unwrap();
    assert_eq!(prior.phase, ModelPhase::Unknown);
    assert!(prior.connection.is_none());
    assert!(prior.credential.installed);
    assert!(
        serde_json::from_value::<OldState>(new).is_err(),
        "old reader cannot accept new diagnostics"
    );
}
