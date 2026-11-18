//! Facade tests reuse real pinned-image, filesystem and transaction fixtures.
use super::*;
use crate::local_vm::{
    LocalFactory, LocalProgress, LocalVm, LocalVmStatus, StartFuture, UpgradeTarget,
};
use std::sync::{Arc, Mutex};

use crate::engine::{types::EngineRequest, TransportFuture};
use crate::local_vm::LocalSession;
use crate::runtime::{
    types::{BackendHandle, LaunchSpec, VmBackend},
    RuntimeWorker,
};

struct Backend;
impl VmBackend for Backend {
    fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
        // Stand in only for the platform formatter, never for journal admission.
        fs::OpenOptions::new()
            .write(true)
            .open(&spec.state_path)?
            .set_len(64)?;
        Ok(())
    }
    fn start(&mut self, _: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
        Ok(BackendHandle::new(()))
    }
    fn stop(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn pause(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
        unreachable!()
    }
    fn resume(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
        unreachable!()
    }
}
// Own this inside block_on so unwinding releases blocking workers before Runtime::drop.
struct ReleaseOnDrop(std::sync::mpsc::Sender<()>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}
impl std::ops::Deref for ReleaseOnDrop {
    type Target = std::sync::mpsc::Sender<()>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
type StopBarrier = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
struct Session(
    RuntimeWorker,
    Arc<std::sync::atomic::AtomicUsize>,
    Arc<Mutex<Option<StopBarrier>>>,
);
impl LocalSession for Session {
    fn execute(&mut self, _: EngineRequest) -> TransportFuture<'_> {
        panic!("upgrade must never send Connect/Test/Chat")
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let gate = self.2.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        self.0.stop()
    }
}
struct DestinationFactory {
    root: PathBuf,
    data: PathBuf,
    destination: Mutex<Option<Journal>>,
    adoption: Option<(
        std::sync::mpsc::Sender<()>,
        Mutex<std::sync::mpsc::Receiver<()>>,
    )>,
    launched: Mutex<Vec<Uuid>>,
    stops: Arc<std::sync::atomic::AtomicUsize>,
    releases: std::sync::atomic::AtomicUsize,
    adoption_error: Mutex<Option<RuntimeError>>,
    stop_gate: Arc<Mutex<Option<StopBarrier>>>,
    release_failure: std::sync::atomic::AtomicBool,
    launch_gate: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            tokio::sync::oneshot::Receiver<()>,
        )>,
    >,
}
impl LocalFactory for DestinationFactory {
    fn adopt_destination(&self, journal: Journal) -> Result<(), RuntimeError> {
        if let Some((entered, release)) = &self.adoption {
            entered.send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
        }
        if let Some(error) = self.adoption_error.lock().unwrap().take() {
            return Err(error);
        }
        *self.destination.lock().unwrap() = Some(journal);
        Ok(())
    }
    fn release_stopped(&self) -> Result<(), RuntimeError> {
        self.releases
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self
            .release_failure
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(RuntimeError::Unavailable);
        }
        self.destination.lock().unwrap().take();
        Ok(())
    }
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        _: Arc<dyn Fn(LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        // Exercise the same live-journal admission used by NativeFactory. No
        // platform VM or guest messages (Connect/Test/Chat) are involved.
        Box::pin(async {
            let gate = self.launch_gate.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            let journal = match self.destination.lock().unwrap().take() {
                Some(journal) => {
                    admit_prepared(&self.data, &self.root, &journal)?;
                    journal.require_fresh_creation()?;
                    journal
                }
                None => {
                    admit(&self.data, &self.root)?;
                    Journal::open(&self.root)?
                }
            };
            let installation = journal.record().installation_id;
            let fresh = journal.record().phase != StatePhase::Consumed;
            let installed = Arc::new(crate::runtime::image_install::open_installed_image(
                &journal.image_target("new-state")?,
            )?);
            let mut worker = RuntimeWorker::new(Box::new(Backend), journal, installed)?;
            worker.start()?;
            let boot = worker.boot_request(
                &bridge_runtime_protocol::StageHello {
                    version: bridge_runtime_protocol::VERSION,
                    stage: bridge_runtime_protocol::Stage::Early,
                    guest_nonce: Uuid::new_v4(),
                    accepted_boot_nonce: None,
                    run_cleanup: false,
                },
                1,
            )?;
            let bridge_runtime_protocol::RequestBody::Boot(descriptor) = boot.body else {
                panic!("boot descriptor")
            };
            assert_eq!(
                descriptor.state_creation,
                if fresh {
                    bridge_runtime_protocol::StateCreation::New
                } else {
                    bridge_runtime_protocol::StateCreation::Existing
                }
            );
            self.launched.lock().unwrap().push(installation);
            Ok(
                Box::new(Session(worker, self.stops.clone(), self.stop_gate.clone()))
                    as Box<dyn LocalSession>,
            )
        })
    }
}
fn facade(
    scene: &Scene,
    adoption: Option<(
        std::sync::mpsc::Sender<()>,
        Mutex<std::sync::mpsc::Receiver<()>>,
    )>,
) -> (Arc<LocalVm>, Arc<DestinationFactory>) {
    let factory = Arc::new(DestinationFactory {
        root: scene.local.join("local-vm-v1"),
        data: scene.data.clone(),
        destination: Mutex::new(None),
        adoption,
        launched: Mutex::new(Vec::new()),
        stops: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        releases: std::sync::atomic::AtomicUsize::new(0),
        adoption_error: Mutex::new(None),
        stop_gate: Arc::new(Mutex::new(None)),
        release_failure: std::sync::atomic::AtomicBool::new(false),
        launch_gate: Mutex::new(None),
    });
    (LocalVm::new(factory.clone()), factory)
}
fn completed(reply: UpgradeActionReply) -> UpgradeStatusDto {
    match reply {
        UpgradeActionReply::Completed { status } => status,
        UpgradeActionReply::Cancelled { .. } => panic!("expected completion, not cancellation"),
    }
}
fn target(scene: &Scene) -> UpgradeTarget {
    UpgradeTarget {
        source: scene.source.clone(),
        machine_id: vec![2],
    }
}
fn seed_cache(scene: &mut Scene) {
    // Seed exactly the verified cache which production acquisition reads; no
    // bypass of replace/retry, manifest policy or materialization.
    scene.target.take();
    fs::rename(
        scene._temp.path().join("new-cache/cache"),
        scene.local.join("image-cache-v1"),
    )
    .unwrap();
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn forward_task(
    scene: &Scene,
    vm: &Arc<LocalVm>,
    transaction: Option<Uuid>,
) -> tokio::task::JoinHandle<Result<UpgradeActionReply, RuntimeError>> {
    let (vm, data, local, target) = (
        vm.clone(),
        scene.data.clone(),
        scene.local.clone(),
        target(scene),
    );
    tokio::spawn(async move {
        match transaction {
            Some(id) => vm.retry_workspace(data, local, target, id).await,
            None => vm.replace_workspace(data, local, target).await,
        }
    })
}
fn retry_transaction(scene: &mut Scene, retry: bool) -> Option<Uuid> {
    if !retry {
        return None;
    }
    scene.replace(Some("intent")).unwrap_err();
    Some(
        WorkspaceUpgrade::open(&scene.data, &scene.local)
            .unwrap()
            .read_intent()
            .unwrap()
            .unwrap()
            .transaction,
    )
}

#[test]
fn facade_busy_restore_transfer_remove_and_quarantine_have_no_cancel_authority() {
    for action in ["restore", "transfer", "remove"] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            let dto = completed(forward_task(&scene, &vm, None).await.unwrap().unwrap());
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let (entered, entered_rx) = std::sync::mpsc::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let release = ReleaseOnDrop(release);
            *factory.stop_gate.lock().unwrap() = Some((entered, release_rx));
            // Fail custody release after stop, before any credential-store access.
            factory
                .release_failure
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let (owner, data, local) = (vm.clone(), scene.data.clone(), scene.local.clone());
            let work = tokio::spawn(async move {
                match action {
                    "restore" => {
                        owner
                            .restore_workspace(data, local, dto.transaction.unwrap())
                            .await
                    }
                    "transfer" => {
                        owner
                            .transfer_workspace_credential(data, local, dto.transaction.unwrap())
                            .await
                    }
                    "remove" => {
                        owner
                            .remove_workspace_credential(data, local, dto.transaction.unwrap())
                            .await
                    }
                    _ => unreachable!(),
                }
            });
            tokio::task::spawn_blocking(move || {
                entered_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
            })
            .await
            .unwrap();
            let status = vm
                .upgrade_status(scene.data.clone(), scene.local.clone())
                .await;
            let mut cancel = Box::pin(vm.cancel_workspace_upgrade());
            let observed = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(std::future::Future::poll(cancel.as_mut(), cx))
            })
            .await;
            release.send(()).unwrap();
            let work_result = work.await.unwrap();
            if observed.is_pending() {
                let _ = cancel.await;
            }
            assert!(work_result.is_err());
            assert!(status.busy);
            assert!(!status.can_cancel);
            assert!(matches!(
                observed,
                std::task::Poll::Ready(Ok(UpgradeCancelOutcome::NotCancellable))
            ));
            let quarantined = vm
                .upgrade_status(scene.data.clone(), scene.local.clone())
                .await;
            assert_eq!(quarantined.error, Some("stopUnconfirmed"));
            assert!(!quarantined.can_cancel);
            assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_adoption_failures_are_not_user_cancellation() {
    for retry in [false, true] {
        for closed in [false, true] {
            let mut scene = Scene::new(ImageTarget::WindowsAmd64);
            let transaction = retry_transaction(&mut scene, retry);
            seed_cache(&mut scene);
            let (entered, entered_rx) = std::sync::mpsc::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let (vm, factory) = facade(&scene, Some((entered, Mutex::new(release_rx))));
            *factory.adoption_error.lock().unwrap() = Some(if closed {
                RuntimeError::Closed
            } else {
                RuntimeError::Io(std::io::Error::other("adoption I/O failure"))
            });
            runtime().block_on(async {
                let release = ReleaseOnDrop(release);
                let worker = forward_task(&scene, &vm, transaction);
                tokio::task::spawn_blocking(move || {
                    entered_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap()
                })
                .await
                .unwrap();
                let can_cancel = vm
                    .upgrade_status(scene.data.clone(), scene.local.clone())
                    .await
                    .can_cancel;
                let mut cancel = Box::pin(vm.cancel_workspace_upgrade());
                let observed = std::future::poll_fn(|cx| {
                    std::task::Poll::Ready(std::future::Future::poll(cancel.as_mut(), cx))
                })
                .await;
                release.send(()).unwrap();
                let result = worker.await.unwrap();
                if observed.is_pending() {
                    let _ = cancel.await;
                }
                assert!(can_cancel);
                assert!(matches!(
                    observed,
                    std::task::Poll::Ready(Ok(UpgradeCancelOutcome::Requested))
                ));
                if closed {
                    assert!(matches!(result, Err(RuntimeError::Closed)));
                } else {
                    assert!(matches!(result, Err(RuntimeError::Io(_))));
                }
                assert!(factory.launched.lock().unwrap().is_empty());
                assert!(
                    !vm.upgrade_status(scene.data.clone(), scene.local.clone())
                        .await
                        .can_cancel
                );
                vm.shutdown().await.unwrap();
            });
        }
    }
}

