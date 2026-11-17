use bridge_engine::engine::types::EngineAction;
use bridge_engine::runtime as fixture_runtime;
use bridge_engine::runtime::{journal::*, types::*, wire::*, RuntimeState, RuntimeWorker};
use bridge_runtime_protocol::*;
use std::{
    fs,
    sync::{Arc, Mutex},
};
use tempfile::tempdir;
#[path = "support/runtime_image_fixture.rs"]
mod image_fixture;

fn make_worker(backend: Box<dyn VmBackend>, journal: Journal) -> RuntimeWorker {
    let image = {
        let target = journal.image_target("fixture-state-v1").unwrap();
        fixture_runtime::image_install::open_installed_image(&target).unwrap()
    };
    RuntimeWorker::new(backend, journal, Arc::new(image)).unwrap()
}

fn create() -> (tempfile::TempDir, Journal) {
    let temp = tempdir().unwrap();
    let verified = image_fixture::verified(
        temp.path(),
        fixture_runtime::image_types::ImageTarget::MacosArm64,
    );
    let journal = Journal::create(
        &temp.path().join("install"),
        BackendKind::Apple,
        vec![1, 2, 3],
    )
    .unwrap();
    install_image(&journal, &verified);
    (temp, journal)
}
fn install_image(journal: &Journal, verified: &fixture_runtime::image_types::VerifiedImageSet) {
    let target = journal
        .image_target(&verified.image().identity().state_compatibility_id)
        .unwrap();
    let (_sender, closed) = tokio::sync::watch::channel(false);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fixture_runtime::image_install::materialize_image(
            verified,
            &target,
            closed,
            |_| {},
        ))
        .unwrap();
}
#[test]
fn buffered_boot_handoff_preserves_ready2_read_ahead_for_the_client() {
    buffered_handoff(false);
    buffered_handoff(true);
}
fn buffered_handoff(cleanup: bool) {
    use std::io::{Read, Write};
    struct Channel {
        bytes: std::io::Cursor<Vec<u8>>,
        writes: Arc<Mutex<Vec<u8>>>,
    }
    impl Read for Channel {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.bytes.read(b)
        }
    }
    impl Write for Channel {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.writes.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (_temp, journal) = create();
    let mut worker = make_worker(Box::new(Backend::default()), journal);
    worker.start().unwrap();
    let nonce = worker.lease().unwrap().spec.boot_nonce;
    let mut bytes = Vec::new();
    for (stage, id, guest) in [
        (Stage::Early, 1, Uuid::new_v4()),
        (Stage::Serve, 2, Uuid::new_v4()),
    ] {
        bytes.extend(
            encode(&Frame::Hello(StageHello {
                version: VERSION,
                stage,
                guest_nonce: guest,
                run_cleanup: cleanup && stage == Stage::Serve,
                accepted_boot_nonce: if stage == Stage::Serve {
                    Some(nonce)
                } else {
                    None
                },
            }))
            .unwrap(),
        );
        bytes.extend(
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: nonce,
                request_id: id,
                body: ResultBody::Ready {},
            }))
            .unwrap(),
        );
    }
    bytes.extend(
        encode(&Frame::Result(WireResult {
            version: VERSION,
            boot_nonce: nonce,
            request_id: 3,
            body: ResultBody::Infrastructure(InfrastructureState {
                phase: Some(BootstrapPhase::ClusterReady),
                service: InfrastructureService::Starting,
            }),
        }))
        .unwrap(),
    );
    let writes = Arc::new(Mutex::new(Vec::new()));
    let mut client = fixture_runtime::handshake::establish(
        &mut worker,
        Box::new(Channel {
            bytes: std::io::Cursor::new(bytes),
            writes: writes.clone(),
        }),
        123,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(client.readiness().unwrap()).unwrap(),
        serde_json::json!({"phase": "cluster-ready", "service": "starting"})
    );
    let error = client.cleanup_run_agent("old-uid").unwrap_err();
    assert_eq!(error.kind() == std::io::ErrorKind::Unsupported, !cleanup);
    let frames: Vec<Frame> = writes
        .lock()
        .unwrap()
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect();
    assert!(matches!(
        &frames[0],
        Frame::Request(WireRequest {
            request_id: 1,
            body: RequestBody::Boot(BootDescriptor {
                state_creation: StateCreation::New,
                ..
            }),
            ..
        })
    ));
    assert!(matches!(
        &frames[1],
        Frame::Request(WireRequest {
            request_id: 2,
            body: RequestBody::Boot(BootDescriptor {
                state_creation: StateCreation::Existing,
                ..
            }),
            ..
        })
    ));
    assert!(matches!(
        &frames[2],
        Frame::Request(WireRequest {
            request_id: 3,
            body: RequestBody::Readiness {},
            ..
        })
    ));
}
// Mutations must not let a failed exchange retain a live backend or issue New twice.
fn check_failed_handshake(case: &str, stage: Stage) {
    use std::io::{Read, Write};
    struct Channel {
        bytes: std::io::Cursor<Vec<u8>>,
        writes: Arc<Mutex<Vec<u8>>>,
        root: std::path::PathBuf,
        fail_write: bool,
    }
    impl Read for Channel {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.bytes.read(b)
        }
    }
    impl Write for Channel {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            // Capture the attempted descriptor even when no bytes reach the guest.
            self.writes.lock().unwrap().extend_from_slice(b);
            if self.fail_write {
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(self.root.join("record.json")).unwrap())
                        .unwrap();
                assert_eq!(record["phase"], "consumed");
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let transcript = |nonce, bad: bool| {
        let mut bytes = Vec::new();
        for (current, id, guest) in [(Stage::Early, 1, 3), (Stage::Serve, 2, 4)] {
            let reject = bad && current == stage;
            bytes.extend(
                encode(&Frame::Hello(hello(
                    current,
                    nonce,
                    if reject && case == "repeated-guest" {
                        3
                    } else {
                        guest
                    },
                )))
                .unwrap(),
            );
            if reject && matches!(case, "repeated-guest" | "boot-write") {
                break;
            }
            let mut reply = WireResult {
                version: VERSION,
                boot_nonce: nonce,
                request_id: id,
                body: ResultBody::Ready {},
            };
            if reject {
                match case {
                    "id" => reply.request_id = 99,
                    "version" => reply.version = VERSION + 1,
                    "nonce" => reply.boot_nonce = Uuid::from_u128(99),
                    "body" => {
                        reply.body = ResultBody::Error {
                            code: ErrorCode::Unavailable,
                        }
                    }
                    _ => {}
                }
            }
            // Bypass encode validation only to deliver deliberately invalid wire input.
            let mut frame = serde_json::to_vec(&Frame::Result(reply)).unwrap();
            frame.push(b'\n');
            if reject && case == "truncated" {
                frame.pop();
            }
            if reject && case == "oversized" {
                frame.pop();
                frame.resize(MAX_FRAME, b' ');
                frame.push(b'\n');
            }
            bytes.extend(frame);
            if reject {
                break;
            }
        }
        bytes
    };
    let (temp, journal) = create();
    let root = temp.path().join("install");
    let backend = Backend {
        stop_root: Some(root.clone()),
        ..Backend::default()
    };
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), journal);
    worker.start().unwrap();
    let nonce = worker.lease().unwrap().spec.boot_nonce;
    let writes = Arc::new(Mutex::new(Vec::new()));
    let result = fixture_runtime::handshake::establish(
        &mut worker,
        Box::new(Channel {
            bytes: std::io::Cursor::new(transcript(nonce, true)),
            writes: writes.clone(),
            root: root.clone(),
            fail_write: case == "boot-write",
        }),
        123,
    );
    assert!(result.is_err(), "{case} at {stage:?} accepted");
    if case == "boot-write" {
        assert!(
            matches!(result, Err(RuntimeError::Io(e)) if e.kind() == std::io::ErrorKind::BrokenPipe)
        );
    }
    assert!(
        worker.lease().is_none(),
        "{case} at {stage:?} retained lease"
    );
    assert_eq!(*events.lock().unwrap(), vec!["create", "start", "stop"]);
    let requests = |writes: &Arc<Mutex<Vec<u8>>>| -> Vec<WireRequest> {
        writes
            .lock()
            .unwrap()
            .split(|b| *b == b'\n')
            .filter(|b| !b.is_empty())
            .map(|b| match serde_json::from_slice(b).unwrap() {
                Frame::Request(request) => request,
                _ => panic!("host sent a non-request"),
            })
            .collect()
    };
    let attempted = requests(&writes);
    assert_eq!(
        attempted.len(),
        if stage == Stage::Early || case == "repeated-guest" {
            1
        } else {
            2
        }
    );
    assert!(matches!(
        &attempted[0].body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::New,
            ..
        })
    ));
    if attempted.len() == 2 {
        assert!(matches!(
            &attempted[1].body,
            RequestBody::Boot(BootDescriptor {
                state_creation: StateCreation::Existing,
                ..
            })
        ));
    }
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("record.json")).unwrap()).unwrap();
    assert_eq!(record["phase"], "consumed");
    assert!(matches!(Journal::open(&root), Err(RuntimeError::Busy)));
    // Reuse the actual held owner for the next boot and verify its wire
    // descriptor. Immediate cross-process lock release is a separate #79 case,
    // not a proxy for whether New was durably consumed before delivery.
    worker.start().unwrap();
    let next = worker.lease().unwrap().spec.boot_nonce;
    assert_ne!(next, nonce);
    let writes = Arc::new(Mutex::new(Vec::new()));
    let client = fixture_runtime::handshake::establish(
        &mut worker,
        Box::new(Channel {
            bytes: std::io::Cursor::new(transcript(next, false)),
            writes: writes.clone(),
            root: root.clone(),
            fail_write: false,
        }),
        123,
    )
    .unwrap();
    let retried = requests(&writes);
    assert_eq!(retried.len(), 2);
    assert!(retried.iter().all(|r| matches!(
        &r.body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::Existing,
            ..
        })
    )));
    drop(client);
    worker.stop().unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec!["create", "start", "stop", "start", "stop"]
    );
}

