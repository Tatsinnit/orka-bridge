#![cfg(unix)]
extern crate self as bridge_engine;
pub mod engine {
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../app/src-tauri/src/engine/types.rs"
        ));
    }
}
#[path = "../../../app/src-tauri/src/runtime/client.rs"]
pub mod client;
#[path = "../../../app/src-tauri/src/runtime/snapshot.rs"]
pub mod snapshot;
use bridge_runtime_protocol::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};
struct Observed {
    io: UnixStream,
    writes: Arc<Mutex<Vec<u8>>>,
}
impl Read for Observed {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.io.read(b)
    }
}
impl Write for Observed {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.writes.lock().unwrap().extend_from_slice(b);
        self.io.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.io.flush()
    }
}
fn receive(io: &mut BufReader<UnixStream>) -> WireRequest {
    let mut raw = Vec::new();
    io.read_until(b'\n', &mut raw).unwrap();
    let Frame::Request(r) = serde_json::from_slice(&raw).unwrap() else {
        panic!("request")
    };
    r
}
fn reply(io: &mut BufReader<UnixStream>, r: &WireRequest, body: ResultBody) {
    io.get_mut()
        .write_all(
            &encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: r.boot_nonce,
                request_id: r.request_id,
                body,
            }))
            .unwrap(),
        )
        .unwrap();
}
fn state(generation: Option<Uuid>, installed: bool) -> ResultBody {
    ResultBody::CredentialState(CredentialState {
        profile: CREDENTIAL_PROFILE,
        generation,
        installed,
    })
}
#[test]
fn negotiated_delivery_and_clear_have_only_metadata_acknowledgements() {
    let (a, b) = UnixStream::pair().unwrap();
    let id = Uuid::new_v4();
    let cleared = Uuid::new_v4();
    let peer = std::thread::spawn(move || {
        let mut io = BufReader::new(b);
        let r = receive(&mut io);
        assert!(matches!(r.body, RequestBody::CredentialStatus {}));
        reply(&mut io, &r, state(None, false));
        let r = receive(&mut io);
        let RequestBody::CredentialPublish {
            profile: CREDENTIAL_PROFILE,
            token,
            expected_generation,
        } = &r.body
        else {
            panic!("publish")
        };
        assert_eq!(token.as_bytes(), b"synthetic-private-marker");
        assert_eq!(*expected_generation, None);
        reply(&mut io, &r, state(Some(id), true));
        let r = receive(&mut io);
        assert!(matches!(r.body, RequestBody::CredentialStatus {}));
        reply(&mut io, &r, state(Some(id), true));
        let r = receive(&mut io);
        assert!(
            matches!(r.body,RequestBody::CredentialClear{expected_generation} if expected_generation==id)
        );
        reply(&mut io, &r, state(Some(cleared), false));
    });
    let mut client = client::VmClient::established(Box::new(a), Uuid::new_v4()).unwrap();
    let token = CredentialPayload::new(b"synthetic-private-marker").unwrap();
    assert_eq!(
        client.deliver_credential(&token, None).unwrap().generation,
        Some(id)
    );
    assert_eq!(
        client.clear_credential_file(id).unwrap().generation,
        Some(cleared)
    );
    peer.join().unwrap();
}
#[test]
fn profiles_can_switch_from_pristine_copilot_to_key_and_back_after_clear() {
    let (a, b) = UnixStream::pair().unwrap();
    let key_generation = Uuid::new_v4();
    let cleared = Uuid::new_v4();
    let copilot_generation = Uuid::new_v4();
    let peer = std::thread::spawn(move || {
        let mut io = BufReader::new(b);
        for (requested, before_profile, before, after) in [
            (2, 1, None, key_generation),
            (1, 2, Some(cleared), copilot_generation),
        ] {
            let r = receive(&mut io);
            assert!(matches!(r.body, RequestBody::CredentialStatus {}));
            reply(
                &mut io,
                &r,
                ResultBody::CredentialState(CredentialState {
                    profile: before_profile,
                    generation: before,
                    installed: false,
                }),
            );
            let r = receive(&mut io);
            let RequestBody::CredentialPublish {
                profile,
                expected_generation,
                token,
            } = &r.body
            else {
                panic!("expected publication")
            };
            assert_eq!((*profile, *expected_generation), (requested, before));
            assert!(token.as_bytes() == b"synthetic-private-marker");
            reply(
                &mut io,
                &r,
                ResultBody::CredentialState(CredentialState {
                    profile: requested,
                    generation: Some(after),
                    installed: true,
                }),
            );
            if requested == 2 {
                let r = receive(&mut io);
                assert!(matches!(r.body, RequestBody::CredentialStatus {}));
                reply(
                    &mut io,
                    &r,
                    ResultBody::CredentialState(CredentialState {
                        profile: 2,
                        generation: Some(after),
                        installed: true,
                    }),
                );
                let r = receive(&mut io);
                assert!(
                    matches!(r.body, RequestBody::CredentialClear { expected_generation } if expected_generation == after)
                );
                reply(
                    &mut io,
                    &r,
                    ResultBody::CredentialState(CredentialState {
                        profile: 2,
                        generation: Some(cleared),
                        installed: false,
                    }),
                );
            }
        }
    });
    let mut client = client::VmClient::established(Box::new(a), Uuid::new_v4()).unwrap();
    let secret = CredentialPayload::new(b"synthetic-private-marker").unwrap();
    assert_eq!(
        client
            .deliver_credential_for_profile(2, &secret, None)
            .unwrap()
            .generation,
        Some(key_generation)
    );
    assert_eq!(
        client
            .clear_credential_file(key_generation)
            .unwrap()
            .generation,
        Some(cleared)
    );
    assert_eq!(
        client
            .deliver_credential(&secret, Some(cleared))
            .unwrap()
            .generation,
        Some(copilot_generation)
    );
    peer.join().unwrap();
}

