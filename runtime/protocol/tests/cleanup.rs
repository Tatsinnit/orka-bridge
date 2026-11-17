use bridge_runtime_protocol::*;
use serde_json::{json, Value};

#[test]
fn cleanup_action_accepts_only_a_bounded_uid_not_name_namespace_or_generic_delete() {
    let action: EngineActionWire =
        serde_json::from_value(json!({"type":"cleanupRunAgent","uid":"recorded-uid"})).unwrap();
    assert!(validate_action(&action).is_ok());
    for uid in ["".to_owned(), "x".repeat(65)] {
        let action: EngineActionWire =
            serde_json::from_value(json!({"type":"cleanupRunAgent","uid":uid})).unwrap();
        assert_eq!(validate_action(&action), Err(ProtocolError::Invalid));
        for execute in [false, true] {
            let body = if execute {
                RequestBody::Execute {
                    operation: Uuid::new_v4(),
                    action: action.clone(),
                    snapshot: None,
                }
            } else {
                RequestBody::Engine(action.clone())
            };
            assert!(Frame::Request(WireRequest {
                version: VERSION,
                boot_nonce: Uuid::new_v4(),
                request_id: 3,
                body
            })
            .validate()
            .is_err());
        }
    }
    let max: EngineActionWire =
        serde_json::from_value(json!({"type":"cleanupRunAgent","uid":"x".repeat(64)})).unwrap();
    assert!(validate_action(&max).is_ok());
    for field in ["name", "namespace", "path", "selector", "method"] {
        let mut wire = json!({"type":"cleanupRunAgent","uid":"recorded-uid"});
        wire[field] = json!("injected");
        assert!(serde_json::from_value::<EngineActionWire>(wire).is_err());
    }
    assert!(serde_json::from_value::<EngineActionWire>(json!({"type":"orka","request":{
        "method":"delete","path":"agents/bridge-run-0123456789abcdef0123456789abcdef","after":null,"body":null}})).is_err());
}
#[test]
fn cleanup_capability_is_serve_only_optional_and_false_is_omitted() {
    let nonce = Uuid::from_u128(42);
    for stage in ["early", "serve"] {
        let mut wire = json!({"version":VERSION,"stage":stage,"guestNonce":Uuid::from_u128(7),
            "acceptedBootNonce": if stage == "serve" {json!(nonce)} else {Value::Null}});
        let old: StageHello = serde_json::from_value(wire.clone()).unwrap();
        assert!(validate_hello(&old, old.stage, nonce).is_ok());
        assert!(serde_json::to_value(old)
            .unwrap()
            .get("runCleanup")
            .is_none());
        wire["runCleanup"] = json!(true);
        let new: StageHello = serde_json::from_value(wire).unwrap();
        assert_eq!(
            validate_hello(&new, new.stage, nonce).is_ok(),
            stage == "serve"
        );
        assert_eq!(serde_json::to_value(new).unwrap()["runCleanup"], true);
    }
}