#[test]
fn handshake_negative_ready_fields_stop_backend_without_new_retry() {
    for stage in [Stage::Early, Stage::Serve] {
        for case in ["id", "version", "nonce", "body"] {
            check_failed_handshake(case, stage);
        }
    }
}

#[test]
fn handshake_negative_repeated_nonce_and_frames_stop_backend_without_new_retry() {
    check_failed_handshake("repeated-guest", Stage::Serve);
    for stage in [Stage::Early, Stage::Serve] {
        for case in ["truncated", "oversized"] {
            check_failed_handshake(case, stage);
        }
    }
}

#[test]
fn handshake_negative_first_boot_write_keeps_new_consumed_after_cleanup() {
    check_failed_handshake("boot-write", Stage::Early);
}

#[test]
fn readiness_refusal_keeps_the_sequenced_channel_open() {
    use std::io::{Read, Write};
    struct Channel(std::io::Cursor<Vec<u8>>);
    impl Read for Channel {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }
    impl Write for Channel {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let nonce = Uuid::new_v4();
    let mut bytes = Vec::new();
    for (request_id, body) in [
        (
            3,
            ResultBody::Error {
                code: ErrorCode::Unavailable,
            },
        ),
        (
            4,
            ResultBody::Infrastructure(InfrastructureState {
                phase: Some(BootstrapPhase::InfrastructureReady),
                service: InfrastructureService::Ready,
            }),
        ),
        (5, ResultBody::Ready {}),
    ] {
        bytes.extend(
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: nonce,
                request_id,
                body,
            }))
            .unwrap(),
        );
    }
    let mut client = fixture_runtime::client::VmClient::established(
        Box::new(Channel(std::io::Cursor::new(bytes))),
        nonce,
    )
    .unwrap();
    assert!(client.readiness().is_err());
    assert_eq!(
        serde_json::to_value(client.readiness().unwrap()).unwrap(),
        serde_json::json!({"phase": "infrastructure-ready", "service": "ready"})
    );
    assert!(
        client.readiness().is_err(),
        "legacy storage Ready cannot unlock Orka"
    );
}
#[test]
fn exclusive_directory_lock_and_persistent_random_identity() {
    let (temp, j) = create();
    let root = temp.path().join("install");
    let id = j.record().installation_id;
    assert!(
        matches!(Journal::create(&root, BackendKind::Apple, vec![1]), Err(RuntimeError::Io(e)) if e.kind()==std::io::ErrorKind::AlreadyExists)
    );
    assert!(matches!(Journal::open(&root), Err(RuntimeError::Busy)));
    let mac = j.record().mac;
    assert_eq!(mac[0] & 3, 2);
    drop(j);
    let j = Journal::open(&root).unwrap();
    assert_eq!(j.record().installation_id, id);
    assert_eq!(j.record().mac, mac);
    let (_other, other) = create();
    assert_ne!(other.record().installation_id, id);
    assert_ne!(other.record().mac, mac);
}
#[cfg(unix)]
#[test]
fn concurrent_spawn_cannot_extend_dropped_journal_ownership() {
    use std::{
        io::{Read, Write},
        os::unix::{net::UnixStream, process::CommandExt},
    };
    let (temp, journal) = create();
    let mut worker = make_worker(Box::new(Backend::default()), journal);
    worker.start().unwrap();
    let (parent, child) = UnixStream::pair().unwrap();
    child
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.arg("--list").stdout(std::process::Stdio::null());
    // SAFETY: the child only reads and writes its socket before exec.
    unsafe {
        command.pre_exec(move || {
            (&child).write_all(b"f")?;
            (&child).read_exact(&mut [0])
        });
    }
    let spawner = std::thread::spawn(move || command.status().unwrap());
    // Pause this spawn between fork and exec, like any concurrent app spawn:
    // it shares every open descriptor.
    (&parent).read_exact(&mut [0]).unwrap();
    drop(worker);
    let reopened = Journal::open(&temp.path().join("install"));
    (&parent).write_all(b"x").unwrap();
    assert!(spawner.join().unwrap().success());
    reopened.unwrap();
}
#[test]
fn missing_replaced_or_unjournaled_state_is_never_recreated() {
    let (temp, j) = create();
    let root = temp.path().join("install");
    drop(j);
    fs::rename(root.join("state.disk"), root.join("retained.disk")).unwrap();
    assert!(
        matches!(Journal::open(&root), Err(RuntimeError::Io(e)) if e.kind()==std::io::ErrorKind::NotFound)
    );
    assert!(!root.join("state.disk").exists());
    fs::write(root.join("state.disk"), b"unknown state").unwrap();
    assert!(matches!(
        Journal::open(&root),
        Err(RuntimeError::RetainedState)
    ));
    assert_eq!(fs::read(root.join("state.disk")).unwrap(), b"unknown state");
}
#[test]
fn nonempty_uninitialized_owned_file_is_retained_before_backend_can_touch_it() {
    let (temp, journal) = create();
    let state = temp.path().join("install/state.disk");
    fs::write(&state, b"retained unknown state").unwrap();
    let backend = Backend::default();
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), journal);
    assert!(matches!(worker.start(), Err(RuntimeError::RetainedState)));
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(fs::read(state).unwrap(), b"retained unknown state");
}

