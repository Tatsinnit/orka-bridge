use super::*;
use crate::engine::{
    types::{EngineAction, EngineRequest},
    EngineTransport,
};
#[test]
fn stopping_with_an_inflight_engine_client_revokes_its_only_guest_capability() {
    let (t, journal, image) = installation();
    let resources = resource(&serving_peer("engine"));
    let mut session =
        AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
    let transport = session.transport.take().unwrap();
    // Materialization above owns its runtime; enter the engine runtime afterward.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let task = tokio::spawn(async move {
                transport
                    .execute(EngineRequest {
                        action: EngineAction::Version {},
                    })
                    .await
            });
            let seen = t.path().join("install/request-seen");
            let end = Instant::now() + Duration::from_secs(2);
            while !seen.exists() {
                assert!(Instant::now() < end, "engine request did not reach peer");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            // Taking the serving transport leaves only destructive cleanup.
            assert!(matches!(session.stop(), Err(RuntimeError::ShutdownForced)));
            assert!(tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap()
                .is_err());
            assert!(session.transport().is_none());
        });
}
#[test]
fn idle_stop_after_consumed_origin_does_not_grant_another_guest_interval() {
    let (t, journal, image) = installation();
    let resources = resource(&serving_peer("good"));
    let mut session =
        AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
    let slot = session
        .worker
        .lease()
        .unwrap()
        .handle
        .get::<AppleBackendHandle>()
        .unwrap()
        .guest
        .clone();
    let (_closed, close) = tokio::sync::watch::channel(true);
    let origin = Arc::new(std::sync::Mutex::new(Some(
        Instant::now() - Duration::from_secs(20),
    )));
    slot.bind_close(close, Some(origin));
    let started = Instant::now();
    assert!(matches!(session.stop(), Err(RuntimeError::ShutdownForced)));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!t.path().join("install/shutdown-request").exists());
    assert!(session.worker.lease().is_none());
}
#[test]
fn close_interrupts_inflight_io_before_acquiring_session_and_preserves_forced_outcome() {
    let (t, journal, image) = installation();
    let resources = resource(&serving_peer("engine"));
    let session =
        AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
    let slot = session
        .worker
        .lease()
        .unwrap()
        .handle
        .get::<AppleBackendHandle>()
        .unwrap()
        .guest
        .clone();
    let (closed, close) = tokio::sync::watch::channel(false);
    slot.bind_close(close, None);
    crate::runtime::apple_channel::tests::short_close(&slot);
    let session = Arc::new(tokio::sync::Mutex::new(session));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let executing = session.clone();
            let task = tokio::spawn(async move {
                let session = executing.lock().await;
                session
                    .transport()
                    .unwrap()
                    .execute(EngineRequest {
                        action: EngineAction::Version {},
                    })
                    .await
            });
            let seen = t.path().join("install/request-seen");
            let end = Instant::now() + Duration::from_secs(2);
            while !seen.exists() {
                assert!(Instant::now() < end);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            // Same order as the facade: publish close BEFORE awaiting its session mutex.
            let began = Instant::now();
            closed.send_replace(true);
            let stopping = session.clone();
            let stopped = tokio::time::timeout(
                Duration::from_secs(2),
                tokio::task::spawn_blocking(move || stopping.blocking_lock().stop()),
            )
            .await
            .expect("active request prevented native stop")
            .unwrap();
            assert!(began.elapsed() < Duration::from_secs(2));
            assert!(
                matches!(stopped, Err(RuntimeError::ShutdownForced)),
                "{stopped:?}"
            );
            assert!(task.await.unwrap().is_err());
            let mut session = session.lock().await;
            assert!(session.worker.lease().is_none());
            assert!(matches!(session.stop(), Err(RuntimeError::ShutdownForced)));
        });
    assert!(t.path().join("install/hard-stop").exists());
}
#[test]
fn forced_outcome_survives_revoke_failure_and_transportless_retry() {
    let (_t, journal, image) = installation();
    let resources = resource(&serving_peer("good"));
    let mut session =
        AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
    let slot = session
        .worker
        .lease()
        .unwrap()
        .handle
        .get::<AppleBackendHandle>()
        .unwrap()
        .guest
        .clone();
    // Simulate delayed entry with one second left for native cleanup. The
    // real scoped reference models a descheduled I/O thread after native exit.
    slot.prepare_shutdown(Instant::now() - Duration::from_secs(9));
    let scope = crate::runtime::apple_channel::tests::hold_scope(&slot);
    let first = session.stop();
    assert!(
        matches!(first, Err(RuntimeError::Io(ref error)) if error.kind() == std::io::ErrorKind::TimedOut),
        "{first:?}"
    );
    assert!(session.worker.lease().is_some());
    drop(scope);
    assert!(matches!(session.stop(), Err(RuntimeError::ShutdownForced)));
    assert!(session.worker.lease().is_none());
    assert!(matches!(session.stop(), Err(RuntimeError::ShutdownForced)));
}
#[test]
fn close_during_startup_handshake_reports_confirmed_hard_cleanup_as_unclean() {
    let (t, journal, image) = installation();
    let resources = resource(&serving_peer("startup-wait"));
    let (closed, close) = tokio::sync::watch::channel(false);
    let launcher = std::thread::spawn(move || {
        let worker = RuntimeWorker::new(
            Box::new(AppleBackend::new(resources.path().to_path_buf())),
            journal,
            image,
        )
        .unwrap();
        AppleVmSession::start(worker, 1, Some(close), None)
    });
    let end = Instant::now() + Duration::from_secs(2);
    while !t.path().join("install/startup-wait").exists() {
        assert!(Instant::now() < end);
        std::thread::yield_now();
    }
    closed.send_replace(true);
    assert!(matches!(
        launcher.join().unwrap(),
        Err(RuntimeError::ShutdownForced)
    ));
}
#[test]
fn helper_ack_alone_never_establishes_guest_transport() {
    let (t, journal, image) = installation();
    let resources = resource(&PEER.replace(
        "s.sendall(b'BRVM\\x01')",
        "s.sendall(b'BRVM\\x01')\ng.shutdown(socket.SHUT_WR)",
    ));
    assert!(AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).is_err());
    let reopened = Journal::open(&t.path().join("install")).unwrap();
    assert_eq!(
        reopened.record().phase,
        crate::runtime::journal::StatePhase::Prepared
    );
}
