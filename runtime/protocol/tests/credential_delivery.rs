use bridge_runtime_protocol::*;

#[test]
fn private_credential_capability_is_a_nonsecret_wire_operation() {
    let value = serde_json::json!({"type":"request","body":{
        "version":VERSION,"bootNonce":Uuid::new_v4(),"requestId":3,
        "body":{"op":"credentialStatus","body":{}}
    }});
    let decoded = serde_json::from_value::<Frame>(value);
    assert!(
        decoded.is_ok(),
        "a host must negotiate credential support before sending a secret"
    );
    decoded.unwrap().validate().unwrap();
}

#[test]
fn credential_profiles_preserve_legacy_requests_and_reject_unknown_profiles() {
    for profile in [1, 2, 0, 3] {
        let value = serde_json::json!({"type":"request","body":{
            "version":VERSION,"bootNonce":Uuid::new_v4(),"requestId":3,
            "body":{"op":"credentialPublish","body":{
                "profile":profile,"expected_generation":null,"token":[120]}}
        }});
        let frame: Frame = serde_json::from_value(value).unwrap();
        assert_eq!(frame.validate().is_ok(), matches!(profile, 1 | 2));
        if profile == 1 {
            let wire = serde_json::to_value(frame).unwrap();
            assert!(wire["body"]["body"]["body"].get("profile").is_none());
        }
    }
    let old = serde_json::json!({"op":"credentialPublish","body":{
        "expected_generation":null,"token":[120]}});
    let decoded: RequestBody = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), old);
    let frame = Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: Uuid::new_v4(),
        request_id: 3,
        body: ResultBody::CredentialState(CredentialState {
            profile: 2,
            generation: Some(Uuid::new_v4()),
            installed: true,
        }),
    });
    frame.validate().unwrap();
}

#[test]
fn payload_bounds_redaction_and_ack_shape_are_explicit() {
    for raw in [
        vec![],
        vec![b'x'; MAX_CREDENTIAL + 1],
        b"bad\nvalue".to_vec(),
        vec![255],
    ] {
        assert!(CredentialPayload::new(&raw).is_err());
        assert!(serde_json::from_value::<CredentialPayload>(serde_json::json!(raw)).is_err());
    }
    let token = CredentialPayload::new(&vec![b'x'; MAX_CREDENTIAL]).unwrap();
    let f = Frame::Request(WireRequest {
        version: VERSION,
        boot_nonce: Uuid::new_v4(),
        request_id: 4,
        body: RequestBody::CredentialPublish {
            profile: CREDENTIAL_PROFILE,
            expected_generation: None,
            token: token.clone(),
        },
    });
    let wire = encode(&f).unwrap();
    assert!(wire.len() < MAX_FRAME);
    let Frame::Request(decoded) = serde_json::from_slice::<Frame>(&wire).unwrap() else {
        panic!("request")
    };
    let RequestBody::CredentialPublish { token: decoded, .. } = decoded.body else {
        panic!("payload")
    };
    assert_eq!(decoded.as_bytes(), token.as_bytes());
    assert!(!format!("{f:?}").contains(&format!("{:?}", token.as_bytes())));
    assert!(format!("{f:?}").contains("REDACTED"));
    let result = Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: Uuid::new_v4(),
        request_id: 4,
        body: ResultBody::CredentialState(CredentialState {
            profile: CREDENTIAL_PROFILE,
            generation: None,
            installed: true,
        }),
    });
    assert!(
        result.validate().is_err(),
        "an installed credential needs a generation"
    );
}
