use bridge_engine::{
    engine::{types::*, TransportFuture},
    local_vm::{LocalFactory, LocalSession, LocalVm, LocalVmStatus, StartFuture},
    runtime::{client::VmClient, types::RuntimeError},
};
use bridge_runtime_protocol::Uuid;
use std::{
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
};
use tokio::sync::{watch, Notify};
struct Channel;
impl Read for Channel {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}
impl Write for Channel {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Default)]
struct Probe {
    entered: Notify,
    gate: (Mutex<bool>, Condvar),
    calls: AtomicUsize,
    stops: AtomicUsize,
}
struct Factory(Arc<Probe>, Uuid);
struct Session(Arc<Probe>, Uuid, VmClient);
impl LocalFactory for Factory {
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        _: Arc<dyn Fn(bridge_engine::local_vm::LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        let session = Session(
            self.0.clone(),
            self.1,
            VmClient::established(Box::new(Channel), Uuid::new_v4()).unwrap(),
        );
        Box::pin(async { Ok(Box::new(session) as Box<dyn LocalSession>) })
    }
}
impl LocalSession for Session {
    fn execute(&mut self, _: EngineRequest) -> TransportFuture<'_> {
        Box::pin(async { Err(EngineError::new(ErrorKind::Unavailable)) })
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn with_client(
        &mut self,
        work: &mut dyn FnMut(Uuid, &mut VmClient) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        work(self.1, &mut self.2)
    }
}
async fn ready(vm: &Arc<LocalVm>) {
    vm.start();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while vm.status() != LocalVmStatus::Ready {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn client_callback_is_once_native_and_closed_admission_never_calls_it() {
    let p = Arc::new(Probe::default());
    let id = Uuid::new_v4();
    let vm = LocalVm::new(Arc::new(Factory(p.clone(), id)));
    assert!(vm.with_client(|_, _| panic!("not ready")).await.is_err());
    ready(&vm).await;
    let result = vm
        .with_client(move |installation, client| {
            assert_eq!(installation, id);
            assert_ne!(
                client.boot_nonce(),
                id,
                "boot nonce is not installation identity"
            );
            vec!["native typed result"]
        })
        .await
        .unwrap();
    assert_eq!(result, ["native typed result"]);
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    vm.shutdown().await.unwrap();
    assert!(vm.with_client(|_, _| panic!("closed")).await.is_err());
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn paired_image_model_support_is_visible_without_probing_the_guest() {
    let p = Arc::new(Probe::default());
    let vm = LocalVm::new(Arc::new(Factory(p.clone(), Uuid::new_v4())));
    ready(&vm).await;
    let root = tempfile::tempdir().unwrap();
    let setup = Arc::new(bridge_engine::setup::SetupStore::new(Some(
        root.path().to_owned(),
    )));
    setup.select_local().unwrap();
    let snapshot = bridge_engine::local_vm::setup_status(setup, &vm)
        .await
        .unwrap();
    let snapshot = serde_json::to_value(snapshot).unwrap();
    let supported = cfg!(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(windows, target_arch = "x86_64")
    ));
    assert_eq!(snapshot["modelSupported"], supported);
    assert_eq!(
        snapshot["model"],
        if supported { "available" } else { "locked" }
    );
    assert_eq!(p.calls.load(Ordering::SeqCst), 0);
    assert_eq!(vm.status(), LocalVmStatus::Ready);
    assert!(!root.path().join("setup-v1/model-v1").exists());
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn completion_after_close_is_not_admitted_as_success() {
    let p = Arc::new(Probe::default());
    let vm = LocalVm::new(Arc::new(Factory(p.clone(), Uuid::new_v4())));
    ready(&vm).await;
    let close = vm.clone();
    let result = vm
        .with_client(move |_, _| {
            close.request_close();
            true
        })
        .await;
    assert!(matches!(result, Err(RuntimeError::Closed)));
    vm.shutdown().await.unwrap();
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn cancelled_capture_or_connect_waiter_keeps_custody_and_excludes_duplicate_until_stop() {
    let p = Arc::new(Probe::default());
    let vm = LocalVm::new(Arc::new(Factory(p.clone(), Uuid::new_v4())));
    ready(&vm).await;
    let task = {
        let vm = vm.clone();
        let p = p.clone();
        tokio::spawn(async move {
            vm.with_client(move |_, _| {
                p.entered.notify_one();
                // The native model wrapper block_on(capture) also runs inside this guard.
                let gate = p
                    .gate
                    .1
                    .wait_timeout_while(
                        p.gate.0.lock().unwrap(),
                        std::time::Duration::from_secs(3),
                        |v| !*v,
                    )
                    .unwrap();
                assert!(*gate.0, "test must release native work");
            })
            .await
        })
    };
    p.entered.notified().await;
    assert!(vm.with_client(|_, _| panic!("duplicate")).await.is_err());
    task.abort();
    let _ = task.await;
    let stop = {
        let vm = vm.clone();
        tokio::spawn(async move { vm.shutdown().await })
    };
    tokio::task::yield_now().await;
    assert_eq!(p.stops.load(Ordering::SeqCst), 0);
    *p.gate.0.lock().unwrap() = true;
    p.gate.1.notify_all();
    stop.await.unwrap().unwrap();
    assert_eq!(p.stops.load(Ordering::SeqCst), 1);
    assert_eq!(p.calls.load(Ordering::SeqCst), 1);
}