#[test]
fn single_component_relative_root_is_normalized_before_creation() {
    let current = std::env::current_dir().unwrap();
    let temp = tempfile::tempdir_in(&current).unwrap();
    let relative = std::path::Path::new(temp.path().file_name().unwrap());
    fs::remove_dir(temp.path()).unwrap();
    let journal = Journal::create(relative, BackendKind::Apple, vec![1]).unwrap();
    assert!(journal.record().installation_id != Uuid::nil());
    drop(journal);
    assert!(Journal::open(relative).is_ok());
}

#[test]
fn handshake_stop_failure_is_typed_even_if_a_later_stop_succeeds() {
    let (_temp, journal) = create();
    let backend = Backend::default();
    let fail = backend.fail_stop.clone();
    let mut worker = make_worker(Box::new(backend), journal);
    worker.start().unwrap();
    *fail.lock().unwrap() = true;
    let error = fixture_runtime::handshake::establish(
        &mut worker,
        Box::new(std::io::Cursor::new(Vec::<u8>::new())),
        0,
    )
    .err()
    .unwrap();
    assert!(matches!(error, RuntimeError::StopUnconfirmed(_)));
    *fail.lock().unwrap() = false;
    worker.stop().unwrap();
}
#[derive(Clone, Default)]
struct Backend {
    events: Arc<Mutex<Vec<&'static str>>>,
    fail_stop: Arc<Mutex<bool>>,
    stop_root: Option<std::path::PathBuf>,
    retained: Arc<Mutex<Vec<LaunchSpec>>>,
}
impl VmBackend for Backend {
    fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
        self.events.lock().unwrap().push("create");
        assert_eq!(spec.state_bytes, STATE_BYTES);
        // Model preparation only; never allocate a real 32 GiB disk in unit tests.
        fs::OpenOptions::new()
            .write(true)
            .open(&spec.state_path)?
            .set_len(64)?;
        Ok(())
    }
    fn start(&mut self, spec: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
        self.events.lock().unwrap().push("start");
        self.retained.lock().unwrap().push(spec.clone());
        Ok(BackendHandle::new(42_u64))
    }
    fn stop(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        if let Some(root) = &self.stop_root {
            assert!(matches!(Journal::open(root), Err(RuntimeError::Busy)));
        }
        assert_eq!(handle.get::<u64>(), Some(&42));
        self.events.lock().unwrap().push("stop");
        if *self.fail_stop.lock().unwrap() {
            Err(RuntimeError::Unavailable)
        } else {
            Ok(())
        }
    }
    fn pause(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
        self.events.lock().unwrap().push("pause");
        Ok(())
    }
    fn resume(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
        self.events.lock().unwrap().push("resume");
        Ok(())
    }
}
fn hello(stage: Stage, nonce: Uuid, guest: u128) -> StageHello {
    StageHello {
        version: VERSION,
        stage,
        guest_nonce: Uuid::from_u128(guest),
        run_cleanup: false,
        accepted_boot_nonce: if stage == Stage::Serve {
            Some(nonce)
        } else {
            None
        },
    }
}
#[test]
fn creation_consumed_durably_before_delivery_never_repeated_on_serve_or_reopen() {
    let (temp, j) = create();
    let mut worker = make_worker(Box::new(Backend::default()), j);
    worker.start().unwrap();
    let nonce = worker.lease().unwrap().spec.boot_nonce;
    let early = worker
        .boot_request(&hello(Stage::Early, nonce, 3), 123)
        .unwrap();
    assert!(matches!(
        early.body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::New,
            ..
        })
    ));
    assert!(worker
        .boot_request(&hello(Stage::Early, nonce, 4), 123)
        .is_err());
    let ready = worker
        .boot_request(&hello(Stage::Serve, nonce, 4), 123)
        .unwrap();
    assert!(matches!(
        ready.body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::Existing,
            ..
        })
    ));
    worker.stop().unwrap();
    drop(worker);
    let mut worker = make_worker(
        Box::new(Backend::default()),
        Journal::open(&temp.path().join("install")).unwrap(),
    );
    worker.start().unwrap();
    let other = worker.lease().unwrap().spec.boot_nonce;
    assert_ne!(nonce, other);
    let early = worker
        .boot_request(&hello(Stage::Early, other, 5), 123)
        .unwrap();
    assert!(matches!(
        early.body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::Existing,
            ..
        })
    ));
}
#[test]
fn backend_failures_keep_ownership_and_wrong_hello_cannot_consume_creation() {
    let (_temp, j) = create();
    let backend = Backend::default();
    let failing = backend.fail_stop.clone();
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), j);
    worker.start().unwrap();
    let nonce = worker.lease().unwrap().spec.boot_nonce;
    assert!(worker.start().is_err());
    assert!(worker
        .boot_request(&hello(Stage::Serve, Uuid::from_u128(99), 3), 123)
        .is_err());
    assert!(matches!(
        worker
            .boot_request(&hello(Stage::Early, nonce, 3), 123)
            .unwrap()
            .body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::New,
            ..
        })
    ));
    worker.pause().unwrap();
    worker.resume().unwrap();
    *failing.lock().unwrap() = true;
    assert!(worker.stop().is_err());
    assert!(worker.lease().is_some());
    *failing.lock().unwrap() = false;
    worker.stop().unwrap();
    assert!(worker.lease().is_none());
    assert_eq!(
        *events.lock().unwrap(),
        vec!["create", "start", "pause", "resume", "stop", "stop"]
    );
}
#[test]
fn merged_engine_actions_have_only_typed_fixed_wire_projection() {
    let request = engine_request(
        Uuid::from_u128(1),
        7,
        EngineAction::ListAgents {
            context: "context with spaces".into(),
            namespace: "ns".into(),
        },
    );
    assert!(
        matches!(request.body,RequestBody::Engine(EngineActionWire::ListAgents{context,namespace}) if context=="context with spaces" && namespace=="ns")
    );
    assert!(matches!(
        engine_request(Uuid::from_u128(1), 8, EngineAction::Version {}).body,
        RequestBody::Engine(EngineActionWire::Version {})
    ));
}
#[test]
fn managed_runtime_stays_unavailable_and_shutdown_is_idempotent() {
    let state = RuntimeState::default();
    assert!(state.start().is_err());
    state.shutdown().unwrap();
    state.shutdown().unwrap();
    assert!(state.start().is_err());
}

