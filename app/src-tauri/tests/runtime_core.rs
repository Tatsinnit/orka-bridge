use bridge_engine::engine::types::EngineAction;
use bridge_engine::runtime::{journal::*, types::*, wire::*, RuntimeState, RuntimeWorker};
use bridge_runtime_protocol::*;
use std::{
    fs,
    sync::{Arc, Mutex},
};
use tempfile::tempdir;

fn create() -> (tempfile::TempDir, Journal) {
    let temp = tempdir().unwrap();
    let journal = Journal::create(
        &temp.path().join("install"),
        BackendKind::Apple,
        vec![1, 2, 3],
    )
    .unwrap();
    (temp, journal)
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
    let mut worker = RuntimeWorker::new(Box::new(backend), journal);
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

#[derive(Clone, Default)]
struct Backend {
    events: Arc<Mutex<Vec<&'static str>>>,
    fail_stop: Arc<Mutex<bool>>,
    stop_root: Option<std::path::PathBuf>,
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
    fn start(&mut self, _: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
        self.events.lock().unwrap().push("start");
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
    let mut worker = RuntimeWorker::new(Box::new(Backend::default()), j);
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
    let mut worker = RuntimeWorker::new(
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
    let mut worker = RuntimeWorker::new(Box::new(backend), j);
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
    let mut worker = RuntimeWorker::new(Box::new(backend), journal);
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
    let service = Arc::new(RuntimeState::with_worker(RuntimeWorker::new(
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
    let mut worker = RuntimeWorker::new(Box::new(Backend::default()), journal);
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
    let mut worker = RuntimeWorker::new(Box::new(backend), journal);
    worker.start().unwrap();
    drop(worker);
    assert_eq!(*events.lock().unwrap(), vec!["create", "start", "stop"]);
    assert!(Journal::open(&temp.path().join("install")).is_ok());
}

#[test]
fn maximum_machine_identifier_survives_boot_record_updates_and_reopen() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("install");
    let journal = Journal::create(&root, BackendKind::Apple, vec![255; 4096]).unwrap();
    let mut worker = RuntimeWorker::new(Box::new(Backend::default()), journal);
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
    let mut worker = RuntimeWorker::new(Box::new(backend), Journal::open(&root).unwrap());
    assert!(matches!(worker.start(), Err(RuntimeError::RetainedState)));
    assert!(events.lock().unwrap().is_empty());

    let (temp, journal) = create();
    let root = temp.path().join("install");
    let mut worker = RuntimeWorker::new(Box::new(Backend::default()), journal);
    worker.start().unwrap();
    worker.stop().unwrap();
    drop(worker);
    // No descriptor escaped, but the original exclusive creation handle closed.
    let backend = Backend::default();
    let events = backend.events.clone();
    let mut worker = RuntimeWorker::new(Box::new(backend), Journal::open(&root).unwrap());
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
    let journal = Journal::create(&root.join("install"), BackendKind::Apple, vec![1]).unwrap();
    let backend = Backend {
        stop_root: Some(root.join("install")),
        ..Backend::default()
    };
    *backend.fail_stop.lock().unwrap() = true;
    let mut worker = RuntimeWorker::new(Box::new(backend), journal);
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
