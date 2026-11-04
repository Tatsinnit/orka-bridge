use bridge_engine::runtime::{
    client::VmClient,
    snapshot::{AgentDocument, Snapshot},
};
use bridge_runtime_protocol::*;
use std::io::{self, Read, Write};
#[derive(Default)]
struct Broken;
impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}
impl Write for Broken {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn snapshot_is_immutable_and_bounded() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("a"), b"agent").unwrap();
    std::fs::write(t.path().join("t"), b"test").unwrap();
    let doc = AgentDocument {
        agent: "a".into(),
        tests: "t".into(),
    };
    let s = Snapshot::capture(t.path(), &doc).unwrap();
    std::fs::write(t.path().join("a"), b"edited").unwrap();
    assert_eq!(s.file(FileSlot::Agent), b"agent");
    assert_ne!(s.id(), Snapshot::capture(t.path(), &doc).unwrap().id());
    assert!(Snapshot::capture(
        t.path(),
        &AgentDocument {
            agent: "../outside".into(),
            tests: "t".into()
        }
    )
    .is_err());
    std::fs::write(t.path().join("a"), vec![0; MAX_FILE + 1]).unwrap();
    assert!(Snapshot::capture(t.path(), &doc).is_err());
}
#[test]
fn lost_response_remains_unknown_not_replayed() {
    let mut client = VmClient::established(Box::new(Broken), Uuid::new_v4()).unwrap();
    let id = client
        .execute(bridge_engine::engine::types::EngineAction::Version {}, None)
        .unwrap();
    assert_eq!(client.status(id).unwrap().state, OperationState::Unknown);
    assert!(client
        .execute(bridge_engine::engine::types::EngineAction::Version {}, None)
        .is_err());
}
#[test]
fn unknown_result_id_and_oversized_artifact_are_rejected() {
    struct Reply {
        read: std::io::Cursor<Vec<u8>>,
    }
    impl Read for Reply {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.read.read(b)
        }
    }
    impl Write for Reply {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let nonce = Uuid::new_v4();
    let bytes = encode(&Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: nonce,
        request_id: 42,
        body: ResultBody::Ready {},
    }))
    .unwrap();
    let mut c = VmClient::established(
        Box::new(Reply {
            read: std::io::Cursor::new(bytes),
        }),
        nonce,
    )
    .unwrap();
    assert!(c.status(Uuid::new_v4()).is_err());
    let t = tempfile::tempdir().unwrap();
    assert!(VmClient::store_result(t.path(), Uuid::new_v4(), &vec![0; MAX_FILE + 1]).is_err());
    assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
}
#[test]
fn validated_guest_errors_do_not_destroy_the_channel() {
    struct Reply(std::io::Cursor<Vec<u8>>);
    impl Read for Reply {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.0.read(b)
        }
    }
    impl Write for Reply {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let nonce = Uuid::new_v4();
    let mut bytes = Vec::new();
    for id in [3, 4] {
        bytes.extend(
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: nonce,
                request_id: id,
                body: ResultBody::Error {
                    code: ErrorCode::InvalidRequest,
                },
            }))
            .unwrap(),
        );
    }
    let mut c = VmClient::established(Box::new(Reply(std::io::Cursor::new(bytes))), nonce).unwrap();
    c.execute(bridge_engine::engine::types::EngineAction::Version {}, None)
        .unwrap();
    c.execute(bridge_engine::engine::types::EngineAction::Version {}, None)
        .unwrap();
}
#[test]
fn large_result_uses_buffered_channel_reads() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Count {
        data: std::io::Cursor<Vec<u8>>,
        reads: Arc<AtomicUsize>,
    }
    impl Read for Count {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.data.read(b)
        }
    }
    impl Write for Count {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let nonce = Uuid::new_v4();
    let operation = Uuid::new_v4();
    let mut bytes = encode(&Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: nonce,
        request_id: 3,
        body: ResultBody::Operation {
            operation,
            info: OperationInfo {
                state: OperationState::Completed,
                exit: Some(0),
                bytes: MAX_FILE,
                has_diagnostics: false,
            },
        },
    }))
    .unwrap();
    for n in 0..MAX_FILE / MAX_CHUNK {
        bytes.extend(
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: nonce,
                request_id: 4 + n as u64,
                body: ResultBody::Chunk {
                    data: encode_chunk(&vec![42; MAX_CHUNK]),
                },
            }))
            .unwrap(),
        );
    }
    let reads = Arc::new(AtomicUsize::new(0));
    let mut c = VmClient::established(
        Box::new(Count {
            data: std::io::Cursor::new(bytes),
            reads: reads.clone(),
        }),
        nonce,
    )
    .unwrap();
    assert_eq!(c.result(operation).unwrap().stdout.len(), MAX_FILE);
    assert!(reads.load(Ordering::Relaxed) < 1024);
}
#[cfg(unix)]
#[test]
fn snapshot_rejects_symlink() {
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("t"), b"test").unwrap();
    std::os::unix::fs::symlink("t", t.path().join("a")).unwrap();
    assert!(Snapshot::capture(
        t.path(),
        &AgentDocument {
            agent: "a".into(),
            tests: "t".into()
        }
    )
    .is_err());
}
