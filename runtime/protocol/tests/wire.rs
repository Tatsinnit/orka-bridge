use bridge_runtime_protocol::*;
use serde_json::json;

fn nonce(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn hello(stage: Stage, accepted: Option<Uuid>, guest: u128) -> Frame {
    Frame::Hello(StageHello {
        version: VERSION,
        stage,
        guest_nonce: nonce(guest),
        accepted_boot_nonce: accepted,
    })
}
fn parse(decoder: &mut Decoder, bytes: &[u8]) -> Result<Option<Frame>, ProtocolError> {
    let mut result = None;
    for byte in bytes {
        if let Some(frame) = decoder.push(*byte)? {
            result = Some(frame);
        }
    }
    Ok(result)
}
#[test]
fn hello_wire_fixture_and_strict_major_one_fields() {
    let value = json!({"type":"hello","body":{"version":1,"stage":"early","guestNonce":nonce(2),"acceptedBootNonce":null}});
    let mut d = Decoder::new(Stage::Early, nonce(1));
    assert_eq!(
        parse(&mut d, format!("{value}\n").as_bytes()).unwrap(),
        Some(hello(Stage::Early, None, 2))
    );
    for changed in [
        json!({"type":"hello","body":{"version":2,"stage":"early","guestNonce":nonce(2),"acceptedBootNonce":null}}),
        json!({"type":"hello","body":{"version":1,"stage":"early","guestNonce":nonce(2),"acceptedBootNonce":null,"shell":"id"}}),
        json!({"type":"shell","body":{}}),
    ] {
        assert!(parse(
            &mut Decoder::new(Stage::Early, nonce(1)),
            format!("{changed}\n").as_bytes()
        )
        .is_err());
    }
}
#[test]
fn exact_frame_bound_and_invalid_utf8_fail_closed() {
    let frame = hello(Stage::Early, None, 2);
    let mut wire = encode(&frame).unwrap();
    wire.pop();
    wire.resize(MAX_FRAME - 1, b' ');
    wire.push(b'\n');
    assert!(parse(&mut Decoder::new(Stage::Early, nonce(1)), &wire)
        .unwrap()
        .is_some());
    wire.insert(wire.len() - 1, b' ');
    assert_eq!(
        parse(&mut Decoder::new(Stage::Early, nonce(1)), &wire),
        Err(ProtocolError::Oversize)
    );
    assert_eq!(
        parse(&mut Decoder::new(Stage::Early, nonce(1)), b"\xff\n"),
        Err(ProtocolError::Invalid)
    );
}
#[test]
fn decoded_chunk_boundary_is_enforced() {
    let allowed = "AAAA".repeat(MAX_CHUNK / 3) + "AAA=";
    let frame = |data: String| {
        Frame::Request(WireRequest {
            version: VERSION,
            boot_nonce: nonce(1),
            request_id: 1,
            body: RequestBody::Chunk { data },
        })
    };
    assert!(encode(&frame(allowed)).is_ok());
    assert!(encode(&frame("A".repeat(4 * ((MAX_CHUNK + 3) / 3)))).is_err());
    assert!(encode(&frame("not base64!".into())).is_err());
}
#[test]
fn stage_handoff_discards_fragments_and_binds_nonce() {
    let mut d = Decoder::new(Stage::Early, nonce(1));
    parse(&mut d, &encode(&hello(Stage::Early, None, 2)).unwrap()).unwrap();
    parse(&mut d, b"{\"type\":\"request\"").unwrap();
    d.begin_stage(Stage::Serve);
    assert_eq!(
        parse(
            &mut d,
            &encode(&hello(Stage::Serve, Some(nonce(1)), 3)).unwrap()
        )
        .unwrap(),
        Some(hello(Stage::Serve, Some(nonce(1)), 3))
    );
    for bad in [
        hello(Stage::Early, None, 3),
        hello(Stage::Serve, Some(nonce(99)), 3),
        hello(Stage::Serve, None, 3),
    ] {
        assert!(parse(
            &mut Decoder::new(Stage::Serve, nonce(1)),
            &encode(&bad).unwrap()
        )
        .is_err());
    }
    let mut d = Decoder::new(Stage::Early, nonce(1));
    parse(&mut d, &encode(&hello(Stage::Early, None, 2)).unwrap()).unwrap();
    d.begin_stage(Stage::Serve);
    assert!(parse(
        &mut d,
        &encode(&hello(Stage::Serve, Some(nonce(1)), 2)).unwrap()
    )
    .is_err());
}
#[test]
fn host_rejects_guest_sent_requests_after_valid_hello() {
    let mut d = Decoder::new(Stage::Early, nonce(1));
    parse(&mut d, &encode(&hello(Stage::Early, None, 2)).unwrap()).unwrap();
    let request = Frame::Request(WireRequest {
        version: VERSION,
        boot_nonce: nonce(1),
        request_id: 1,
        body: RequestBody::Shutdown {},
    });
    assert_eq!(
        parse(&mut d, &encode(&request).unwrap()),
        Err(ProtocolError::Stage)
    );
}
#[test]
fn engine_wire_refuses_unknown_fields_inside_actions() {
    let value = json!({"type":"request","body":{"version":1,"bootNonce":nonce(1),"requestId":1,"body":{"op":"engine","body":{"type":"version","argv":["shell"]}}}});
    let mut valid = value.clone();
    valid["body"]["body"]["body"]
        .as_object_mut()
        .unwrap()
        .remove("argv");
    assert!(serde_json::from_value::<Frame>(valid).is_ok());
    assert!(serde_json::from_value::<Frame>(value).is_err());
}
#[test]
fn results_need_stage_hello_and_current_lease_nonce() {
    let request = |boot_nonce| {
        Frame::Result(WireResult {
            version: VERSION,
            boot_nonce,
            request_id: 1,
            body: ResultBody::Ready {},
        })
    };
    let mut d = Decoder::new(Stage::Serve, nonce(1));
    assert_eq!(
        parse(&mut d, &encode(&request(nonce(1))).unwrap()),
        Err(ProtocolError::Stage)
    );
    let mut d = Decoder::new(Stage::Serve, nonce(1));
    parse(
        &mut d,
        &encode(&hello(Stage::Serve, Some(nonce(1)), 3)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        parse(&mut d, &encode(&request(nonce(99))).unwrap()),
        Err(ProtocolError::Nonce)
    );
}