#[test]
fn delivery_rejects_an_ack_for_the_wrong_profile_and_poisoned_channel_cannot_replay() {
    let (a, b) = UnixStream::pair().unwrap();
    let peer = std::thread::spawn(move || {
        let mut io = BufReader::new(b);
        let r = receive(&mut io);
        reply(&mut io, &r, state(None, false));
        let r = receive(&mut io);
        assert!(matches!(
            r.body,
            RequestBody::CredentialPublish { profile: 1, .. }
        ));
        reply(
            &mut io,
            &r,
            ResultBody::CredentialState(CredentialState {
                profile: 2,
                generation: Some(Uuid::new_v4()),
                installed: true,
            }),
        );
    });
    let mut client = client::VmClient::established(Box::new(a), Uuid::new_v4()).unwrap();
    let secret = CredentialPayload::new(b"synthetic-private-marker").unwrap();
    assert!(client.deliver_credential(&secret, None).is_err());
    assert!(client.credential_status().is_err());
    peer.join().unwrap();
}

#[test]
fn unavailable_peer_gets_no_token_and_lost_ack_is_never_replayed() {
    for unavailable in [true, false] {
        let (a, b) = UnixStream::pair().unwrap();
        let peer = std::thread::spawn(move || {
            let mut io = BufReader::new(b);
            let r = receive(&mut io);
            assert!(matches!(r.body, RequestBody::CredentialStatus {}));
            if unavailable {
                reply(
                    &mut io,
                    &r,
                    ResultBody::Error {
                        code: ErrorCode::Unavailable,
                    },
                );
            } else {
                reply(&mut io, &r, state(None, false));
                let r = receive(&mut io);
                assert!(matches!(r.body, RequestBody::CredentialPublish { .. }));
            }
            // Closing without a publication ACK is an unknown outcome, not permission to resend.
        });
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut c = client::VmClient::established(
            Box::new(Observed {
                io: a,
                writes: writes.clone(),
            }),
            Uuid::new_v4(),
        )
        .unwrap();
        let token = CredentialPayload::new(b"synthetic-private-marker").unwrap();
        assert!(c.deliver_credential(&token, None).is_err());
        peer.join().unwrap();
        assert!(c.deliver_credential(&token, None).is_err());
        let writes = writes.lock().unwrap();
        let publishes = writes
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(|b| serde_json::from_slice::<Frame>(b).unwrap())
            .filter(|f| {
                matches!(
                    f,
                    Frame::Request(WireRequest {
                        body: RequestBody::CredentialPublish { .. },
                        ..
                    })
                )
            })
            .count();
        assert_eq!(
            publishes,
            usize::from(!unavailable),
            "observe actual write attempts, not just returned errors"
        );
    }
}
