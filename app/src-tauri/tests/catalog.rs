use bridge_engine::runtime::client::VmClient;
use bridge_runtime_protocol::*;
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};
struct Peer {
    nonce: Uuid,
    read: io::Cursor<Vec<u8>>,
    payload: Vec<u8>,
    seen: Arc<Mutex<Vec<EngineActionWire>>>,
    refuse: bool,
}
impl Read for Peer {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.read.read(b)
    }
}
impl Write for Peer {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        let Frame::Request(r) = serde_json::from_slice::<Frame>(b).unwrap() else {
            panic!("request")
        };
        let info = || OperationInfo {
            state: OperationState::Completed,
            exit: Some(0),
            bytes: self.payload.len(),
            has_diagnostics: false,
        };
        let body = match r.body {
            RequestBody::Execute {
                operation,
                action,
                snapshot,
            } => {
                assert_eq!(snapshot, None);
                self.seen.lock().unwrap().push(action);
                if self.refuse {
                    ResultBody::Error {
                        code: ErrorCode::Unavailable,
                    }
                } else {
                    ResultBody::Operation {
                        operation,
                        info: info(),
                    }
                }
            }
            RequestBody::Status { operation } => ResultBody::Operation {
                operation,
                info: info(),
            },
            RequestBody::ReadResult { offset, .. } => ResultBody::Chunk {
                data: encode_chunk(
                    &self.payload[offset..self.payload.len().min(offset + MAX_CHUNK)],
                ),
            },
            _ => panic!("catalog must issue only its fixed operation and result reads"),
        };
        self.read = io::Cursor::new(
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: self.nonce,
                request_id: r.request_id,
                body,
            }))
            .unwrap(),
        );
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn client(v: Value, refuse: bool) -> (VmClient, Arc<Mutex<Vec<EngineActionWire>>>) {
    let nonce = Uuid::new_v4();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let peer = Peer {
        nonce,
        read: io::Cursor::new(Vec::new()),
        payload: serde_json::to_vec(&v).unwrap(),
        seen: seen.clone(),
        refuse,
    };
    (VmClient::established(Box::new(peer), nonce).unwrap(), seen)
}
#[test]
fn catalog_uses_only_fixed_action_and_reads_all_result_chunks() {
    let tools: Vec<_> = (0..8)
        .map(|i| json!({"kind":"tool","name":format!("custom-{i}"),"description":"d".repeat(8192)}))
        .collect();
    let value = json!({"models":{"state":"ready","items":[{"id":"model:exact","pickerEnabled":false}]},"tools":{"state":"ready","items":tools}});
    let (mut c, seen) = client(value.clone(), false);
    assert_eq!(
        serde_json::to_value(c.workspace_catalog().unwrap()).unwrap(),
        value
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [EngineActionWire::WorkspaceCatalog {}]
    );
}
#[test]
fn catalog_refusal_and_malformed_projection_never_return_supplied_details() {
    let fixture = json!({"models":{"state":"ready","items":[]},"tools":{"state":"unavailable"}});
    let (mut c, _) = client(fixture.clone(), true);
    assert!(c.workspace_catalog().is_err());
    for value in [
        json!({"token":"synthetic-private"}),
        json!({"models":{"state":"unavailable","items":[]},"tools":{"state":"unavailable"}}),
        json!({"models":{"state":"ready","items":[{"id":"auto"}]},"tools":{"state":"unavailable"}}),
    ] {
        let (mut c, _) = client(value, false);
        assert!(c.workspace_catalog().is_err());
    }
}
