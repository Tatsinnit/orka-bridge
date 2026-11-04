use bridge_guest::{
    engine::{Operations, Runner},
    staging::Staging,
};
use bridge_runtime_protocol::*;
use std::io;
#[derive(Default)]
struct Fake(usize);
impl Runner for Fake {
    fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
        self.0 += 1;
        Ok(EngineResult {
            exit: 0,
            stdout: b"kmx dev\n".to_vec(),
            has_diagnostics: true,
        })
    }
}
#[test]
fn transfer_digest_commit_and_duplicate_execution() {
    let root = tempfile::tempdir().unwrap();
    let mut staging = Staging::new(root.path().join("snapshots")).unwrap();
    let bytes = b"agent";
    let id = SnapshotId::of(bytes, b"test");
    let upload = Uuid::new_v4();
    staging.begin(upload, id.clone()).unwrap();
    assert!(staging.commit(upload).is_err());
    staging.chunk(upload, FileSlot::Agent, 0, bytes).unwrap();
    assert!(staging.chunk(upload, FileSlot::Agent, 0, bytes).is_err());
    staging.chunk(upload, FileSlot::Tests, 0, b"test").unwrap();
    assert_eq!(staging.commit(upload).unwrap(), id);
    let mut ops = Operations::new(root.path().join("ops"), Fake::default()).unwrap();
    let op = Uuid::new_v4();
    assert_eq!(
        ops.execute(op, EngineActionWire::Version {}, Some(id.clone()), &staging)
            .unwrap(),
        OperationState::Completed
    );
    assert_eq!(
        ops.execute(op, EngineActionWire::Version {}, Some(id), &staging)
            .unwrap(),
        OperationState::Completed
    );
    assert_eq!(ops.runner().0, 1);
    assert_eq!(ops.read(op, 0).unwrap(), b"kmx dev\n");
    assert_eq!(ops.status(Uuid::new_v4()).unwrap(), OperationState::Unknown);
    let mut reopened = Operations::new(root.path().join("ops"), Fake::default()).unwrap();
    assert_eq!(reopened.status(op).unwrap(), OperationState::Completed);
    assert_eq!(
        reopened
            .execute(op, EngineActionWire::Version {}, None, &staging)
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}
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
#[test]
fn actual_client_crosses_bounded_guest_session() {
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), b"agent").unwrap();
    std::fs::write(root.path().join("t"), b"test").unwrap();
    let snapshot = snapshot::Snapshot::capture(
        root.path(),
        &snapshot::AgentDocument {
            agent: "a".into(),
            tests: "t".into(),
        },
    )
    .unwrap();
    let (host, mut guest) = UnixStream::pair().unwrap();
    let nonce = Uuid::new_v4();
    host.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let base = root.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let boot = BootDescriptor {
            installation_id: Uuid::new_v4(),
            generation: 1,
            boot_nonce: nonce,
            state_creation: StateCreation::Existing,
            state_fs_uuid: Uuid::nil(),
            state_bytes: STATE_BYTES,
            host_utc: 1,
        };
        let mut boot = boot;
        boot.state_fs_uuid = boot.installation_id;
        let mut session = bridge_guest::service::Session::local(boot).unwrap();
        let mut ops = Operations::new(base.join("ops"), Fake::default()).unwrap();
        let mut staging = Staging::new(base.join("stage")).unwrap();
        loop {
            let mut b = [0];
            if guest.read_exact(&mut b).is_err() {
                break;
            }
            if let Some(request) = session.push(b[0]).unwrap() {
                let body = ops.handle(&mut staging, &request.body).unwrap();
                guest
                    .write_all(&encode(&bridge_guest::service::result(&request, body)).unwrap())
                    .unwrap();
            }
        }
        assert_eq!(ops.runner().0, 1);
    });
    let mut c = client::VmClient::established(Box::new(host), nonce).unwrap();
    let s = c.stage_snapshot(&snapshot).unwrap();
    let id = c
        .execute(engine::types::EngineAction::Version {}, Some(s))
        .unwrap();
    assert_eq!(c.result(id).unwrap().stdout, b"kmx dev\n");
    drop(c);
    worker.join().unwrap();
}
#[test]
fn fixed_argv_preserves_spaces_and_scope() {
    assert_eq!(
        bridge_guest::engine::argv(&EngineActionWire::ListAgents {
            context: "kind owned".into(),
            namespace: "space ns".into()
        })
        .unwrap(),
        [
            "cluster",
            "orka",
            "list",
            "--context",
            "kind owned",
            "--namespace",
            "space ns",
            "-o",
            "json"
        ]
    );
}
#[test]
fn raw_agent_spec_and_diagnostics_are_not_persisted() {
    struct List;
    impl Runner for List {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            Ok(EngineResult{exit:0,stdout:br#"{"apiVersion":"v1","kind":"List","items":[{"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":{"name":"a","namespace":"n","uid":"u"},"spec":{"secret":"NEVER-PERSIST"}}]}"#.to_vec(),has_diagnostics:true})
        }
    }
    let root = tempfile::tempdir().unwrap();
    let stage = Staging::new(root.path().join("stage")).unwrap();
    let mut ops = Operations::new(root.path().join("ops"), List).unwrap();
    let id = Uuid::new_v4();
    ops.execute(
        id,
        EngineActionWire::ListAgents {
            context: "c".into(),
            namespace: "n".into(),
        },
        None,
        &stage,
    )
    .unwrap();
    let output = ops.read(id, 0).unwrap();
    assert!(!String::from_utf8_lossy(&output).contains("NEVER-PERSIST"));
    assert!(!String::from_utf8_lossy(
        &std::fs::read(root.path().join("ops").join(id.to_string())).unwrap()
    )
    .contains("NEVER-PERSIST"));
}
#[test]
fn uncertain_and_invalid_results_are_not_replayed() {
    struct Uncertain(usize);
    impl Runner for Uncertain {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            self.0 += 1;
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }
    let t = tempfile::tempdir().unwrap();
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(t.path().join("ops"), Uncertain(0)).unwrap();
    let id = Uuid::new_v4();
    for _ in 0..2 {
        assert_eq!(
            ops.execute(id, EngineActionWire::Version {}, None, &s)
                .unwrap(),
            OperationState::Unknown
        )
    }
    assert_eq!(ops.runner().0, 1);
    drop(ops);
    let ops = Operations::new(t.path().join("ops"), Fake::default()).unwrap();
    assert_eq!(ops.status(id).unwrap(), OperationState::Unknown);
    struct Truncated;
    impl Runner for Truncated {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            Ok(EngineResult {
                exit: 0,
                stdout: b"{".to_vec(),
                has_diagnostics: true,
            })
        }
    }
    let mut ops = Operations::new(t.path().join("invalid"), Truncated).unwrap();
    let id = Uuid::new_v4();
    assert_eq!(
        ops.execute(
            id,
            EngineActionWire::ListAgents {
                context: "c".into(),
                namespace: "n".into()
            },
            None,
            &s
        )
        .unwrap(),
        OperationState::Failed
    );
    assert!(ops.read(id, 0).unwrap().is_empty());
}
#[test]
fn bounded_payload_caches_do_not_limit_lifetime_reads_or_allow_replay() {
    let t = tempfile::tempdir().unwrap();
    let mut s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(t.path().join("ops"), Fake::default()).unwrap();
    let old = Uuid::new_v4();
    ops.execute(old, EngineActionWire::Version {}, None, &s)
        .unwrap();
    ops.read(old, 0).unwrap();
    for n in 0..MAX_OPERATIONS + 2 {
        let id = Uuid::new_v4();
        ops.execute(id, EngineActionWire::Version {}, None, &s)
            .unwrap();
        let upload = Uuid::new_v4();
        s.begin(upload, SnapshotId::of(&n.to_le_bytes(), b"t"))
            .unwrap();
        s.chunk(upload, FileSlot::Agent, 0, &n.to_le_bytes())
            .unwrap();
        s.chunk(upload, FileSlot::Tests, 0, b"t").unwrap();
        s.commit(upload).unwrap();
    }
    let count = ops.runner().0;
    ops.execute(old, EngineActionWire::Version {}, None, &s)
        .unwrap();
    assert_eq!(ops.runner().0, count);
    assert!(
        std::fs::read_dir(t.path().join("ops/results"))
            .unwrap()
            .count()
            <= MAX_OPERATIONS
    );
    assert!(std::fs::read_dir(t.path().join("stage")).unwrap().count() <= MAX_OPERATIONS);
}
#[test]
fn failed_eviction_never_leaves_a_partial_committed_name() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("stage");
    let mut s = Staging::new(root.clone()).unwrap();
    let mut keys = Vec::new();
    for n in 0..MAX_OPERATIONS {
        let id = SnapshotId::of(&n.to_le_bytes(), b"t");
        let upload = Uuid::new_v4();
        s.begin(upload, id.clone()).unwrap();
        s.chunk(upload, FileSlot::Agent, 0, &n.to_le_bytes())
            .unwrap();
        s.chunk(upload, FileSlot::Tests, 0, b"t").unwrap();
        s.commit(upload).unwrap();
        std::fs::write(root.join(id.key()).join("retain"), b"extra").unwrap();
        keys.push(id.key());
    }
    let upload = Uuid::new_v4();
    s.begin(upload, SnapshotId::of(b"new", b"t")).unwrap();
    s.chunk(upload, FileSlot::Agent, 0, b"new").unwrap();
    s.chunk(upload, FileSlot::Tests, 0, b"t").unwrap();
    assert!(s.commit(upload).is_err());
    assert!(keys.iter().any(|key| !root.join(key).exists()));
}
#[test]
fn version_projection_preserves_the_diagnostic_signal_not_raw_annotations() {
    struct Version;
    impl Runner for Version {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            Ok(EngineResult {
                exit: 0,
                stdout: b"kmx dev (annotation)\nprivate diagnostic\n".to_vec(),
                has_diagnostics: false,
            })
        }
    }
    let t = tempfile::tempdir().unwrap();
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(t.path().join("ops"), Version).unwrap();
    let id = Uuid::new_v4();
    ops.execute(id, EngineActionWire::Version {}, None, &s)
        .unwrap();
    assert!(ops.info(id).unwrap().has_diagnostics);
    assert_eq!(ops.read(id, 0).unwrap(), b"kmx dev\n");
}
#[test]
fn malformed_pagination_never_becomes_an_empty_success() {
    struct List;
    impl Runner for List {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            Ok(EngineResult{exit:0,stdout:br#"{"apiVersion":"v1","kind":"List","metadata":{"remainingItemCount":"2"},"items":[]}"#.to_vec(),has_diagnostics:false})
        }
    }
    let t = tempfile::tempdir().unwrap();
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(t.path().join("ops"), List).unwrap();
    assert_eq!(
        ops.execute(
            Uuid::new_v4(),
            EngineActionWire::ListAgents {
                context: "c".into(),
                namespace: "n".into()
            },
            None,
            &s
        )
        .unwrap(),
        OperationState::Failed
    );
}
#[test]
fn pagination_types_match_the_delivered_typed_decoder() {
    struct List(serde_json::Value);
    impl Runner for List {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            Ok(EngineResult{exit:0,stdout:serde_json::to_vec(&serde_json::json!({"apiVersion":"v1","kind":"List","metadata":self.0,"items":[]})).unwrap(),has_diagnostics:false})
        }
    }
    for metadata in [
        serde_json::json!("wrong"),
        serde_json::json!([]),
        serde_json::json!({"continue":4}),
        serde_json::json!({"remainingItemCount":-1}),
        serde_json::json!({"remainingItemCount":0.5}),
    ] {
        let t = tempfile::tempdir().unwrap();
        let s = Staging::new(t.path().join("stage")).unwrap();
        let mut ops = Operations::new(t.path().join("ops"), List(metadata)).unwrap();
        assert_eq!(
            ops.execute(
                Uuid::new_v4(),
                EngineActionWire::ListAgents {
                    context: "c".into(),
                    namespace: "n".into()
                },
                None,
                &s
            )
            .unwrap(),
            OperationState::Failed
        );
    }
}
#[test]
fn malformed_version_is_failed_before_persistence() {
    struct Invalid;
    impl Runner for Invalid {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            Ok(EngineResult {
                exit: 0,
                stdout: b"kmx not-a-version\n".to_vec(),
                has_diagnostics: false,
            })
        }
    }
    let t = tempfile::tempdir().unwrap();
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(t.path().join("ops"), Invalid).unwrap();
    assert_eq!(
        ops.execute(Uuid::new_v4(), EngineActionWire::Version {}, None, &s)
            .unwrap(),
        OperationState::Failed
    );
}
#[test]
fn active_record_ignores_truncated_payload_and_never_reexecutes() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("ops");
    let id = Uuid::new_v4();
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(root.clone(), Fake::default()).unwrap();
    std::fs::write(root.join(id.to_string()),serde_json::to_vec(&serde_json::json!({"action":{"type":"version"},"snapshot":null,"state":"running","result":null})).unwrap()).unwrap();
    std::fs::write(root.join("results").join(id.to_string()), b"{").unwrap();
    assert_eq!(ops.status(id).unwrap(), OperationState::Unknown);
    assert_eq!(
        ops.execute(id, EngineActionWire::Version {}, None, &s)
            .unwrap(),
        OperationState::Unknown
    );
    assert_eq!(ops.runner().0, 0);
}
#[test]
fn repeated_result_chunks_use_one_bounded_decoded_payload() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("ops");
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(root.clone(), Fake::default()).unwrap();
    let id = Uuid::new_v4();
    ops.execute(id, EngineActionWire::Version {}, None, &s)
        .unwrap();
    let first = ops.read(id, 0).unwrap();
    std::fs::write(root.join("results").join(id.to_string()), b"{").unwrap();
    assert_eq!(ops.read(id, 0).unwrap(), first);
}
#[test]
fn payload_write_failure_never_publishes_a_partial_final_name() {
    struct Fail {
        path: std::path::PathBuf,
        calls: usize,
    }
    impl Runner for Fail {
        fn execute(&mut self, _: &EngineActionWire) -> io::Result<EngineResult> {
            self.calls += 1;
            std::fs::write(&self.path, b"{")?;
            Ok(EngineResult {
                exit: 0,
                stdout: b"kmx dev\n".to_vec(),
                has_diagnostics: false,
            })
        }
    }
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("ops");
    let id = Uuid::new_v4();
    let s = Staging::new(t.path().join("stage")).unwrap();
    let mut ops = Operations::new(
        root.clone(),
        Fail {
            path: root
                .join("results")
                .join(id.to_string())
                .with_extension("tmp"),
            calls: 0,
        },
    )
    .unwrap();
    assert!(ops
        .execute(id, EngineActionWire::Version {}, None, &s)
        .is_err());
    assert!(!root.join("results").join(id.to_string()).exists());
    assert_eq!(ops.status(id).unwrap(), OperationState::Unknown);
    assert_eq!(
        ops.execute(id, EngineActionWire::Version {}, None, &s)
            .unwrap(),
        OperationState::Unknown
    );
    assert_eq!(ops.runner().calls, 1);
}
#[test]
fn wrong_digest_and_oversize_never_commit() {
    let root = tempfile::tempdir().unwrap();
    let mut s = Staging::new(root.path().join("snapshots")).unwrap();
    let u = Uuid::new_v4();
    s.begin(u, SnapshotId::of(b"right", b"")).unwrap();
    s.chunk(u, FileSlot::Agent, 0, b"wrong").unwrap();
    assert!(s.commit(u).is_err());
    assert!(s
        .chunk(u, FileSlot::Agent, 5, &vec![0; MAX_CHUNK + 1])
        .is_err());
}
