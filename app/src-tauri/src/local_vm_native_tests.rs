use super::*;
use crate::runtime::{image_install::open_installed_image, types::WorkspaceCause};
use sha2::{Digest, Sha256};
use std::fs;

mod infrastructure_tests {
    use super::*;
    use bridge_runtime_protocol::*;
    use std::{
        io::{self, Read, Write},
        sync::atomic::{AtomicU8, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Probe {
        state: AtomicU8,
        polls: AtomicUsize,
        stops: AtomicUsize,
        starts: AtomicUsize,
        drops: AtomicUsize,
        fail_stop: bool,
    }
    struct Channel {
        probe: Arc<Probe>,
        reply: io::Cursor<Vec<u8>>,
    }
    impl Read for Channel {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.reply.read(out)
        }
    }
    impl Write for Channel {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let Frame::Request(request) = serde_json::from_slice(bytes).unwrap() else {
                panic!("request")
            };
            assert!(
                matches!(request.body, RequestBody::Readiness {}),
                "startup may only observe readiness"
            );
            self.probe.polls.fetch_add(1, Ordering::SeqCst);
            let state = self.probe.state.load(Ordering::SeqCst);
            self.reply = io::Cursor::new(
                encode(&Frame::Result(WireResult {
                    version: VERSION,
                    boot_nonce: request.boot_nonce,
                    request_id: request.request_id,
                    body: ResultBody::Infrastructure(InfrastructureState {
                        phase: Some(if state == 0 {
                            BootstrapPhase::ClusterReady
                        } else {
                            BootstrapPhase::InfrastructureReady
                        }),
                        service: match state {
                            0 => InfrastructureService::Starting,
                            1 => InfrastructureService::Ready,
                            _ => InfrastructureService::Failed,
                        },
                    }),
                }))
                .unwrap(),
            );
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    struct TestSession(Arc<Probe>, VmClient);
    impl Drop for TestSession {
        fn drop(&mut self) {
            self.0.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl LocalSession for TestSession {
        fn with_client(
            &mut self,
            work: &mut dyn FnMut(Uuid, &mut VmClient) -> Result<(), RuntimeError>,
        ) -> Result<(), RuntimeError> {
            work(Uuid::from_u128(42), &mut self.1)
        }
        fn execute(&mut self, _: EngineRequest) -> TransportFuture<'_> {
            Box::pin(async { unreachable!() })
        }
        fn stop(&mut self) -> Result<(), RuntimeError> {
            self.0.stops.fetch_add(1, Ordering::SeqCst);
            if self.0.fail_stop {
                Err(io::Error::other("stop detail").into())
            } else {
                Ok(())
            }
        }
    }
    struct Factory(Arc<Probe>);
    impl LocalFactory for Factory {
        fn launch(
            &self,
            closed: watch::Receiver<bool>,
            progress: Arc<dyn Fn(LocalProgress) + Send + Sync>,
        ) -> StartFuture<'_> {
            let probe = self.0.clone();
            Box::pin(blocking_start(move |runtime| {
                probe.starts.fetch_add(1, Ordering::SeqCst);
                progress(LocalProgress::Status(LocalVmStatus::Starting));
                let client = VmClient::established(
                    Box::new(Channel {
                        probe: probe.clone(),
                        reply: io::Cursor::new(Vec::new()),
                    }),
                    Uuid::new_v4(),
                )?;
                wait_for_infrastructure(Box::new(TestSession(probe, client)), closed, runtime)
            }))
        }
    }
    async fn phase(vm: &LocalVm, expected: LocalVmStatus) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while vm.status() != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    async fn polled(probe: &Probe) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while probe.polls.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn starting_infrastructure_locks_credentials_and_connect_until_ready() {
        let probe = Arc::new(Probe::default());
        let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::setup::SetupStore::new(Some(temp.path().to_owned())));
        vm.start();
        phase(&vm, LocalVmStatus::Starting).await;
        polled(&probe).await;
        let snapshot =
            serde_json::to_value(setup_status(store.clone(), &vm).await.unwrap()).unwrap();
        assert_eq!(snapshot["localVm"], "starting");
        assert_eq!(snapshot["model"], "locked");
        assert!(vm
            .with_client(|_, _| panic!("credential capture must not be admitted"))
            .await
            .is_err());
        assert!(vm.connect_model(store).await.is_err());
        assert_eq!(vm.status(), LocalVmStatus::Starting);
        probe.state.store(1, Ordering::SeqCst);
        phase(&vm, LocalVmStatus::Ready).await;
        vm.shutdown().await.unwrap();
        assert_eq!(probe.stops.load(Ordering::SeqCst), 1);
        assert!(probe.polls.load(Ordering::SeqCst) >= 2);
    }
    #[tokio::test]
    async fn failed_bootstrap_never_becomes_running_and_preserves_stop_error() {
        for fail_stop in [false, true] {
            let probe = Arc::new(Probe {
                state: AtomicU8::new(2),
                fail_stop,
                ..Probe::default()
            });
            let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
            vm.start();
            phase(&vm, LocalVmStatus::Failed).await;
            let temp = tempfile::tempdir().unwrap();
            let store = Arc::new(crate::setup::SetupStore::new(Some(temp.path().to_owned())));
            let snapshot = serde_json::to_value(setup_status(store, &vm).await.unwrap()).unwrap();
            assert_eq!(snapshot["model"], "locked");
            let error = snapshot["localVmError"].as_str().unwrap();
            assert!(error.contains("infrastructure"), "{error}");
            assert_eq!(error.contains("stop detail"), fail_stop, "{error}");
            assert_eq!(
                snapshot["workspaceFailure"],
                if fail_stop {
                    serde_json::json!({"operation": "stop", "cause": "stopUnconfirmed", "codes": []})
                } else {
                    serde_json::json!({"operation": "services", "cause": "services", "codes": []})
                }
            );
            assert_eq!(probe.stops.load(Ordering::SeqCst), 1);
        }
    }
    #[tokio::test]
    async fn retry_repeats_clean_failures_single_flight_then_reaches_ready() {
        let probe = Arc::new(Probe {
            state: AtomicU8::new(2),
            ..Probe::default()
        });
        let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
        for attempt in 1..=3 {
            vm.start();
            vm.start();
            phase(&vm, LocalVmStatus::Failed).await;
            assert_eq!(probe.starts.load(Ordering::SeqCst), attempt);
        }
        probe.state.store(1, Ordering::SeqCst);
        vm.start();
        phase(&vm, LocalVmStatus::Ready).await;
        assert_eq!(probe.starts.load(Ordering::SeqCst), 4);
        vm.shutdown().await.unwrap();
    }
    #[tokio::test]
    async fn infrastructure_failed_stop_keeps_custody_and_blocks_retry() {
        let probe = Arc::new(Probe {
            state: AtomicU8::new(2),
            fail_stop: true,
            ..Probe::default()
        });
        let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
        vm.start();
        phase(&vm, LocalVmStatus::Failed).await;
        assert_eq!(
            probe.drops.load(Ordering::SeqCst),
            0,
            "unconfirmed session must stay held"
        );
        probe.state.store(1, Ordering::SeqCst);
        vm.start();
        tokio::task::yield_now().await;
        assert_eq!(vm.status(), LocalVmStatus::Failed);
        assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
        assert!(vm.shutdown().await.is_err());
        assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn close_wakes_the_infrastructure_wait_and_stops_once() {
        let probe = Arc::new(Probe::default());
        let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
        vm.start();
        polled(&probe).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), vm.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vm.status(), LocalVmStatus::Closed);
        assert_eq!(probe.stops.load(Ordering::SeqCst), 1);
    }
}