#[test]
fn facade_terminal_close_dominates_user_cancel_in_either_order() {
    use std::future::Future;
    for retry in [false, true] {
        for user_first in [false, true] {
            let mut scene = Scene::new(ImageTarget::WindowsAmd64);
            let transaction = retry_transaction(&mut scene, retry);
            seed_cache(&mut scene);
            let (vm, factory) = facade(&scene, None);
            runtime().block_on(async {
                let forward = async {
                    match transaction {
                        Some(id) => {
                            vm.retry_workspace(
                                scene.data.clone(),
                                scene.local.clone(),
                                target(&scene),
                                id,
                            )
                            .await
                        }
                        None => {
                            vm.replace_workspace(
                                scene.data.clone(),
                                scene.local.clone(),
                                target(&scene),
                            )
                            .await
                        }
                    }
                };
                tokio::pin!(forward);
                std::future::poll_fn(|cx| {
                    assert!(forward.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                if user_first {
                    assert_eq!(
                        vm.cancel_workspace_upgrade().await.unwrap(),
                        UpgradeCancelOutcome::Requested
                    );
                }
                vm.request_close();
                assert_eq!(
                    vm.cancel_workspace_upgrade().await.unwrap(),
                    UpgradeCancelOutcome::NotCancellable
                );
                assert!(matches!(forward.await, Err(RuntimeError::Closed)));
                assert!(
                    !vm.upgrade_status(scene.data.clone(), scene.local.clone())
                        .await
                        .can_cancel
                );
                assert!(factory.launched.lock().unwrap().is_empty());
                vm.shutdown().await.unwrap();
            });
        }
    }
}

#[test]
fn facade_cancel_marked_startup_joins_late_session_without_releasing_creator() {
    use std::future::Future;
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = retry_transaction(&mut scene, retry);
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        *factory.launch_gate.lock().unwrap() = Some((entered, release_rx));
        runtime().block_on(async {
            let reply_status = completed(
                forward_task(&scene, &vm, transaction)
                    .await
                    .unwrap()
                    .unwrap(),
            );
            assert!(
                reply_status.can_cancel,
                "the admitted marked startup is cancellable"
            );
            entered_rx.await.unwrap();
            assert_eq!(vm.status(), LocalVmStatus::Preparing);
            assert!(
                vm.upgrade_status(scene.data.clone(), scene.local.clone())
                    .await
                    .can_cancel
            );
            let cancel = vm.cancel_workspace_upgrade();
            tokio::pin!(cancel);
            std::future::poll_fn(|cx| {
                assert!(cancel.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            assert_eq!(vm.status(), LocalVmStatus::Closing);
            assert!(
                !vm.upgrade_status(scene.data.clone(), scene.local.clone())
                    .await
                    .can_cancel
            );
            release.send(()).unwrap();
            assert_eq!(cancel.await.unwrap(), UpgradeCancelOutcome::Completed);
            assert_eq!(vm.status(), LocalVmStatus::Cancelled);
            assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(
                factory.releases.load(std::sync::atomic::Ordering::SeqCst),
                1
            );
            // An ordinary restart must not inherit upgrade cancellation authority.
            let (entered, entered_rx) = tokio::sync::oneshot::channel();
            let (release, release_rx) = tokio::sync::oneshot::channel();
            *factory.launch_gate.lock().unwrap() = Some((entered, release_rx));
            vm.start();
            entered_rx.await.unwrap();
            assert!(
                !vm.upgrade_status(scene.data.clone(), scene.local.clone())
                    .await
                    .can_cancel
            );
            assert_eq!(
                vm.cancel_workspace_upgrade().await.unwrap(),
                UpgradeCancelOutcome::NotCancellable
            );
            assert_eq!(vm.status(), LocalVmStatus::Preparing);
            release.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_retirement_observer_still_has_forward_cancel_authority() {
    use std::future::Future;
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = retry_transaction(&mut scene, retry);
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        let weak = Arc::downgrade(&vm);
        vm.on_workspace_replaced(Arc::new(move || {
            let vm = weak.upgrade().unwrap();
            let mut cancel = Box::pin(vm.cancel_workspace_upgrade());
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            assert!(matches!(
                cancel.as_mut().poll(&mut cx),
                std::task::Poll::Ready(Ok(UpgradeCancelOutcome::Requested))
            ));
        }));
        runtime().block_on(async {
            assert!(matches!(
                forward_task(&scene, &vm, transaction).await.unwrap(),
                Ok(UpgradeActionReply::Cancelled { .. })
            ));
            assert!(factory.launched.lock().unwrap().is_empty());
            factory
                .destination
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .require_fresh_creation()
                .unwrap();
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_upgrade_reply_serialization_is_per_action_not_status_acknowledgement() {
    for (outcome, value) in [
        (UpgradeCancelOutcome::Requested, "requested"),
        (UpgradeCancelOutcome::Completed, "completed"),
        (UpgradeCancelOutcome::NotCancellable, "notCancellable"),
    ] {
        assert_eq!(serde_json::to_value(outcome).unwrap(), value);
    }
    let dto = UpgradeStatusDto::none();
    assert!(!dto.can_cancel);
    for (reply, outcome) in [
        (
            UpgradeActionReply::Completed {
                status: dto.clone(),
            },
            "completed",
        ),
        (UpgradeActionReply::Cancelled { status: dto }, "cancelled"),
    ] {
        let value = serde_json::to_value(reply).unwrap();
        assert_eq!(value["outcome"], outcome);
        assert_eq!(value["status"]["canCancel"], false);
        assert!(value["status"].get("outcome").is_none());
    }
}

#[test]
fn facade_late_upgrade_cancel_cannot_stop_ready_destination() {
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = if retry {
            scene.replace(Some("intent")).unwrap_err();
            Some(
                WorkspaceUpgrade::open(&scene.data, &scene.local)
                    .unwrap()
                    .read_intent()
                    .unwrap()
                    .unwrap()
                    .transaction,
            )
        } else {
            None
        };
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            match transaction {
                Some(id) => {
                    vm.retry_workspace(scene.data.clone(), scene.local.clone(), target(&scene), id)
                        .await
                }
                None => {
                    vm.replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
                        .await
                }
            }
            .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                !vm.upgrade_status(scene.data.clone(), scene.local.clone())
                    .await
                    .can_cancel
            );
            assert_eq!(
                vm.cancel_workspace_upgrade().await.unwrap(),
                UpgradeCancelOutcome::NotCancellable
            );
            assert_eq!(
                vm.status(),
                LocalVmStatus::Ready,
                "late Cancel stopped Ready; retry={retry}"
            );
            assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 0);
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_replace_hands_live_creation_authority_to_launch_and_reports_fresh() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    seed_cache(&mut scene);
    let (vm, factory) = facade(&scene, None);
    runtime().block_on(async {
        let dto = vm
            .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
            .await
            .unwrap();
        let dto = completed(dto);
        assert_eq!(dto.state, "fresh");
        assert!(!dto.busy);
        assert!(vm.workspace_replaced());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while factory.launched.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(factory.launched.lock().unwrap().len(), 1);
        assert_ne!(factory.launched.lock().unwrap()[0], scene.source_id);
        assert_eq!(vm.status(), LocalVmStatus::Ready);
        let status = vm
            .upgrade_status(scene.data.clone(), scene.local.clone())
            .await;
        assert_eq!(status.state, "consumed");
        assert!(
            !status.busy,
            "our live VM journal is not mutation contention"
        );
        assert_eq!(
            read_record(&scene.local.join("local-vm-v1")).unwrap().phase,
            StatePhase::Consumed
        );
        assert!(!vm.model_connected());
        vm.shutdown().await.unwrap();
    });
}

#[test]
fn facade_dropped_forward_waiter_never_releases_lifecycle_before_work_finishes() {
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = if retry {
            scene.replace(Some("intent")).unwrap_err();
            Some(
                WorkspaceUpgrade::open(&scene.data, &scene.local)
                    .unwrap()
                    .read_intent()
                    .unwrap()
                    .unwrap()
                    .transaction,
            )
        } else {
            None
        };
        seed_cache(&mut scene);
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let (vm, factory) = facade(&scene, Some((entered, Mutex::new(release_rx))));
        let retired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = retired.clone();
        vm.on_workspace_replaced(Arc::new(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        runtime().block_on(async {
            let worker = {
                let (vm, data, local, target) = (
                    vm.clone(),
                    scene.data.clone(),
                    scene.local.clone(),
                    target(&scene),
                );
                tokio::spawn(async move {
                    match transaction {
                        Some(id) => vm.retry_workspace(data, local, target, id).await,
                        None => vm.replace_workspace(data, local, target).await,
                    }
                })
            };
            tokio::task::spawn_blocking(move || {
                entered_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
            })
            .await
            .unwrap();
            worker.abort();
            assert!(worker.await.unwrap_err().is_cancelled());
            vm.start();
            let held_status = vm.status();
            let launched_early = !factory.launched.lock().unwrap().is_empty();
            release.send(()).unwrap();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while retired.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert_eq!(
                retired.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "native commit must retire ephemeral state even after the IPC waiter drops"
            );
            assert!(matches!(
                held_status,
                LocalVmStatus::Cancelled | LocalVmStatus::Preparing
            ));
            assert!(
                !launched_early,
                "gate released before worker finished; retry={retry}"
            );
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_shutdown_joins_forward_handoff_before_returning() {
    use std::future::Future;
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = if retry {
            scene.replace(Some("intent")).unwrap_err();
            Some(
                WorkspaceUpgrade::open(&scene.data, &scene.local)
                    .unwrap()
                    .read_intent()
                    .unwrap()
                    .unwrap()
                    .transaction,
            )
        } else {
            None
        };
        seed_cache(&mut scene);
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let (vm, factory) = facade(&scene, Some((entered, Mutex::new(release_rx))));
        let retired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = retired.clone();
        vm.on_workspace_replaced(Arc::new(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        runtime().block_on(async {
            let worker = {
                let (vm, data, local, target) = (
                    vm.clone(),
                    scene.data.clone(),
                    scene.local.clone(),
                    target(&scene),
                );
                tokio::spawn(async move {
                    match transaction {
                        Some(id) => vm.retry_workspace(data, local, target, id).await,
                        None => vm.replace_workspace(data, local, target).await,
                    }
                })
            };
            tokio::task::spawn_blocking(move || {
                entered_rx
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap()
            })
            .await
            .unwrap();
            let shutdown = vm.shutdown();
            tokio::pin!(shutdown);
            let finished_early = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(shutdown.as_mut().poll(cx).is_ready())
            })
            .await;
            // Always unblock the owned fileworker before assertions or joins.
            release.send(()).unwrap();
            assert!(matches!(worker.await.unwrap(), Err(RuntimeError::Closed)));
            shutdown.await.unwrap();
            vm.start();
            tokio::task::yield_now().await;
            assert!(!finished_early, "shutdown detached handoff; retry={retry}");
            assert_eq!(retired.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(
                factory.launched.lock().unwrap().is_empty(),
                "terminal close must bar late launch; retry={retry}"
            );
            assert_eq!(vm.status(), LocalVmStatus::Closed);
        });
    }
}

#[test]
fn facade_rejects_foreign_nil_and_replayed_transactions_without_mutating_identity() {
    for id in [Uuid::nil(), Uuid::from_u128(123)] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            let dto = vm
                .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
                .await
                .unwrap();
            let dto = completed(dto);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let releases = factory.releases.load(std::sync::atomic::Ordering::SeqCst);
            let before = fs::read(scene.local.join("workspace-upgrades-v1/intent.json")).unwrap();
            assert!(vm
                .retry_workspace(scene.data.clone(), scene.local.clone(), target(&scene), id)
                .await
                .is_err());
            assert!(vm
                .restore_workspace(scene.data.clone(), scene.local.clone(), id)
                .await
                .is_err());
            assert!(vm
                .transfer_workspace_credential(scene.data.clone(), scene.local.clone(), id)
                .await
                .is_err());
            assert!(vm
                .remove_workspace_credential(scene.data.clone(), scene.local.clone(), id)
                .await
                .is_err());
            assert_eq!(
                fs::read(scene.local.join("workspace-upgrades-v1/intent.json")).unwrap(),
                before
            );
            assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert_eq!(
                factory.releases.load(std::sync::atomic::Ordering::SeqCst),
                releases
            );
            assert_eq!(vm.status(), LocalVmStatus::Ready);
            assert!(vm
                .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
                .await
                .is_err());
            assert!(vm
                .retry_workspace(
                    scene.data.clone(),
                    scene.local.clone(),
                    target(&scene),
                    dto.transaction.unwrap()
                )
                .await
                .is_err());
            assert_eq!(
                factory.stops.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "direct initial-replace bypass must not stop current B"
            );
            assert_eq!(
                factory.releases.load(std::sync::atomic::Ordering::SeqCst),
                releases
            );
            assert_eq!(vm.status(), LocalVmStatus::Ready);
            assert!(dto.transaction.is_some());
            assert!(factory.launched.lock().unwrap().len() <= 1);
        });
    }
}

#[test]
fn facade_explicit_retry_from_blocked_startup_boots_only_new_b() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(Some("intent")).unwrap_err();
    let transaction = WorkspaceUpgrade::open(&scene.data, &scene.local)
        .unwrap()
        .read_intent()
        .unwrap()
        .unwrap()
        .transaction;
    seed_cache(&mut scene);
    let (configured, factory) = facade(&scene, None);
    let vm = configured.clone().for_recovery();
    runtime().block_on(async {
        vm.start();
        tokio::task::yield_now().await;
        assert!(factory.launched.lock().unwrap().is_empty());
        let dto = vm
            .retry_workspace(
                scene.data.clone(),
                scene.local.clone(),
                target(&scene),
                transaction,
            )
            .await
            .unwrap();
        let dto = completed(dto);
        assert_eq!(dto.state, "fresh");
        assert!(
            Arc::ptr_eq(&configured, &vm),
            "recovery must retain the configured owner and factory"
        );
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while vm.status() != LocalVmStatus::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_ne!(factory.launched.lock().unwrap()[0], scene.source_id);
        assert_eq!(
            vm.upgrade_status(scene.data.clone(), scene.local.clone())
                .await
                .state,
            "consumed"
        );
        assert!(!vm.model_connected());
        vm.transfer_workspace_credential(scene.data.clone(), scene.local.clone(), transaction)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while vm.status() != LocalVmStatus::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let launched = factory.launched.lock().unwrap().clone();
        assert_eq!(launched.len(), 2);
        assert_eq!(launched[0], launched[1]);
        assert!(!vm.model_connected());
        let stops = factory.stops.load(std::sync::atomic::Ordering::SeqCst);
        let releases = factory.releases.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            vm.transfer_workspace_credential(scene.data.clone(), scene.local.clone(), transaction)
                .await
                .is_err(),
            "completed transfer replay must not stop or restart B"
        );
        assert_eq!(
            factory.stops.load(std::sync::atomic::Ordering::SeqCst),
            stops
        );
        assert_eq!(
            factory.releases.load(std::sync::atomic::Ordering::SeqCst),
            releases
        );
        assert_eq!(vm.status(), LocalVmStatus::Ready);
        vm.shutdown().await.unwrap();
    });
}

#[test]
fn facade_cold_consumed_restart_seeds_transaction_before_ordinary_startup() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    seed_cache(&mut scene);
    let (vm, _) = facade(&scene, None);
    runtime().block_on(async {
        let before = vm
            .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
            .await
            .unwrap();
        let before = completed(before);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while vm.status() != LocalVmStatus::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        vm.shutdown().await.unwrap();
        let (reopened, _) = facade(&scene, None);
        reopened.seed_upgrade_status(&scene.data, &scene.local);
        reopened.start();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while reopened.status() != LocalVmStatus::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let status = reopened
            .upgrade_status(scene.data.clone(), scene.local.clone())
            .await;
        assert_eq!(status.transaction, before.transaction);
        assert_eq!(status.state, "consumed");
        assert_eq!(status.credential, "eligible");
        assert!(status.can_restore);
        assert!(!status.busy);
        reopened.shutdown().await.unwrap();
    });
}

#[test]
fn facade_foreign_transaction_on_cold_recovery_cannot_release_any_native_custody() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(Some("intent")).unwrap_err();
    let (vm, factory) = facade(&scene, None);
    vm.block_for_recovery();
    let foreign = Uuid::new_v4();
    runtime().block_on(async {
        assert!(vm
            .restore_workspace(scene.data.clone(), scene.local.clone(), foreign)
            .await
            .is_err());
        assert!(vm
            .transfer_workspace_credential(scene.data.clone(), scene.local.clone(), foreign)
            .await
            .is_err());
        assert!(vm
            .remove_workspace_credential(scene.data.clone(), scene.local.clone(), foreign)
            .await
            .is_err());
        assert!(vm
            .retry_workspace(
                scene.data.clone(),
                scene.local.clone(),
                target(&scene),
                foreign
            )
            .await
            .is_err());
    });
    assert_eq!(
        factory.releases.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "cache absence never authorizes lifecycle side effects"
    );
    assert_eq!(
        read_record(&scene.local.join("local-vm-v1"))
            .unwrap()
            .installation_id,
        scene.source_id
    );
}

#[test]
fn facade_restore_stops_current_b_and_restores_a_without_starting_it() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    seed_cache(&mut scene);
    let (vm, factory) = facade(&scene, None);
    runtime().block_on(async {
        let dto = vm
            .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
            .await
            .unwrap();
        let dto = completed(dto);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while vm.status() != LocalVmStatus::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let restored = vm
            .restore_workspace(
                scene.data.clone(),
                scene.local.clone(),
                dto.transaction.unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restored.state, "restored");
        assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            factory.releases.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(
            read_record(&scene.local.join("local-vm-v1"))
                .unwrap()
                .installation_id,
            scene.source_id
        );
        vm.start();
        tokio::task::yield_now().await;
        assert_eq!(
            factory.launched.lock().unwrap().len(),
            1,
            "A is restored for its matching older app, never booted here"
        );
        assert!(!vm.model_connected());
    });
}

#[test]
fn facade_simultaneous_forward_requests_admit_exactly_one_without_stopping_its_b() {
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = if retry {
            scene.replace(Some("intent")).unwrap_err();
            Some(
                WorkspaceUpgrade::open(&scene.data, &scene.local)
                    .unwrap()
                    .read_intent()
                    .unwrap()
                    .unwrap()
                    .transaction,
            )
        } else {
            None
        };
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            let forward = || async {
                match transaction {
                    Some(id) => vm.retry_workspace(scene.data.clone(), scene.local.clone(), target(&scene), id).await,
                    None => vm.replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene)).await,
                }
            };
            // Both futures are polled before either owned worker can run.
            let (first, second) = tokio::join!(forward(), forward());
            assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
            let rejected = first.as_ref().err().or(second.as_ref().err());
            assert!(matches!(rejected, Some(RuntimeError::Busy)), "the second call must lose admission, not stop B and fail in storage; retry={retry}: {rejected:?}");
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready { tokio::task::yield_now().await; }
            }).await.unwrap();
            assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 0);
            assert_eq!(factory.releases.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(factory.launched.lock().unwrap().len(), 1);
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_completed_removal_ack_refuses_replay_before_stopping_b() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    seed_cache(&mut scene);
    let (vm, factory) = facade(&scene, None);
    runtime().block_on(async {
        let dto = vm
            .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
            .await
            .unwrap();
        let dto = completed(dto);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while vm.status() != LocalVmStatus::Ready {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
        let slot = owner
            .resolve_removal_slot(dto.transaction.unwrap())
            .unwrap();
        // Durable acknowledgement from an earlier exact-slot removal, without
        // consulting a real credential store or refreshing the cached DTO.
        owner.write_slot_outcome(slot, b"removed").unwrap();
        drop(owner);
        assert!(vm
            .remove_workspace_credential(
                scene.data.clone(),
                scene.local.clone(),
                dto.transaction.unwrap()
            )
            .await
            .is_err());
        assert_eq!(factory.stops.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            factory.releases.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(vm.status(), LocalVmStatus::Ready);
        vm.shutdown().await.unwrap();
    });
}

#[test]
fn facade_transfer_rejects_removal_outcomes_before_stopping_b() {
    for outcome in [b"removing".as_slice(), b"removed".as_slice()] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            let dto = vm
                .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
                .await
                .unwrap();
            let dto = completed(dto);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
            let transaction = dto.transaction.unwrap();
            let slot = owner.resolve_removal_slot(transaction).unwrap();
            owner.write_slot_outcome(slot, outcome).unwrap();
            drop(owner);
            let stops = factory.stops.load(std::sync::atomic::Ordering::SeqCst);
            let releases = factory.releases.load(std::sync::atomic::Ordering::SeqCst);
            assert!(vm
                .transfer_workspace_credential(scene.data.clone(), scene.local.clone(), transaction)
                .await
                .is_err());
            assert_eq!(
                factory.stops.load(std::sync::atomic::Ordering::SeqCst),
                stops,
                "outcome={outcome:?}"
            );
            assert_eq!(
                factory.releases.load(std::sync::atomic::Ordering::SeqCst),
                releases
            );
            assert_eq!(vm.status(), LocalVmStatus::Ready);
            vm.shutdown().await.unwrap();
        });
    }
}

#[test]
fn facade_cancel_local_before_forward_worker_runs_keeps_a() {
    use std::future::Future;
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = if retry {
            scene.replace(Some("intent")).unwrap_err();
            Some(
                WorkspaceUpgrade::open(&scene.data, &scene.local)
                    .unwrap()
                    .read_intent()
                    .unwrap()
                    .unwrap()
                    .transaction,
            )
        } else {
            None
        };
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            let forward = async {
                match transaction {
                    Some(id) => {
                        vm.retry_workspace(
                            scene.data.clone(),
                            scene.local.clone(),
                            target(&scene),
                            id,
                        )
                        .await
                    }
                    None => {
                        vm.replace_workspace(
                            scene.data.clone(),
                            scene.local.clone(),
                            target(&scene),
                        )
                        .await
                    }
                }
            };
            tokio::pin!(forward);
            std::future::poll_fn(|cx| {
                assert!(forward.as_mut().poll(cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            let store = Arc::new(SetupStore::with_upgrade_root(
                Some(scene.data.clone()),
                Some(scene.local.clone()),
            ));
            let cancel = crate::local_vm::cancel_local(store, &vm).await;
            let result = forward.await;
            vm.shutdown().await.unwrap();
            assert!(cancel.is_ok(), "retry={retry}");
            assert!(
                matches!(result, Ok(UpgradeActionReply::Cancelled { .. })),
                "acknowledged cancel must be a typed cancellation; retry={retry}"
            );
            assert_eq!(
                read_record(&scene.local.join("local-vm-v1"))
                    .unwrap()
                    .installation_id,
                scene.source_id
            );
            assert!(factory.launched.lock().unwrap().is_empty());
        });
    }
}

#[test]
fn facade_cancel_local_during_forward_handoff_retains_b_without_launching() {
    for retry in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        let transaction = if retry {
            scene.replace(Some("intent")).unwrap_err();
            Some(
                WorkspaceUpgrade::open(&scene.data, &scene.local)
                    .unwrap()
                    .read_intent()
                    .unwrap()
                    .unwrap()
                    .transaction,
            )
        } else {
            None
        };
        seed_cache(&mut scene);
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let (vm, factory) = facade(&scene, Some((entered, Mutex::new(release_rx))));
        runtime().block_on(async {
            let forward = {
                let (vm, data, local, target) = (
                    vm.clone(),
                    scene.data.clone(),
                    scene.local.clone(),
                    target(&scene),
                );
                tokio::spawn(async move {
                    match transaction {
                        Some(id) => vm.retry_workspace(data, local, target, id).await,
                        None => vm.replace_workspace(data, local, target).await,
                    }
                })
            };
            let entered = tokio::task::spawn_blocking(move || {
                entered_rx.recv_timeout(std::time::Duration::from_secs(5))
            })
            .await
            .unwrap();
            let store = Arc::new(SetupStore::with_upgrade_root(
                Some(scene.data.clone()),
                Some(scene.local.clone()),
            ));
            let cancel = crate::local_vm::cancel_local(store, &vm).await;
            // Release before any assertion, even if cancellation failed.
            release.send(()).unwrap();
            let result = forward.await.unwrap();
            vm.shutdown().await.unwrap();
            assert!(entered.is_ok());
            assert!(cancel.is_ok(), "retry={retry}");
            assert!(
                matches!(result, Ok(UpgradeActionReply::Cancelled { .. })),
                "handoff must report cancellation; retry={retry}"
            );
            assert!(
                factory.launched.lock().unwrap().is_empty(),
                "launch after acknowledged cancel; retry={retry}"
            );
            let destination = factory.destination.lock().unwrap();
            let journal = destination.as_ref().expect("original B journal retained");
            journal.require_fresh_creation().unwrap();
            assert_eq!(
                journal.record().installation_id,
                read_record(&scene.local.join("local-vm-v1"))
                    .unwrap()
                    .installation_id
            );
        });
    }
}

#[test]
fn facade_transfer_rejects_conflicting_current_metadata_before_stopping_b() {
    for name in ["model-v1", "model-account-v1"] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        seed_cache(&mut scene);
        let (vm, factory) = facade(&scene, None);
        runtime().block_on(async {
            let dto = vm
                .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
                .await
                .unwrap();
            let dto = completed(dto);
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while vm.status() != LocalVmStatus::Ready {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
            let intent = owner.read_intent().unwrap().unwrap();
            let bytes = if name == "model-v1" {
                let model = SetupStore::transferred_model(
                    intent.model.as_deref(),
                    intent.destination.as_ref().unwrap().installation,
                )
                .unwrap()
                .unwrap();
                let mut model: serde_json::Value = serde_json::from_slice(&model).unwrap();
                assert!(model["slot"].is_string());
                model["slot"] = serde_json::json!(Uuid::new_v4());
                serde_json::to_vec(&model).unwrap()
            } else {
                // Unrecognized bytes must fail closed; this is not a schema fixture.
                b"unrecognized current account".to_vec()
            };
            drop(owner);
            let path = scene.data.join("setup-v1").join(name);
            fs::write(&path, &bytes).unwrap();
            let stops = factory.stops.load(std::sync::atomic::Ordering::SeqCst);
            let releases = factory.releases.load(std::sync::atomic::Ordering::SeqCst);
            let result = vm
                .transfer_workspace_credential(
                    scene.data.clone(),
                    scene.local.clone(),
                    dto.transaction.unwrap(),
                )
                .await;
            let status = vm.status();
            let stopped = factory.stops.load(std::sync::atomic::Ordering::SeqCst);
            let released = factory.releases.load(std::sync::atomic::Ordering::SeqCst);
            vm.shutdown().await.unwrap();
            assert!(result.is_err(), "name={name}");
            assert_eq!(status, LocalVmStatus::Ready, "name={name}");
            assert_eq!(stopped, stops, "name={name}");
            assert_eq!(released, releases, "name={name}");
            assert_eq!(fs::read(path).unwrap(), bytes);
        });
    }
}

#[test]
fn facade_terminal_close_before_worker_registers_cancellation_keeps_a() {
    use std::future::Future;
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    seed_cache(&mut scene);
    let (vm, factory) = facade(&scene, None);
    runtime().block_on(async {
        let forward = vm.replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene));
        tokio::pin!(forward);
        // Spawn the owner without allowing it to register its watch yet.
        std::future::poll_fn(|cx| {
            assert!(forward.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        vm.request_close();
        assert!(
            forward.await.is_err(),
            "terminal close must not be lost before watch registration"
        );
        vm.shutdown().await.unwrap();
        assert_eq!(
            read_record(&scene.local.join("local-vm-v1"))
                .unwrap()
                .installation_id,
            scene.source_id
        );
        assert!(factory.launched.lock().unwrap().is_empty());
    });
}

#[test]
fn facade_terminal_close_rejects_late_native_forward_without_filesystem_custody() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    seed_cache(&mut scene);
    let (vm, factory) = facade(&scene, None);
    runtime().block_on(async {
        vm.shutdown().await.unwrap();
        let result = vm
            .replace_workspace(scene.data.clone(), scene.local.clone(), target(&scene))
            .await;
        assert!(
            matches!(result, Err(RuntimeError::Closed)),
            "terminal close must bar later filesystem admission"
        );
        assert_eq!(
            factory.releases.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            read_record(&scene.local.join("local-vm-v1"))
                .unwrap()
                .installation_id,
            scene.source_id
        );
        assert_eq!(vm.status(), LocalVmStatus::Closed);
    });
}

#[test]
fn facade_unavailable_recovery_fallback_restores_without_factory() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(Some("intent")).unwrap_err();
    let transaction = WorkspaceUpgrade::open(&scene.data, &scene.local)
        .unwrap()
        .read_intent()
        .unwrap()
        .unwrap()
        .transaction;
    assert!(recover_startup(&scene.data, &scene.local).is_err());
    let vm = LocalVm::unavailable().for_recovery();
    runtime().block_on(async {
        let dto = vm
            .restore_workspace(scene.data.clone(), scene.local.clone(), transaction)
            .await
            .unwrap();
        assert_eq!(dto.state, "restored");
        vm.start();
        tokio::task::yield_now().await;
        assert_eq!(vm.status(), LocalVmStatus::Failed);
        assert_eq!(
            read_record(&scene.local.join("local-vm-v1"))
                .unwrap()
                .installation_id,
            scene.source_id
        );
    });
}

#[test]
fn facade_restore_after_restart_never_launches_the_old_installation() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(Some("intent")).unwrap_err();
    let transaction = WorkspaceUpgrade::open(&scene.data, &scene.local)
        .unwrap()
        .read_intent()
        .unwrap()
        .unwrap()
        .transaction;
    let (vm, factory) = facade(&scene, None);
    vm.block_for_recovery();
    runtime().block_on(async {
        vm.start();
        tokio::task::yield_now().await;
        assert!(factory.launched.lock().unwrap().is_empty());
        let dto = vm
            .restore_workspace(scene.data.clone(), scene.local.clone(), transaction)
            .await
            .unwrap();
        assert_eq!(dto.state, "restored");
        vm.start();
        tokio::task::yield_now().await;
        assert!(factory.launched.lock().unwrap().is_empty());
        assert_eq!(
            read_record(&scene.local.join("local-vm-v1"))
                .unwrap()
                .installation_id,
            scene.source_id
        );
    });
}