#[test]
fn interrupted_initialization_or_record_write_stays_retained() {
    let (temp, journal) = create();
    let root = temp.path().join("install");
    drop(journal);
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("record.json")).unwrap()).unwrap();
    record["phase"] = serde_json::json!("preparing");
    fs::write(
        root.join("record.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    assert!(Journal::open(&root).is_err());
    assert!(root.join("state.disk").exists());

    let (temp, journal) = create();
    let root = temp.path().join("install");
    let selected = fs::read(root.join("record.json")).unwrap();
    fs::write(root.join("record.next"), b"interrupted record").unwrap();
    let backend = Backend::default();
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), journal);
    assert!(worker.start().is_err());
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(fs::read(root.join("record.json")).unwrap(), selected);
    drop(worker);
    assert!(Journal::open(&root).is_err());
    assert_eq!(
        fs::read(root.join("record.next")).unwrap(),
        b"interrupted record"
    );
}

#[test]
fn concurrent_service_starts_share_one_owned_lease_and_shutdown_barrier() {
    let (_temp, journal) = create();
    let backend = Backend::default();
    let events = backend.events.clone();
    let service = Arc::new(RuntimeState::with_worker(make_worker(
        Box::new(backend),
        journal,
    )));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let service = service.clone();
            std::thread::spawn(move || service.start().is_ok())
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|t| t.join().unwrap().then_some(()))
            .count(),
        1
    );
    service.shutdown().unwrap();
    service.shutdown().unwrap();
    assert!(service.start().is_err());
    assert_eq!(*events.lock().unwrap(), vec!["create", "start", "stop"]);
}