// Native startup must acquire the real Windows inheritance gate after returning
// from async image work, not while its blocking worker is inside block_on.
#[tokio::test]
async fn native_startup_from_tokio_can_enter_the_windows_spawn_window() {
    let result = blocking_start(|runtime| {
        runtime.block_on(async { tokio::task::yield_now().await });
        let _guard = crate::runtime::windows::job::spawn_guard()?;
        Err(std::io::Error::from_raw_os_error(193).into())
    })
    .await;
    assert!(matches!(result, Err(RuntimeError::Io(error)) if error.raw_os_error() == Some(193)));
}

#[cfg(all(debug_assertions, windows))]
use crate::runtime::windows::backend::boot_tests::fixture;

#[cfg(all(debug_assertions, windows))]
#[tokio::test]
async fn windows_session_startup_inside_tokio_reaches_child_creation_without_panicking() {
    use crate::runtime::{image_install::materialize_image, image_types::ImageTarget};
    let temp = tempfile::tempdir_in(std::env::var_os("USERPROFILE").unwrap()).unwrap();
    let root = temp.path().join("install");
    let journal = Journal::create(&root, BackendKind::Qemu, b"pc-q35-11.0".to_vec()).unwrap();
    let verified = fixture::verified(temp.path(), ImageTarget::WindowsAmd64);
    let target = journal
        .image_target(&verified.image().identity().state_compatibility_id)
        .unwrap();
    let (_sender, closed) = watch::channel(false);
    let image = materialize_image(&verified, &target, closed, |_| {})
        .await
        .unwrap();
    let runtime_root = temp.path().join("runtime");
    fs::create_dir_all(runtime_root.join("bin")).unwrap();
    let bytes = b"inert, not an executable";
    let pins: Vec<_> = ["bin/qemu-img.exe", "bin/qemu-system-x86_64.exe"]
        .into_iter()
        .map(|name| {
            fs::write(runtime_root.join(name), bytes).unwrap();
            FilePin {
                name: name.into(),
                bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            }
        })
        .collect();
    let inputs = Arc::new(verify_windows_runtime(&pins, runtime_root).unwrap());
    let launch_root = root.clone();
    let result = blocking_start(move |_| {
        let native = start_windows_session(
            inputs,
            journal,
            Arc::new(image),
            &launch_root,
            0,
            Arc::new(Mutex::new(JournalCustody(None))),
        )?;
        Ok(Box::new(native) as Box<dyn LocalSession>)
    })
    .await;
    // The real backend reaches CreateProcessW, which refuses the inert fixture.
    // A panic is mapped to Unavailable instead, so it cannot satisfy this check.
    match result {
        Err(RuntimeError::Startup(context)) => {
            assert_eq!(context.operation, WorkspaceOperation::Prepare);
            let RuntimeError::Io(error) = &context.error else {
                panic!(
                    "expected native executable refusal, got {:?}",
                    context.error
                );
            };
            assert!(matches!(error.raw_os_error(), Some(193 | 216)), "{error}");
            assert!(context.diagnostic.contains("Run qemu-img"));
            assert!(context.diagnostic.contains("Details:"));
        }
        Err(error) => panic!("expected native executable refusal, got {error:?}"),
        Ok(_) => panic!("inert fixture must not start a session"),
    }
    assert_eq!(fs::metadata(root.join("state.disk")).unwrap().len(), 0);
    let evidence: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("startup-failure-")
                .then_some(path)
        })
        .collect();
    assert_eq!(evidence.len(), 1);
    assert!(evidence[0].join("failure.json").is_file());
    assert!(evidence[0].join("qemu-stderr.txt").is_file());
}

