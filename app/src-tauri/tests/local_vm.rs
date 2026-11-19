use bridge_engine::{
    engine::{types::*, EngineState, EngineTransport, TransportFuture},
    local_vm::{LocalFactory, LocalSession, LocalVm, LocalVmStatus, StartFuture},
    runtime::types::RuntimeError,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use tokio::sync::{watch, Notify};

#[derive(Default)]
struct Probe {
    starts: AtomicUsize,
    stops: AtomicUsize,
    launch: Notify,
    entered: Notify,
    io: (Mutex<bool>, Condvar),
    order: Mutex<Vec<&'static str>>,
    fail_stop: bool,
    fail_launch: bool,
}
struct Factory(Arc<Probe>);
struct Session(Arc<Probe>);
impl LocalFactory for Factory {
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        progress: Arc<dyn Fn(bridge_engine::local_vm::LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        let p = self.0.clone();
        Box::pin(async move {
            p.starts.fetch_add(1, Ordering::SeqCst);
            progress(bridge_engine::local_vm::LocalProgress::Status(
                LocalVmStatus::Installing,
            ));
            progress(bridge_engine::local_vm::LocalProgress::Image(
                bridge_engine::runtime::image_types::ImageProgress::Retained {
                    bytes: 800,
                    total: 1000,
                },
            ));
            p.launch.notified().await;
            if p.fail_launch {
                return Err(RuntimeError::Io(std::io::Error::other(
                    "launch detail\nDetails: C:\\VM\\failure",
                )));
            }
            Ok(Box::new(Session(p)) as Box<dyn LocalSession>)
        })
    }
}
impl LocalSession for Session {
    fn execute(&mut self, _: EngineRequest) -> TransportFuture<'_> {
        let p = self.0.clone();
        Box::pin(async move {
            // Mac-shaped nested blocking work: it survives a dropped outer caller.
            tokio::task::spawn_blocking(move || {
                p.order.lock().unwrap().push("read");
                p.entered.notify_one();
                let (lock, wake) = &p.io;
                let ready = wake
                    .wait_timeout_while(
                        lock.lock().unwrap(),
                        std::time::Duration::from_secs(3),
                        |ready| !*ready,
                    )
                    .unwrap();
                assert!(*ready.0, "test must explicitly finish underlying I/O");
                p.order.lock().unwrap().push("read-done");
                Ok(EngineOutput {
                    exit: 0,
                    stdout: b"kmx v0.9.0\n".to_vec(),
                    stderr: Vec::new(),
                })
            })
            .await
            .unwrap()
        })
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        self.0.order.lock().unwrap().push("stop");
        if self.0.fail_stop {
            Err(RuntimeError::Io(std::io::Error::other("stop detail")))
        } else {
            Ok(())
        }
    }
}
fn owner(p: Arc<Probe>) -> Arc<LocalVm> {
    LocalVm::new(Arc::new(Factory(p)))
}
async fn phase(vm: &LocalVm, expected: LocalVmStatus) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while vm.status() != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
fn finish(p: &Probe) {
    *p.io.0.lock().unwrap() = true;
    p.io.1.notify_all();
}
fn request() -> EngineRequest {
    EngineRequest {
        action: EngineAction::Version {},
    }
}

#[tokio::test]
async fn setup_snapshot_publishes_native_replacement_latch() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(root.path().to_owned())));
    let vm = LocalVm::unavailable();
    let initial =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    assert!(initial.get("workspaceReplaced").is_none());
    vm.set_workspace_replaced(true);
    vm.set_workspace_replaced(false);
    let replaced = serde_json::to_value(local_vm::setup_status(store, &vm).await.unwrap()).unwrap();
    assert_eq!(replaced["workspaceReplaced"], true);
}