#[cfg(unix)]
#[test]
fn symlink_state_is_refused_without_following_or_modifying_target() {
    let (temp, journal) = create();
    let root = temp.path().join("install");
    let outside = temp.path().join("outside");
    fs::write(&outside, b"not owned by the installation").unwrap();
    fs::remove_file(root.join("state.disk")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("state.disk")).unwrap();
    let mut worker = make_worker(Box::new(Backend::default()), journal);
    assert!(worker.start().is_err());
    assert_eq!(fs::read(outside).unwrap(), b"not owned by the installation");
}

#[test]
fn dropping_running_worker_stops_before_releasing_ownership() {
    let (temp, journal) = create();
    let backend = Backend {
        stop_root: Some(temp.path().join("install")),
        ..Backend::default()
    };
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), journal);
    worker.start().unwrap();
    drop(worker);
    assert_eq!(*events.lock().unwrap(), vec!["create", "start", "stop"]);
    assert!(Journal::open(&temp.path().join("install")).is_ok());
}
#[test]
fn retained_backend_capability_keeps_dropped_journal_ownership() {
    let (temp, journal) = create();
    let backend = Backend::default();
    let retained = backend.retained.clone();
    let mut worker = make_worker(Box::new(backend), journal);
    worker.start().unwrap();
    drop(worker);
    assert_eq!(retained.lock().unwrap().len(), 1);
    assert!(matches!(
        Journal::open(&temp.path().join("install")),
        Err(RuntimeError::Busy)
    ));
}