#[test]
fn failed_start_retains_stage_stderr_and_existing_installation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("boot.log"), b"").unwrap();
    fs::write(root.join("record.json"), b"retained record").unwrap();
    let error = RuntimeError::Io(std::io::Error::other("control connection refused"));
    let reported = retain_start_failure(
        root,
        "Connect guest control",
        error,
        Ok(b"qemu: diagnostic\n".to_vec()),
        WorkspaceOperation::Start,
    );
    let text = failure_reason(&reported);
    assert!(text.contains("Connect guest control"));
    assert!(text.contains("control connection refused"));
    let failures: Vec<_> = fs::read_dir(root)
        .unwrap()
        .filter_map(|e| {
            let path = e.unwrap().path();
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("startup-failure-")
                .then_some(path)
        })
        .collect();
    assert_eq!(failures.len(), 1);
    let details: serde_json::Value =
        serde_json::from_slice(&fs::read(failures[0].join("failure.json")).unwrap()).unwrap();
    assert_eq!(details["stage"], "Connect guest control");
    assert!(details["reason"]
        .as_str()
        .unwrap()
        .contains("control connection refused"));
    assert_eq!(
        fs::read(failures[0].join("qemu-stderr.txt")).unwrap(),
        b"qemu: diagnostic\n"
    );
    assert!(text.contains(failures[0].to_str().unwrap()));
    assert_eq!(
        fs::read(root.join("record.json")).unwrap(),
        b"retained record"
    );
    assert_eq!(fs::read(root.join("boot.log")).unwrap(), b"");
}