#[tokio::test]
async fn cancellation_joins_late_session_before_restart_and_change_clears_choice() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    let initial =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    assert_eq!(initial["modelSupported"], local_vm::MODEL_IMAGE_AVAILABLE);
    local_vm::select_local(store.clone(), &vm).await.unwrap();
    phase(&vm, LocalVmStatus::Installing).await;
    let installing =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    assert_eq!(
        installing["modelSupported"],
        local_vm::MODEL_IMAGE_AVAILABLE
    );
    let cancelling = {
        let vm = vm.clone();
        let store = store.clone();
        tokio::spawn(async move { local_vm::cancel_local(store, &vm).await })
    };
    phase(&vm, LocalVmStatus::Closing).await;
    vm.start();
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    assert!(!cancelling.is_finished());
    p.launch.notify_one();
    let cancelled = serde_json::to_value(cancelling.await.unwrap().unwrap()).unwrap();
    assert_eq!(cancelled["localVm"], "cancelled");
    assert_eq!(cancelled["selected"], "localVm");
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
    local_vm::select_local(store.clone(), &vm).await.unwrap();
    phase(&vm, LocalVmStatus::Installing).await;
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    let changed = local_vm::change_local(store.clone(), &vm).await.unwrap();
    assert!(changed.selected.is_none());
    assert!(store.status().unwrap().selected.is_none());
    assert_eq!(p.stops.load(Ordering::SeqCst), 2);
    local_vm::select_local(store, &vm).await.unwrap();
    phase(&vm, LocalVmStatus::Installing).await;
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    vm.shutdown().await.unwrap();
    vm.start();
    assert_eq!(p.starts.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn cancelled_failed_stop_quarantines_late_session_and_keeps_selection() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    let p = Arc::new(Probe {
        fail_stop: true,
        ..Probe::default()
    });
    let vm = owner(p.clone());
    local_vm::select_local(store.clone(), &vm).await.unwrap();
    phase(&vm, LocalVmStatus::Installing).await;
    let changing = {
        let vm = vm.clone();
        let store = store.clone();
        tokio::spawn(async move { local_vm::change_local(store, &vm).await })
    };
    phase(&vm, LocalVmStatus::Closing).await;
    p.launch.notify_one();
    assert!(changing.await.unwrap().is_err());
    assert_eq!(vm.status(), LocalVmStatus::Failed);
    assert!(store.status().unwrap().selected.is_some());
    let failed =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    assert_eq!(
        failed["workspaceFailure"],
        serde_json::json!({"operation": "stop", "cause": "stopUnconfirmed", "codes": []})
    );
    vm.start();
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap()
            ["workspaceFailure"]["cause"],
        "stopUnconfirmed"
    );
    assert!(vm.execute(request()).await.is_err());
}
struct CooperativeFactory;
impl LocalFactory for CooperativeFactory {
    fn launch(
        &self,
        mut cancelled: watch::Receiver<bool>,
        _: Arc<dyn Fn(bridge_engine::local_vm::LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        Box::pin(async move {
            while !*cancelled.borrow_and_update() {
                cancelled.changed().await.unwrap();
            }
            Err(RuntimeError::Closed)
        })
    }
}
#[tokio::test]
async fn cooperative_cancellation_is_clean_but_terminal_close_is_irreversible() {
    let vm = LocalVm::new(Arc::new(CooperativeFactory));
    vm.start();
    vm.cancel().await.unwrap();
    assert_eq!(vm.status(), LocalVmStatus::Cancelled);
    vm.start();
    assert_eq!(vm.status(), LocalVmStatus::Preparing);
    let cancelling = {
        let vm = vm.clone();
        tokio::spawn(async move { vm.cancel().await })
    };
    vm.request_close();
    cancelling.await.unwrap().unwrap();
    vm.shutdown().await.unwrap();
    assert_eq!(vm.status(), LocalVmStatus::Closed);
    vm.start();
    assert_eq!(vm.status(), LocalVmStatus::Closed);
}
#[tokio::test]
async fn retained_progress_is_optional_and_cleared_after_dropped_change_waiter() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    let idle =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    assert!(idle.get("workspaceProgress").is_none());
    local_vm::select_local(store.clone(), &vm).await.unwrap();
    phase(&vm, LocalVmStatus::Installing).await;
    let installing =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    let progress = &installing["workspaceProgress"];
    assert_eq!(progress["phase"], "materializing");
    assert_eq!(progress["bytes"], 800);
    assert_eq!(progress["totalBytes"], 1000);
    assert!(progress["remainingMs"].is_null());
    assert!(progress["elapsedMs"].as_u64().is_some());
    let changing = {
        let vm = vm.clone();
        let store = store.clone();
        tokio::spawn(async move { local_vm::change_local(store, &vm).await })
    };
    phase(&vm, LocalVmStatus::Closing).await;
    changing.abort();
    let _ = changing.await;
    p.launch.notify_one();
    // Joining cancellation proves native stop, but the confirmed preference job also survives.
    vm.cancel().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while store
            .clone()
            .status_async()
            .await
            .unwrap()
            .selected
            .is_some()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let changed = serde_json::to_value(local_vm::setup_status(store, &vm).await.unwrap()).unwrap();
    assert_eq!(changed["localVm"], "cancelled");
    assert!(changed.get("workspaceProgress").is_none());
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn selection_schedules_only_one_session_and_status_is_observational() {
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    assert_eq!(vm.status(), LocalVmStatus::Idle);
    assert_eq!(p.starts.load(Ordering::SeqCst), 0);
    vm.start();
    vm.start();
    phase(&vm, LocalVmStatus::Installing).await;
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    let engine = EngineState::with_transport(vm.clone());
    assert!(
        engine.call(request()).await.is_err(),
        "selection is not readiness"
    );
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    finish(&p);
    assert!(matches!(
        engine.call(request()).await.unwrap(),
        EngineReply::Version { .. }
    ));
    vm.shutdown().await.unwrap();
    assert_eq!(&*p.order.lock().unwrap(), &["read", "read-done", "stop"]);
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
    vm.start();
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    assert!(engine.call(request()).await.is_err());
}
#[tokio::test]
async fn close_during_start_waits_and_stops_late_session_without_ready() {
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    vm.start();
    phase(&vm, LocalVmStatus::Installing).await;
    let closing = {
        let vm = vm.clone();
        tokio::spawn(async move { vm.shutdown().await })
    };
    phase(&vm, LocalVmStatus::Closing).await;
    assert!(!closing.is_finished());
    p.launch.notify_one();
    closing.await.unwrap().unwrap();
    assert_eq!(vm.status(), LocalVmStatus::Closed);
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn cancelled_request_keeps_session_until_inner_io_finishes_and_refuses_queueing() {
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    vm.start();
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    let engine = Arc::new(EngineState::with_transport(vm.clone()));
    let running = {
        let e = engine.clone();
        tokio::spawn(async move { e.call(request()).await })
    };
    p.entered.notified().await;
    assert_eq!(
        engine.call(request()).await.unwrap_err().kind,
        ErrorKind::Execution
    );
    running.abort();
    let _ = running.await;
    let closing = {
        let vm = vm.clone();
        tokio::spawn(async move { vm.shutdown().await })
    };
    phase(&vm, LocalVmStatus::Closing).await;
    assert!(!closing.is_finished());
    assert_eq!(p.stops.load(Ordering::SeqCst), 0);
    finish(&p);
    closing.await.unwrap().unwrap();
    assert_eq!(&*p.order.lock().unwrap(), &["read", "read-done", "stop"]);
}
#[tokio::test]
async fn cancelled_close_waiter_does_not_abandon_startup_or_custody() {
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    vm.start();
    phase(&vm, LocalVmStatus::Installing).await;
    let closing = {
        let vm = vm.clone();
        tokio::spawn(async move { vm.shutdown().await })
    };
    phase(&vm, LocalVmStatus::Closing).await;
    closing.abort();
    let _ = closing.await;
    p.launch.notify_one();
    vm.shutdown().await.unwrap();
    assert_eq!(vm.status(), LocalVmStatus::Closed);
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_stop_cannot_claim_closed_clean_or_restart() {
    let p = Arc::new(Probe {
        fail_stop: true,
        ..Probe::default()
    });
    let vm = owner(p.clone());
    vm.start();
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    assert!(vm.shutdown().await.is_err());
    assert_eq!(vm.status(), LocalVmStatus::Failed);
    vm.start();
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    assert!(vm.shutdown().await.is_err());
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn setup_retains_the_actual_stop_error_after_repeated_close() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    let p = Arc::new(Probe {
        fail_stop: true,
        ..Probe::default()
    });
    let vm = owner(p.clone());
    local_vm::select_local(store.clone(), &vm).await.unwrap();
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    assert!(vm.shutdown().await.is_err());
    let failed =
        serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap()).unwrap();
    let reason = failed["localVmError"]
        .as_str()
        .expect("stop reason retained");
    assert!(reason.contains("Stopping"), "{reason}");
    assert!(reason.contains("stop detail"), "{reason}");
    assert_eq!(failed["localVm"], "failed");
    assert_eq!(failed["model"], "locked");
    assert_eq!(failed["selected"], "localVm");
    assert!(vm.shutdown().await.is_err());
    let again = serde_json::to_value(local_vm::setup_status(store, &vm).await.unwrap()).unwrap();
    assert_eq!(again, failed);
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn setup_retains_the_actual_start_error_including_during_close() {
    use bridge_engine::{local_vm, setup::SetupStore};
    for during_close in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
        let p = Arc::new(Probe {
            fail_launch: true,
            ..Probe::default()
        });
        let vm = owner(p.clone());
        local_vm::select_local(store.clone(), &vm).await.unwrap();
        phase(&vm, LocalVmStatus::Installing).await;
        if during_close {
            vm.request_close();
        }
        p.launch.notify_one();
        if !during_close {
            phase(&vm, LocalVmStatus::Failed).await;
            let failed =
                serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap())
                    .unwrap();
            let reason = failed["localVmError"]
                .as_str()
                .expect("start reason retained");
            assert!(reason.contains("Starting"), "{reason}");
            assert!(
                reason.contains("launch detail\nDetails: C:\\VM\\failure"),
                "{reason}"
            );
        }
        assert!(vm.shutdown().await.is_err());
        let failed =
            serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap())
                .unwrap();
        let reason = failed["localVmError"]
            .as_str()
            .expect("start reason survives close");
        assert!(reason.contains("Starting"), "{reason}");
        assert!(
            reason.contains("launch detail\nDetails: C:\\VM\\failure"),
            "{reason}"
        );
        assert_eq!(failed["localVm"], "failed");
        assert_eq!(failed["model"], "locked");
        assert!(vm.shutdown().await.is_err());
        let again =
            serde_json::to_value(local_vm::setup_status(store, &vm).await.unwrap()).unwrap();
        assert_eq!(again, failed);
        assert_eq!(p.stops.load(Ordering::SeqCst), 0);
    }
}
#[tokio::test]
async fn native_unconfigured_owner_never_starts_from_selection() {
    let vm = LocalVm::unavailable();
    vm.start();
    assert_eq!(vm.status(), LocalVmStatus::NotIntegrated);
    vm.shutdown().await.unwrap();
    assert!(vm.execute(request()).await.is_err());
}
#[tokio::test]
async fn startup_error_cannot_become_clean_close_with_an_empty_session_slot() {
    for during_close in [false, true] {
        let p = Arc::new(Probe {
            fail_launch: true,
            ..Probe::default()
        });
        let vm = owner(p.clone());
        vm.start();
        phase(&vm, LocalVmStatus::Installing).await;
        if during_close {
            vm.request_close();
        }
        p.launch.notify_one();
        if !during_close {
            phase(&vm, LocalVmStatus::Failed).await;
        }
        assert!(
            vm.shutdown().await.is_err(),
            "joining a returned task is not proof of successful native cleanup"
        );
        assert_eq!(vm.status(), LocalVmStatus::Failed);
        assert_eq!(p.stops.load(Ordering::SeqCst), 0);
        assert!(vm.shutdown().await.is_err());
    }
}
#[test]
fn native_close_signal_bars_ready_without_an_async_runtime() {
    let vm = LocalVm::unavailable();
    vm.request_close();
    assert_eq!(vm.status(), LocalVmStatus::Closing);
}
#[tokio::test]
async fn setup_selection_persists_then_starts_but_status_never_starts() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    let observed = local_vm::setup_status(store.clone(), &vm).await.unwrap();
    assert!(observed.selected.is_none());
    assert_eq!(p.starts.load(Ordering::SeqCst), 0);
    let selected = local_vm::select_local(store.clone(), &vm).await.unwrap();
    assert!(selected.selected.is_some());
    assert!(temp.path().join("setup-v1/local-vm").is_file());
    phase(&vm, LocalVmStatus::Installing).await;
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    local_vm::select_local(store.clone(), &vm).await.unwrap();
    assert_eq!(p.starts.load(Ordering::SeqCst), 1);
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    let value = serde_json::to_value(local_vm::setup_status(store, &vm).await.unwrap()).unwrap();
    assert_eq!(value["localVm"], "ready");
    assert!(value.get("localVmError").unwrap().is_null());
    assert_eq!(
        value["model"],
        if cfg!(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(windows, target_arch = "x86_64")
        )) {
            "available"
        } else {
            "locked"
        }
    );
    assert!(
        !temp.path().join("setup-v1/model-v1").exists(),
        "status must not capture or associate a sign-in"
    );
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn native_resume_starts_only_a_saved_choice_without_resaving_it() {
    use bridge_engine::{local_vm, setup::SetupStore};
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    local_vm::resume_selected(store.clone(), &vm).await.unwrap();
    assert_eq!(p.starts.load(Ordering::SeqCst), 0);
    store.select_local().unwrap();
    local_vm::resume_selected(store, &vm).await.unwrap();
    phase(&vm, LocalVmStatus::Installing).await;
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn preflight_failure_survives_direct_selection_and_reopen_without_image_progress() {
    use bridge_engine::{
        local_vm,
        setup::{preflight::Failure, SetupStore},
    };
    struct Refused;
    impl LocalFactory for Refused {
        fn launch(
            &self,
            _: watch::Receiver<bool>,
            _: Arc<dyn Fn(local_vm::LocalProgress) + Send + Sync>,
        ) -> StartFuture<'_> {
            Box::pin(async { Err(RuntimeError::Preflight(Failure::InsufficientMemory)) })
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(SetupStore::new(Some(temp.path().to_owned())));
    for reopen in [false, true] {
        let vm = LocalVm::new(Arc::new(Refused));
        if reopen {
            local_vm::resume_selected(store.clone(), &vm).await.unwrap();
        } else {
            local_vm::select_local(store.clone(), &vm).await.unwrap();
        }
        phase(&vm, LocalVmStatus::Failed).await;
        let value = serde_json::to_value(local_vm::setup_status(store.clone(), &vm).await.unwrap())
            .unwrap();
        assert_eq!(value["preflightFailure"], "insufficientMemory");
        assert_eq!(value["selected"], "localVm");
        assert!(value["workspaceProgress"].is_null());
        assert!(!value["localVmError"]
            .as_str()
            .unwrap()
            .contains("Preflight("));
        let changed =
            serde_json::to_value(local_vm::change_local(store.clone(), &vm).await.unwrap())
                .unwrap();
        assert!(changed.get("preflightFailure").is_none());
        assert!(store.status().unwrap().selected.is_none());
        if !reopen {
            store.select_local().unwrap();
        }
        vm.shutdown().await.unwrap();
    }
}
/// Emits the scripted progress, then fails with the scripted typed error.
struct Scripted(
    Vec<bridge_engine::local_vm::LocalProgress>,
    Mutex<Option<RuntimeError>>,
);
impl LocalFactory for Scripted {
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        progress: Arc<dyn Fn(bridge_engine::local_vm::LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        for event in &self.0 {
            progress(*event);
        }
        let error = self.1.lock().unwrap().take();
        Box::pin(async move { Err(error.expect("one scripted failure")) })
    }
}
async fn scripted_failure(
    events: Vec<bridge_engine::local_vm::LocalProgress>,
    error: RuntimeError,
) -> serde_json::Value {
    let vm = LocalVm::new(Arc::new(Scripted(events, Mutex::new(Some(error)))));
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(bridge_engine::setup::SetupStore::new(Some(
        temp.path().to_owned(),
    )));
    vm.start();
    phase(&vm, LocalVmStatus::Failed).await;
    serde_json::to_value(
        bridge_engine::local_vm::setup_status(store, &vm)
            .await
            .unwrap(),
    )
    .unwrap()
}
#[tokio::test]
async fn failed_phase_reports_typed_metadata_from_the_latest_operation_not_text() {
    use bridge_engine::{
        local_vm::LocalProgress as P,
        runtime::image_types::{ImageError, ImageProgress as Image},
        setup::preflight::Failure,
    };
    use std::io::{Error, ErrorKind};
    let verifying = vec![P::Image(Image::Verifying)];
    let preparing = vec![
        P::Image(Image::Verifying),
        P::Status(LocalVmStatus::Installing),
        P::Image(Image::Materializing { bytes: 1, total: 9 }),
    ];
    let starting = vec![P::Status(LocalVmStatus::Starting)];
    let noisy = "NoSpace StorageFull code: 5 (os error 28) token=ghp_secret C:\\Users\\x";
    let denied_code = if cfg!(windows) { 5 } else { 13 };
    let cases = [
        (
            verifying.clone(),
            RuntimeError::Image(ImageError::Integrity),
            ("verify", "integrity", vec![]),
        ),
        (
            preparing.clone(),
            RuntimeError::Io(Error::from_raw_os_error(denied_code)),
            ("prepare", "permission", vec![denied_code]),
        ),
        // A cleared progress clock must not turn a download failure into startup.
        (
            preparing.clone(),
            RuntimeError::Image(ImageError::Network("transport")),
            ("download", "network", vec![]),
        ),
        (
            starting.clone(),
            RuntimeError::Io(Error::from(ErrorKind::PermissionDenied)),
            ("start", "permission", vec![]),
        ),
        (
            starting.clone(),
            RuntimeError::Io(Error::other(noisy)),
            ("start", "unknown", vec![]),
        ),
        (
            starting.clone(),
            RuntimeError::Infrastructure(noisy.into()),
            ("services", "services", vec![]),
        ),
        (
            starting,
            RuntimeError::StopUnconfirmed(noisy.into()),
            ("stop", "stopUnconfirmed", vec![]),
        ),
        (
            verifying,
            RuntimeError::Preflight(Failure::VirtualizationDisabled),
            ("requirements", "unknown", vec![]),
        ),
    ];
    for (events, error, (operation, cause, codes)) in cases {
        let label = format!("{error:?}");
        let value = scripted_failure(events, error).await;
        assert_eq!(
            value["workspaceFailure"],
            serde_json::json!({"operation": operation, "cause": cause, "codes": codes}),
            "{label}"
        );
        // The private diagnostic stays separate from every finite field.
        let metadata = value["workspaceFailure"].to_string();
        assert!(
            !metadata.contains("ghp_") && !metadata.contains("Users"),
            "{metadata}"
        );
    }
}
#[tokio::test]
async fn metadata_is_only_serialized_while_failed_and_clears_on_retry() {
    let vm = LocalVm::new(Arc::new(Scripted(
        vec![],
        Mutex::new(Some(RuntimeError::Busy)),
    )));
    let temp = tempfile::tempdir().unwrap();
    let store = Arc::new(bridge_engine::setup::SetupStore::new(Some(
        temp.path().to_owned(),
    )));
    let status = || async {
        serde_json::to_value(
            bridge_engine::local_vm::setup_status(store.clone(), &vm)
                .await
                .unwrap(),
        )
        .unwrap()
    };
    assert!(status().await.get("workspaceFailure").is_none());
    vm.start();
    phase(&vm, LocalVmStatus::Failed).await;
    assert_eq!(status().await["workspaceFailure"]["cause"], "inUse");
    // The factory has no second scripted failure and panics inside its task,
    // but admission has already cleared the old metadata.
    vm.start();
    assert!(status().await.get("workspaceFailure").is_none());
}
#[test]
fn fixed_example_is_the_exact_public_pair_not_an_edit_or_run_operation() {
    use sha2::{Digest, Sha256};
    let pair = bridge_engine::local_vm::example();
    assert_eq!(
        format!("{:x}", Sha256::digest(pair.agent.as_bytes())),
        "5c92e1867edf076e62723120a12264a0c5106e3047bfc8901973803f440c39af"
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(pair.tests.as_bytes())),
        "b462346165c70e4f6d33f05c17bc06ffea120f28603edca617fc0a5654fb8144"
    );
    assert!(pair.agent.len() < 65536 && pair.tests.len() < 65536);
}
#[tokio::test]
async fn engine_shutdown_bars_transport_calls() {
    let p = Arc::new(Probe::default());
    let vm = owner(p.clone());
    vm.start();
    p.launch.notify_one();
    phase(&vm, LocalVmStatus::Ready).await;
    finish(&p);
    let engine = EngineState::with_transport(vm.clone());
    engine.shutdown();
    assert!(engine.call(request()).await.is_err());
    assert!(p.order.lock().unwrap().is_empty());
    vm.shutdown().await.unwrap();
}