#[test]
fn maximum_machine_identifier_survives_boot_record_updates_and_reopen() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("install");
    let verified = image_fixture::verified(
        temp.path(),
        fixture_runtime::image_types::ImageTarget::MacosArm64,
    );
    let journal = Journal::create(&root, BackendKind::Apple, vec![255; 4096]).unwrap();
    install_image(&journal, &verified);
    let mut worker = make_worker(Box::new(Backend::default()), journal);
    worker.start().unwrap();
    let nonce = worker.lease().unwrap().spec.boot_nonce;
    worker
        .boot_request(&hello(Stage::Early, nonce, 3), 123)
        .unwrap();
    worker.stop().unwrap();
    drop(worker);
    assert_eq!(
        Journal::open(&root).unwrap().record().machine_id,
        vec![255; 4096]
    );
}

#[test]
fn reopened_unfinished_state_never_regains_creation_authority() {
    let (temp, journal) = create();
    let root = temp.path().join("install");
    drop(journal);
    let backend = Backend::default();
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), Journal::open(&root).unwrap());
    assert!(matches!(worker.start(), Err(RuntimeError::RetainedState)));
    assert!(events.lock().unwrap().is_empty());

    let (temp, journal) = create();
    let root = temp.path().join("install");
    let mut worker = make_worker(Box::new(Backend::default()), journal);
    worker.start().unwrap();
    worker.stop().unwrap();
    drop(worker);
    // No descriptor escaped, but the original exclusive creation handle closed.
    let backend = Backend::default();
    let events = backend.events.clone();
    let mut worker = make_worker(Box::new(backend), Journal::open(&root).unwrap());
    assert!(matches!(worker.start(), Err(RuntimeError::RetainedState)));
    assert!(events.lock().unwrap().is_empty());
}