#[test]
fn retained_diagnostics_keep_the_typed_error_operation_and_real_os_code() {
    let denied_code = if cfg!(windows) { 5 } else { 13 };
    let temp = tempfile::tempdir().unwrap();
    let noisy = b"NoSpace code: 5 (os error 28) token=ghp_secret".to_vec();
    let typed = |error| {
        let reported = retain_start_failure(
            temp.path(),
            "Connect guest control",
            error,
            Ok(noisy.clone()),
            WorkspaceOperation::Start,
        );
        assert!(failure_reason(&reported).contains("ghp_secret"));
        let f = WorkspaceFailure::from_error(&reported, WorkspaceOperation::Prepare);
        (f.operation, f.cause, f.codes)
    };
    assert_eq!(
        typed(RuntimeError::Io(std::io::Error::from_raw_os_error(
            denied_code
        ))),
        (
            WorkspaceOperation::Start,
            WorkspaceCause::Permission,
            vec![denied_code as u32]
        )
    );
    assert_eq!(
        typed(RuntimeError::Io(std::io::Error::other("code: 5 NoSpace"))),
        (WorkspaceOperation::Start, WorkspaceCause::Unknown, vec![])
    );
    // Unconfirmed cleanup stays the top-level quarantine error, not a wrapper.
    let held = retain_start_failure(
        temp.path(),
        "Shut down guest",
        RuntimeError::StopUnconfirmed("held".into()),
        Ok(Vec::new()),
        WorkspaceOperation::Stop,
    );
    assert!(matches!(held, RuntimeError::StopUnconfirmed(_)));
}
#[test]
fn authenticated_nested_manifest_is_part_of_the_complete_runtime_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("bin")).unwrap();
    fs::create_dir(root.join("provenance")).unwrap();
    let bytes = b"inert runtime fixture";
    let sha = format!("{:x}", Sha256::digest(bytes));
    for name in ["qemu-img.exe", "qemu-system-x86_64.exe"] {
        fs::write(root.join("bin").join(name), bytes).unwrap();
    }
    let manifest = serde_json::to_vec(&serde_json::json!({"files":[
        {"path":"bin/qemu-img.exe","bytes":bytes.len(),"sha256":sha},
        {"path":"bin/qemu-system-x86_64.exe","bytes":bytes.len(),"sha256":sha}
    ]}))
    .unwrap();
    // Test authority is independently retained here, not read from that file.
    let pin = FilePin {
        name: "provenance/runtime-manifest.json".into(),
        bytes: manifest.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&manifest)),
    };
    fs::write(root.join(&pin.name), &manifest).unwrap();
    let pins = read_runtime_manifest(root, &pin).unwrap();
    assert_eq!(pins.len(), 3);
    assert_eq!(pins.last().unwrap().name, pin.name);
    let inputs = verify_windows_runtime(&pins, root.to_owned()).unwrap();
    inputs.recheck_runtime().unwrap();
    fs::write(root.join("extra"), b"not authorized").unwrap();
    assert!(inputs.recheck_runtime().is_err());
    fs::remove_file(root.join("extra")).unwrap();
    drop(inputs); // Windows retains no-write handles until the capability ends.
    let mut changed = manifest.clone();
    changed[0] ^= 1;
    fs::write(root.join(&pin.name), changed).unwrap();
    assert!(read_runtime_manifest(root, &pin).is_err());
    assert!(verify_windows_runtime(&pins, root.to_owned()).is_err());
}
#[test]
fn missing_manifest_and_untrusted_self_description_cannot_supply_runtime_pins() {
    let temp = tempfile::tempdir().unwrap();
    assert!(read_runtime_manifest(temp.path(), &runtime_manifest_pin()).is_err());
    fs::create_dir(temp.path().join("provenance")).unwrap();
    fs::write(
        temp.path().join("provenance/runtime-manifest.json"),
        br#"{"files":[],"sha256":"pretend"}"#,
    )
    .unwrap();
    assert!(read_runtime_manifest(temp.path(), &runtime_manifest_pin()).is_err());
}
#[test]
fn reopen_never_creates_or_repairs_missing_or_incomplete_installations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("local-vm-v1");
    let (_fixture, profile, _) = release_image_tests::profile();
    let mut source = release_image_tests::source(profile);
    assert!(reopen(&root, &mut source).unwrap().is_none());
    assert!(!root.exists());
    let journal = Journal::create(&root, BackendKind::Qemu, b"pc-q35-11.0".to_vec()).unwrap();
    drop(journal);
    let before = fs::read(root.join("record.json")).unwrap();
    assert!(reopen(&root, &mut source).is_err());
    assert_eq!(fs::read(root.join("record.json")).unwrap(), before);
    assert_eq!(fs::metadata(root.join("state.disk")).unwrap().len(), 0);
    assert!(!root.join("image-binding.json").exists());
}
use crate::runtime as fixture_runtime;
mod image_fixture {
    include!("../tests/support/runtime_image_fixture.rs");
}
#[tokio::test]
async fn original_creation_journal_survives_partial_and_post_materialization_cancel() {
    use crate::runtime::{
        image_install::materialize_image,
        image_types::{ImageProgress, ImageTarget},
    };
    let temp = tempfile::tempdir().unwrap();
    let image = image_fixture::verified(temp.path(), ImageTarget::WindowsAmd64);
    let compatibility = &image.image().identity().state_compatibility_id;
    let root = temp.path().join("install");
    let mut custody = JournalCustody(Some(
        Journal::create(&root, BackendKind::Qemu, vec![1]).unwrap(),
    ));
    let (stop, closed) = watch::channel(false);
    let target = custody
        .0
        .as_ref()
        .unwrap()
        .image_target(compatibility)
        .unwrap();
    assert!(materialize_image(&image, &target, closed.clone(), |event| {
        if matches!(event, ImageProgress::Materializing { bytes, .. } if bytes > 0) {
            stop.send_replace(true);
        }
    })
    .await
    .is_err());
    assert!(matches!(
        custody.take_for_launch(&closed),
        Err(RuntimeError::Closed)
    ));
    stop.send_replace(false);
    let target = custody
        .0
        .as_ref()
        .unwrap()
        .image_target(compatibility)
        .unwrap();
    materialize_image(&image, &target, closed.clone(), |_| {})
        .await
        .unwrap();
    drop(target);
    // Cancellation after the final image binding, before native session transfer.
    stop.send_replace(true);
    assert!(matches!(
        custody.take_for_launch(&closed),
        Err(RuntimeError::Closed)
    ));
    stop.send_replace(false);
    let target = custody
        .0
        .as_ref()
        .unwrap()
        .image_target(compatibility)
        .unwrap();
    assert!(open_installed_image(&target).is_ok());
    materialize_image(&image, &target, closed.clone(), |_| {})
        .await
        .unwrap();
    drop(target);
    drop(custody.take_for_launch(&closed).unwrap());
    // Reopening must not gain the creation permission the held original had.
    let reopened = Journal::open(&root).unwrap();
    assert!(materialize_image(
        &image,
        &reopened.image_target(compatibility).unwrap(),
        closed,
        |_| {}
    )
    .await
    .is_err());
}
mod retry_custody_tests {
    use super::*;
    use crate::runtime::{types::*, RuntimeWorker};
    use bridge_runtime_protocol::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Backend(Arc<AtomicUsize>, bool, bool);
    impl VmBackend for Backend {
        fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            fs::OpenOptions::new()
                .write(true)
                .open(&spec.state_path)?
                .set_len(64)?;
            Ok(())
        }
        fn start(&mut self, _: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
            if self.1 {
                Err(RuntimeError::Unavailable)
            } else {
                Ok(BackendHandle::new(()))
            }
        }
        fn stop(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
            if self.2 {
                Err(RuntimeError::Unavailable)
            } else {
                Ok(())
            }
        }
        fn pause(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
            unreachable!()
        }
        fn resume(&mut self, _: &BackendHandle) -> Result<(), RuntimeError> {
            unreachable!()
        }
    }
    #[tokio::test]
    async fn failed_boot_keeps_original_creation_and_reuses_image_without_source() {
        use crate::runtime::{image_install::materialize_image, image_types::ImageTarget};
        for (target, backend) in [
            (ImageTarget::WindowsAmd64, BackendKind::Qemu),
            (ImageTarget::MacosArm64, BackendKind::Apple),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let verified = image_fixture::verified(temp.path(), target);
            let compatibility = verified.image().identity().state_compatibility_id.clone();
            let root = temp.path().join("install");
            let journal = Journal::create(&root, backend, vec![1]).unwrap();
            let (_sender, closed) = watch::channel(false);
            materialize_image(
                &verified,
                &journal.image_target(&compatibility).unwrap(),
                closed.clone(),
                |_| {},
            )
            .await
            .unwrap();
            drop(verified);
            fs::remove_dir_all(temp.path().join("cache")).unwrap();
            let custody = Arc::new(Mutex::new(JournalCustody(Some(journal))));
            let creates = Arc::new(AtomicUsize::new(0));
            for attempt in 0..4 {
                let mut held = custody.lock().unwrap();
                let journal = held.take_for_launch(&closed).unwrap();
                let image = Arc::new(
                    open_installed_image(&journal.image_target(&compatibility).unwrap()).unwrap(),
                );
                let installation = image.installation_id();
                drop(held);
                let mut worker = RuntimeWorker::new(
                    Box::new(Backend(creates.clone(), attempt < 2, false)),
                    journal,
                    image,
                )
                .unwrap()
                .retain_journal(custody.clone());
                if attempt < 2 {
                    assert!(worker.start().is_err());
                } else {
                    worker.start().unwrap();
                    let nonce = worker.lease().unwrap().spec.boot_nonce;
                    let request = worker
                        .boot_request(
                            &StageHello {
                                version: VERSION,
                                stage: Stage::Early,
                                guest_nonce: Uuid::new_v4(),
                                run_cleanup: false,
                                accepted_boot_nonce: None,
                            },
                            0,
                        )
                        .unwrap();
                    let RequestBody::Boot(descriptor) = request.body else {
                        panic!("boot")
                    };
                    assert_eq!(
                        descriptor.state_creation,
                        if attempt == 2 {
                            StateCreation::New
                        } else {
                            StateCreation::Existing
                        }
                    );
                    assert_eq!(descriptor.installation_id, installation);
                    assert_eq!(descriptor.boot_nonce, nonce);
                }
                drop(worker);
                assert!(
                    matches!(Journal::open(&root), Err(RuntimeError::Busy)),
                    "owner stays held between attempts"
                );
            }
            assert_eq!(creates.load(Ordering::SeqCst), 1);
        }
    }
    #[tokio::test]
    async fn worker_failed_stop_never_returns_journal_for_retry() {
        use crate::runtime::{image_install::materialize_image, image_types::ImageTarget};
        let temp = tempfile::tempdir().unwrap();
        let verified = image_fixture::verified(temp.path(), ImageTarget::WindowsAmd64);
        let root = temp.path().join("install");
        let journal = Journal::create(&root, BackendKind::Qemu, vec![1]).unwrap();
        let (_sender, closed) = watch::channel(false);
        let image = materialize_image(
            &verified,
            &journal.image_target("fixture-state-v1").unwrap(),
            closed,
            |_| {},
        )
        .await
        .unwrap();
        let custody = Arc::new(Mutex::new(JournalCustody(None)));
        let mut worker = RuntimeWorker::new(
            Box::new(Backend(Arc::new(AtomicUsize::new(0)), false, true)),
            journal,
            Arc::new(image),
        )
        .unwrap()
        .retain_journal(custody.clone());
        worker.start().unwrap();
        let error = worker.startup_failed(RuntimeError::Unavailable);
        assert!(matches!(error, RuntimeError::StopUnconfirmed(_)));
        assert!(custody.lock().unwrap().0.is_none());
        assert!(matches!(Journal::open(&root), Err(RuntimeError::Busy)));
    }
}
mod release_image_tests {
    use super::super::image::*;
    use super::*;
    use crate::runtime::image_types::*;

    pub(super) fn profile() -> (tempfile::TempDir, ReleaseImage, ImageIdentity) {
        let temp = tempfile::tempdir().unwrap();
        let verified = image_fixture::verified(temp.path(), ImageTarget::WindowsAmd64);
        let identity = verified.image().identity().clone();
        let profile = ReleaseImage {
            pin: ImagePin {
                release_id: "fixture".into(),
                manifest_url: "https://fixture.invalid/manifest.json".into(),
                manifest_sha256: identity.manifest_sha256.clone(),
                accepted_schema_sha256: format!(
                    "{:x}",
                    Sha256::digest(include_bytes!("../../../images/manifest.schema.json"))
                ),
                target: ImageTarget::WindowsAmd64,
                asset_base: "https://fixture.invalid/".into(),
                redirect_origins: vec![],
                limits: TransferLimits {
                    compressed_bytes: 8192,
                    disk_bytes: 1024 * 1024,
                    kernel_bytes: 8192,
                    initrd_bytes: 8192,
                },
            },
            manifest: verified.image().manifest_bytes().to_vec(),
            image_id: identity.image_id.clone(),
            state_compatibility_id: identity.state_compatibility_id.clone(),
        };
        (temp, profile, identity)
    }

    #[test]
    fn release_selection_never_uses_development_paths() {
        for target in [ImageTarget::WindowsAmd64, ImageTarget::MacosArm64] {
            // Release selection stays pinned despite a development override or retained root.
            assert!(matches!(
                select_source(
                    false,
                    true,
                    true,
                    shipping_image(target).map(|p| (
                        p,
                        HostTarget {
                            target,
                            host_version: "13.0".into(),
                            app_version: "1.0.0".into(),
                        }
                    ))
                ),
                Some(ImageSource::Pinned { .. })
            ));
        }
        #[cfg(debug_assertions)]
        assert!(matches!(
            select_source(true, true, false, None),
            Some(ImageSource::Development)
        ));
    }

    #[test]
    fn shipping_profiles_resolve_the_exact_paired_images() {
        for (target, digest, disk_bytes) in [
            (
                ImageTarget::MacosArm64,
                "60a36d2f38643ab1337f0849dfd3d2db570e445c1a4116612fe25ab7c6d1e144",
                17179869184,
            ),
            (
                ImageTarget::WindowsAmd64,
                "4d62e8f907b9557a704d14a265cb67de2f9020396ddb1e1a8b1fb938c9fda9ec",
                3780706304,
            ),
        ] {
            let profile = shipping_image(target).expect("approved platform image");
            assert_eq!(profile.pin.manifest_sha256, digest);
            assert_eq!(format!("{:x}", Sha256::digest(&profile.manifest)), digest);
            assert_eq!(profile.pin.limits.disk_bytes, disk_bytes);
            if (target == ImageTarget::MacosArm64
                && cfg!(all(target_os = "macos", target_arch = "aarch64")))
                || (target == ImageTarget::WindowsAmd64
                    && cfg!(all(windows, target_arch = "x86_64")))
            {
                assert_eq!(crate::catalog::IMAGE_BYTES, Some(disk_bytes));
            }
            let resolved = crate::runtime::manifest::resolve_image(
                &profile.pin,
                &profile.manifest,
                &HostTarget {
                    target,
                    host_version: "100.0".into(),
                    app_version: "1.0.0".into(),
                },
            )
            .unwrap();
            profile.validate(resolved.identity(), target).unwrap();
            let policy: serde_json::Value =
                serde_json::from_slice(include_bytes!("../../../images/release-set.json")).unwrap();
            let tag = policy["imageRelease"]["tag"].as_str().unwrap();
            let base =
                format!("https://github.com/orka-agents/orka-bridge/releases/download/{tag}/");
            assert_eq!(profile.pin.release_id, tag);
            assert_eq!(profile.pin.asset_base, base);
            let platform = if target == ImageTarget::MacosArm64 {
                "macos-arm64"
            } else {
                "windows-amd64"
            };
            assert_eq!(
                profile.pin.manifest_url,
                format!(
                    "{}{}",
                    base,
                    policy["images"][platform]["manifestName"].as_str().unwrap()
                )
            );
            assert_eq!(
                profile.pin.redirect_origins,
                ["https://release-assets.githubusercontent.com"]
            );
            assert_eq!(
                resolved.identity().state_compatibility_id,
                "engineering-d86ebac3"
            );
            let mut other = resolved.identity().clone();
            other.manifest_sha256 = "f".repeat(64);
            assert_eq!(profile.validate(&other, target), Err(ImageError::Integrity));
        }
    }