#[test]
#[ignore = "subprocess fixture for failed-drop ownership"]
fn failed_drop_child() {
    let Some(root) = std::env::var_os("BRIDGE_RUNTIME_DROP_TEST_DIR") else {
        return; // This entrypoint is driven by the parent test, not standalone.
    };
    let root = std::path::PathBuf::from(root);
    let verified =
        image_fixture::verified(&root, fixture_runtime::image_types::ImageTarget::MacosArm64);
    let journal = Journal::create(&root.join("install"), BackendKind::Apple, vec![1]).unwrap();
    install_image(&journal, &verified);
    let backend = Backend {
        stop_root: Some(root.join("install")),
        ..Backend::default()
    };
    *backend.fail_stop.lock().unwrap() = true;
    let mut worker = make_worker(Box::new(backend), journal);
    worker.start().unwrap();
    drop(worker);
    fs::write(root.join("ready"), b"ready").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !root.join("exit").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "parent did not release subprocess"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn failed_worker_drop_quarantines_ownership_until_process_exit() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let temp = tempdir().unwrap();
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "failed_drop_child"])
            .env("BRIDGE_RUNTIME_DROP_TEST_DIR", temp.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !temp.path().join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child exited before ownership check"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "child did not publish ownership check"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(matches!(
        Journal::open(&temp.path().join("install")),
        Err(RuntimeError::Busy)
    ));
    fs::write(temp.path().join("exit"), b"exit").unwrap();
    assert!(child.0.wait().unwrap().success());
    assert!(Journal::open(&temp.path().join("install")).is_ok());
}

mod protocol_checks {
    include!("../../../runtime/protocol/tests/wire.rs");
}