    #[test]
    fn selected_identity_requires_manifest_image_target_and_compatibility() {
        let (_temp, profile, identity) = profile();
        assert!(profile
            .validate(&identity, ImageTarget::WindowsAmd64)
            .is_ok());
        for field in ["manifest", "image", "compatibility"] {
            let mut other = identity.clone();
            match field {
                "manifest" => other.manifest_sha256 = "f".repeat(64),
                "image" => other.image_id = "other".into(),
                _ => other.state_compatibility_id = "other".into(),
            }
            assert_eq!(
                profile.validate(&other, ImageTarget::WindowsAmd64),
                Err(ImageError::Integrity)
            );
        }
        assert_eq!(
            profile.validate(&identity, ImageTarget::MacosArm64),
            Err(ImageError::Integrity)
        );
    }

    pub(super) fn source(profile: ReleaseImage) -> ImageSource {
        ImageSource::Pinned {
            host: HostTarget {
                target: profile.pin.target,
                host_version: "13.0".into(),
                app_version: "1.0.0".into(),
            },
            profile: Box::new(profile),
        }
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn pinned_startup_materializes_through_owned_journal() {
        let (temp, profile, expected) = profile();
        let root = temp.path().join("install");
        let mut custody = JournalCustody(None);
        let (_stop, closed) = watch::channel(false);
        let runtime = runtime();
        let image = prepare_installation(
            &source(profile),
            &root,
            &temp.path().join("cache"),
            &mut custody,
            closed,
            |_| {},
            || Journal::create(&root, BackendKind::Qemu, vec![1]),
            runtime.handle(),
        )
        .unwrap();
        assert_eq!(image.identity().manifest_sha256, expected.manifest_sha256);
        assert!(root.join("image-binding.json").exists());
        assert!(matches!(Journal::open(&root), Err(RuntimeError::Busy)));
        assert!(custody.0.is_some());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_pinned_selection_survives_first_install_and_restart() {
        let (temp, profile, expected) = profile();
        let root = temp.path().join("install");
        let cache = temp.path().join("cache");
        let runtime = runtime();
        let (_stop, closed) = watch::channel(false);
        let mut custody = JournalCustody(None);
        for existing in [false, true] {
            let host = HostTarget {
                target: profile.pin.target,
                host_version: "13.0".into(),
                app_version: "1.0.0".into(),
            };
            let selected =
                select_source(true, false, existing, Some((profile.clone(), host))).unwrap();
            let image = prepare_installation(
                &selected,
                &root,
                &cache,
                &mut custody,
                closed.clone(),
                |_| {},
                || Journal::create(&root, BackendKind::Qemu, vec![1]),
                runtime.handle(),
            )
            .unwrap();
            assert_eq!(image.identity().manifest_sha256, expected.manifest_sha256);
            drop(custody.0.take());
            if !existing {
                fs::remove_dir_all(&cache).unwrap();
            }
        }
        assert!(!cache.exists());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn debug_reopen_only_accepts_engineering_identity_when_a_pin_is_available() {
        for engineering in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let verified = image_fixture::verified_with_identity(
                temp.path(),
                ImageTarget::WindowsAmd64,
                Some(crate::dev_image::STATE_COMPATIBILITY_ID),
                engineering.then_some("engineering-windows-amd64"),
            );
            let root = temp.path().join("install");
            let journal = Journal::create(&root, BackendKind::Qemu, vec![1]).unwrap();
            let (_stop, closed) = watch::channel(false);
            let runtime = runtime();
            runtime
                .block_on(crate::runtime::image_install::materialize_image(
                    &verified,
                    &journal
                        .image_target(crate::dev_image::STATE_COMPATIBILITY_ID)
                        .unwrap(),
                    closed.clone(),
                    |_| {},
                ))
                .unwrap();
            drop((verified, journal));
            fs::remove_dir_all(temp.path().join("cache")).unwrap();
            let (_release_fixture, mut profile, _) = profile();
            // Both actual paired profiles use this same compatibility ID. It alone
            // must not turn a verified development binding into a release binding.
            profile.state_compatibility_id = crate::dev_image::STATE_COMPATIBILITY_ID.into();
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&profile.manifest).unwrap();
            manifest["stateCompatibilityId"] = profile.state_compatibility_id.clone().into();
            manifest["target"]["minimumHostVersion"] = "12.0".into();
            profile.manifest = serde_json::to_vec(&manifest).unwrap();
            profile.pin.manifest_sha256 = format!("{:x}", Sha256::digest(&profile.manifest));
            let mut custody = JournalCustody(None);
            let image = prepare_installation(
                &source(profile),
                &root,
                &temp.path().join("cache"),
                &mut custody,
                closed,
                |_| {},
                || panic!("retained identity checks must not create"),
                runtime.handle(),
            );
            if engineering {
                assert_eq!(
                    image.unwrap().identity().image_id,
                    "engineering-windows-amd64"
                );
            } else {
                assert!(matches!(
                    image,
                    Err(RuntimeError::Image(ImageError::Integrity))
                ));
            }
            assert!(!temp.path().join("cache").exists());
        }
    }

    #[test]
    fn pinned_reopen_rechecks_minimum_versions_without_cache() {
        let (temp, profile, _) = profile();
        let root = temp.path().join("install");
        let cache = temp.path().join("cache");
        let runtime = runtime();
        let (_stop, closed) = watch::channel(false);
        let mut custody = JournalCustody(None);
        prepare_installation(
            &source(profile.clone()),
            &root,
            &cache,
            &mut custody,
            closed.clone(),
            |_| {},
            || Journal::create(&root, BackendKind::Qemu, vec![1]),
            runtime.handle(),
        )
        .unwrap();
        fs::remove_dir_all(&cache).unwrap();
        for (host_version, app_version, reason) in [
            ("1.0", "1.0.0", Incompatibility::HostVersion),
            ("13.0", "0.0.0", Incompatibility::AppVersion),
        ] {
            let selected = ImageSource::Pinned {
                profile: Box::new(profile.clone()),
                host: HostTarget {
                    target: profile.pin.target,
                    host_version: host_version.into(),
                    app_version: app_version.into(),
                },
            };
            let result = prepare_installation(
                &selected,
                &root,
                &cache,
                &mut custody,
                closed.clone(),
                |_| {},
                || panic!("version rejection cannot replace the install"),
                runtime.handle(),
            );
            assert!(
                matches!(result, Err(RuntimeError::Image(ImageError::Incompatible(actual))) if actual == reason)
            );
            assert!(!cache.exists());
            // Exercise both already-held retry custody and committed reopen.
            drop(custody.0.take());
        }
    }

    #[test]
    fn pinned_reopen_rejects_self_consistent_boot_binding_tampering() {
        use sha2::{Digest, Sha256};
        for changed in ["kernel", "initrd", "rootSelector", "buildSourceCommit"] {
            let (temp, profile, _) = profile();
            let root = temp.path().join("install");
            let cache = temp.path().join("cache");
            let runtime = runtime();
            let (_stop, closed) = watch::channel(false);
            let mut custody = JournalCustody(None);
            prepare_installation(
                &source(profile.clone()),
                &root,
                &cache,
                &mut custody,
                closed.clone(),
                |_| {},
                || Journal::create(&root, BackendKind::Qemu, vec![1]),
                runtime.handle(),
            )
            .unwrap();
            fs::remove_dir_all(&cache).unwrap();
            let binding_path = root.join("image-binding.json");
            let mut binding: serde_json::Value =
                serde_json::from_slice(&fs::read(&binding_path).unwrap()).unwrap();
            if changed == "kernel" || changed == "initrd" {
                let path = root.join(format!("image/1/os.{changed}"));
                let mut bytes = fs::read(&path).unwrap();
                bytes[0] ^= 1;
                fs::write(&path, &bytes).unwrap();
                let hash = format!("{:x}", Sha256::digest(&bytes));
                binding[changed]["initialSha256"] = hash.clone().into();
                binding["boot"][changed]["sha256"] = hash.into();
            } else if changed == "rootSelector" {
                let original = binding["boot"][changed]["value"].as_str().unwrap();
                let mut replacement = original.to_owned();
                replacement.replace_range(..1, if original.starts_with('0') { "1" } else { "0" });
                let command = binding["boot"]["commandLine"]
                    .as_str()
                    .unwrap()
                    .replace(original, &replacement);
                binding["boot"][changed]["value"] = replacement.into();
                binding["boot"]["commandLine"] = command.into();
            } else {
                let original = binding["identity"][changed].as_str().unwrap();
                let mut replacement = original.to_owned();
                replacement.replace_range(..1, if original.starts_with('a') { "b" } else { "a" });
                binding["identity"][changed] = replacement.into();
            }
            fs::write(&binding_path, serde_json::to_vec(&binding).unwrap()).unwrap();
            let journal = custody.0.as_ref().unwrap();
            // File/binding self-consistency is not proof of the selected pin.
            let opened = open_installed_image(
                &journal
                    .image_target(&profile.state_compatibility_id)
                    .unwrap(),
            );
            assert!(opened.is_ok(), "{changed}: {opened:?}");
            drop(custody.0.take());
            let result = prepare_installation(
                &source(profile),
                &root,
                &cache,
                &mut custody,
                closed,
                |_| {},
                || panic!("a mismatch cannot replace the install"),
                runtime.handle(),
            );
            assert!(
                matches!(result, Err(RuntimeError::Image(ImageError::Integrity))),
                "{changed}"
            );
            assert!(!cache.exists());
        }
    }

    #[test]
    fn pinned_reopen_keeps_writable_os_disk_policy() {
        let (temp, profile, _) = profile();
        let root = temp.path().join("install");
        let cache = temp.path().join("cache");
        let runtime = runtime();
        let (_stop, closed) = watch::channel(false);
        let mut custody = JournalCustody(None);
        prepare_installation(
            &source(profile.clone()),
            &root,
            &cache,
            &mut custody,
            closed.clone(),
            |_| {},
            || Journal::create(&root, BackendKind::Qemu, vec![1]),
            runtime.handle(),
        )
        .unwrap();
        let disk = root.join("image/1/os.disk");
        let mut bytes = fs::read(&disk).unwrap();
        bytes[0] ^= 1;
        fs::write(&disk, bytes).unwrap();
        drop(custody.0.take());
        fs::remove_dir_all(&cache).unwrap();
        prepare_installation(
            &source(profile),
            &root,
            &cache,
            &mut custody,
            closed,
            |_| {},
            || panic!("retained OS writes do not require replacement"),
            runtime.handle(),
        )
        .unwrap();
        assert!(!cache.exists());
    }

    #[test]
    fn pinned_reopen_without_cache_does_not_create_cache() {
        let (temp, profile, expected) = profile();
        let root = temp.path().join("install");
        let cache = temp.path().join("cache");
        let mut custody = JournalCustody(None);
        let (_stop, closed) = watch::channel(false);
        let runtime = runtime();
        let source = source(profile);
        prepare_installation(
            &source,
            &root,
            &cache,
            &mut custody,
            closed.clone(),
            |_| {},
            || Journal::create(&root, BackendKind::Qemu, vec![1]),
            runtime.handle(),
        )
        .unwrap();
        drop(custody.0.take());
        fs::remove_dir_all(&cache).unwrap();
        let image = prepare_installation(
            &source,
            &root,
            &cache,
            &mut custody,
            closed,
            |_| {},
            || panic!("reopen must not create"),
            runtime.handle(),
        )
        .unwrap();
        assert_eq!(image.identity().manifest_sha256, expected.manifest_sha256);
        assert!(!cache.exists());
    }

    #[test]
    fn pinned_reopen_rejects_other_image_without_writes() {
        let (temp, profile, _) = profile();
        let root = temp.path().join("install");
        let cache = temp.path().join("cache");
        let mut custody = JournalCustody(None);
        let (_stop, closed) = watch::channel(false);
        let runtime = runtime();
        prepare_installation(
            &source(profile.clone()),
            &root,
            &cache,
            &mut custody,
            closed.clone(),
            |_| {},
            || Journal::create(&root, BackendKind::Qemu, vec![1]),
            runtime.handle(),
        )
        .unwrap();
        let before = fs::read(root.join("image-binding.json")).unwrap();
        fs::remove_dir_all(&cache).unwrap();
        // Both retained retry custody and a new factory must reject the other
        // image, even though its state compatibility string is identical.
        for retained in [true, false] {
            if !retained {
                drop(custody.0.take());
            }
            let mut other = profile.clone();
            other.pin.manifest_sha256 = "f".repeat(64);
            let result = prepare_installation(
                &source(other),
                &root,
                &cache,
                &mut custody,
                closed.clone(),
                |_| {},
                || panic!("mismatch must not replace installation"),
                runtime.handle(),
            );
            assert!(matches!(
                result,
                Err(RuntimeError::Image(ImageError::Integrity))
            ));
            assert_eq!(fs::read(root.join("image-binding.json")).unwrap(), before);
            assert!(!cache.exists());
        }
    }

    #[test]
    fn cancelled_pinned_startup_retains_original_owner() {
        let (temp, profile, _) = profile();
        let root = temp.path().join("install");
        let cache = temp.path().join("cache");
        let mut custody = JournalCustody(None);
        let (stop, closed) = watch::channel(false);
        let runtime = runtime();
        let source = source(profile);
        let result = prepare_installation(
            &source,
            &root,
            &cache,
            &mut custody,
            closed.clone(),
            |event| {
                if matches!(event, LocalProgress::Image(ImageProgress::Materializing { bytes, .. }) if bytes > 0)
                {
                    stop.send_replace(true);
                }
            },
            || Journal::create(&root, BackendKind::Qemu, vec![1]),
            runtime.handle(),
        );
        assert!(matches!(
            result,
            Err(RuntimeError::Image(ImageError::Cancelled))
        ));
        assert!(custody.0.is_some());
        assert!(matches!(Journal::open(&root), Err(RuntimeError::Busy)));
        stop.send_replace(false);
        prepare_installation(
            &source,
            &root,
            &cache,
            &mut custody,
            closed,
            |_| {},
            || panic!("retry must retain original creation authority"),
            runtime.handle(),
        )
        .unwrap();
        assert!(root.join("image-binding.json").exists());
    }

    #[cfg(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    #[test]
    fn native_host_facts_come_from_the_running_os() {
        #[cfg(windows)]
        let target = ImageTarget::WindowsAmd64;
        #[cfg(target_os = "macos")]
        let target = ImageTarget::MacosArm64;
        let host = native_host_target(target).unwrap();
        assert_eq!(host.target, target);
        assert_eq!(host.app_version, env!("CARGO_PKG_VERSION"));
        assert!(checked_host_version(&host.host_version).is_ok());
    }

    #[test]
    fn host_version_must_be_observed_not_invented() {
        assert_eq!(checked_host_version("27.2\0"), Ok("27.2".into()));
        for value in ["", "unknown", "13", "13.0.bad", "13.0\0extra"] {
            assert_eq!(checked_host_version(value), Err(ImageError::Unsupported));
        }
        #[cfg(not(any(windows, all(target_os = "macos", target_arch = "aarch64"))))]
        assert!(matches!(
            native_host_target(ImageTarget::MacosArm64),
            Err(ImageError::Unsupported)
        ));
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn release_factory_does_not_activate_from_existing_state() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("local-vm-v1");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("record.json"), b"not an approved image").unwrap();
    assert_eq!(
        configured(Some(data.path().into()), Some(data.path().into())).status(),
        if crate::catalog::IMAGE_AVAILABLE {
            LocalVmStatus::Idle
        } else {
            LocalVmStatus::NotIntegrated
        }
    );
    assert_eq!(
        fs::read(root.join("record.json")).unwrap(),
        b"not an approved image"
    );
    assert!(!data.path().join("image-cache-v1").exists());
}

#[test]
fn factory_configuration_without_native_paths_cannot_activate_a_session() {
    assert_eq!(
        configured(None, None).status(),
        LocalVmStatus::NotIntegrated
    );
}
