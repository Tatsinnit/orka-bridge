use bridge_engine::shutdown::TerminalShutdown;
use core_foundation::{
    base::TCFType,
    date::CFAbsoluteTimeGetCurrent,
    runloop::{
        kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext,
        CFRunLoopTimerInvalidate, CFRunLoopTimerRef,
    },
    string::CFString,
};
use objc2::{
    runtime::{AnyObject, ClassBuilder, Sel},
    sel, MainThreadMarker,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use std::{cell::RefCell, ffi::c_void, sync::Arc, time::Duration, time::Instant};

struct Adapter {
    shutdown: Arc<TerminalShutdown>,
    request: Arc<dyn Fn() + Send + Sync>,
    deadline: Option<Instant>,
    timer: Option<CFRunLoopTimer>,
}
thread_local! {
    // All delegate callbacks and timer firings are on AppKit's main thread.
    static ADAPTER: RefCell<Option<Adapter>> = const { RefCell::new(None) };
}

pub fn install(
    shutdown: Arc<TerminalShutdown>,
    request: Arc<dyn Fn() + Send + Sync>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("termination adapter requires AppKit's main thread")?;
    if ADAPTER.with(|slot| slot.borrow().is_some()) {
        return Err("termination adapter already installed".into());
    }
    let app = NSApplication::sharedApplication(mtm);
    // Keep a strong reference while temporarily clearing AppKit's weak delegate.
    let delegate = app
        .delegate()
        .ok_or("Tao application delegate is missing")?;
    let object: &AnyObject = delegate.as_ref();
    let original = object.class();
    let mut subclass = ClassBuilder::new(c"BridgeTerminalAppDelegate", original)
        .ok_or("could not register termination delegate subclass")?;
    // SAFETY: This selector's ABI is -(NSUInteger)applicationShouldTerminate:(NSApplication*).
    // The binding's enum has the exact NSUInteger encoding; no other Tao methods change.
    unsafe {
        subclass.add_method(
            sel!(applicationShouldTerminate:),
            should_terminate as extern "C-unwind" fn(_, _, _) -> _,
        );
    }
    let subclass = subclass.register();
    ADAPTER.with(|slot| {
        *slot.borrow_mut() = Some(Adapter {
            shutdown,
            request,
            deadline: None,
            timer: None,
        });
    });
    // SAFETY: Install runs once on the native main thread before saved startup.
    // This is an immediate subclass of the object's actual class, adds NO ivars,
    // and only overrides the ABI-compatible termination policy. Tao's object
    // identity, ivar offsets, inherited callbacks and deallocator remain intact.
    // AppKit/Tao do not concurrently mutate this main-thread-owned delegate.
    let previous = unsafe { AnyObject::set_class(object, subclass) };
    assert!(
        std::ptr::eq(previous, original),
        "delegate class changed during installation"
    );
    // AppKit caches optional delegate capabilities. A same-object assignment may
    // be optimized away, so clear and restore while our Retained keeps Tao alive.
    app.setDelegate(None);
    app.setDelegate(Some(&delegate));
    Ok(())
}

extern "C-unwind" fn should_terminate(
    _: &AnyObject,
    _: Sel,
    _: &NSApplication,
) -> NSApplicationTerminateReply {
    let request = ADAPTER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let adapter = slot.as_mut().expect("installed termination adapter");
        // Claim exit ownership BEFORE admitting cleanup, including an existing Quit.
        if let Some(clean) = adapter.shutdown.native_termination() {
            diagnose(Some(clean));
            return None;
        }
        if adapter.deadline.is_some() {
            return Some(None); // A deferred reply already owns this termination.
        }
        adapter.deadline = Some(Instant::now() + Duration::from_secs(30));
        let timer = CFRunLoopTimer::new(
            // SAFETY: Core Foundation's clock getter has no preconditions.
            unsafe { CFAbsoluteTimeGetCurrent() } + 0.05,
            0.05,
            0,
            0,
            poll,
            std::ptr::null_mut::<CFRunLoopTimerContext>(),
        );
        let run_loop = CFRunLoop::get_main();
        // A Tauri queue/common-mode callback cannot service AppKit's termination
        // modal loop. Register explicitly in both the modal and ordinary modes.
        let modal = CFString::new("NSModalPanelRunLoopMode");
        run_loop.add_timer(&timer, modal.as_concrete_TypeRef());
        // SAFETY: The exported default run-loop mode is a process-lifetime CFString.
        run_loop.add_timer(&timer, unsafe { kCFRunLoopDefaultMode });
        adapter.timer = Some(timer);
        Some(Some(adapter.request.clone()))
    });
    match request {
        None => {
            #[cfg(test)]
            native_tests::observed_reply(NSApplicationTerminateReply::TerminateNow);
            NSApplicationTerminateReply::TerminateNow
        }
        Some(request) => {
            if let Some(request) = request {
                request(); // Synchronous admission; worker never needs this UI thread.
            }
            #[cfg(test)]
            native_tests::observed_reply(NSApplicationTerminateReply::TerminateLater);
            NSApplicationTerminateReply::TerminateLater
        }
    }
}

extern "C" fn poll(_: CFRunLoopTimerRef, _: *mut c_void) {
    let completion = ADAPTER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let adapter = slot.as_mut().expect("installed termination adapter");
        let result = adapter.shutdown.result();
        if result.is_none() && Instant::now() < adapter.deadline.unwrap() {
            return None;
        }
        adapter.timer.take().map(|timer| (result, timer))
    });
    if let Some((result, timer)) = completion {
        // SAFETY: Our owned timer is live; invalidation removes it from BOTH modes
        // before any reentrant AppKit work, guaranteeing at most one final reply.
        unsafe { CFRunLoopTimerInvalidate(timer.as_concrete_TypeRef()) };
        diagnose(result);
        #[cfg(test)]
        native_tests::observed_poll(result);
        let mtm = MainThreadMarker::new().expect("main-run-loop termination timer");
        // AppKit alone exits, even on failure/deadline. Its eventual status 0 is
        // NOT a clean-shutdown assertion; only the coordinator result is evidence.
        NSApplication::sharedApplication(mtm).replyToApplicationShouldTerminate(true);
    }
}

fn diagnose(result: Option<bool>) {
    match result {
        Some(false) => eprintln!("Native termination: workspace cleanup was unclean."),
        None => eprintln!("Native termination: 30s cleanup deadline expired; stop is unconfirmed."),
        Some(true) => {}
    }
}

// The fixture is a separate executable: AppKit requires the real main thread,
// whereas Rust's test harness runs #[test] bodies on worker threads.
#[cfg(test)]
#[allow(dead_code)] // Called by the standalone native driver, not Rust's test harness.
pub mod native_tests {
    use super::*;
    use bridge_engine::{
        agent_folder::AgentFolder,
        clusters::commands::ClusterSession,
        engine::EngineState,
        local_vm::{LocalFactory, LocalProgress, LocalVm, LocalVmStatus, StartFuture},
        runtime::types::RuntimeError,
        send::SendOwner,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FailedStartup;
    impl LocalFactory for FailedStartup {
        fn launch(
            &self,
            _: tokio::sync::watch::Receiver<bool>,
            _: Arc<dyn Fn(LocalProgress) + Send + Sync>,
        ) -> StartFuture<'_> {
            Box::pin(async { Err(RuntimeError::Unavailable) })
        }
    }

    extern "C" fn terminate(_: CFRunLoopTimerRef, _: *mut c_void) {
        NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).terminate(None);
    }

    pub fn observed_poll(result: Option<bool>) {
        // A default-mode-only timer must fail the real terminate: test.
        assert_eq!(
            CFRunLoop::get_current().current_mode().as_deref(),
            Some("NSModalPanelRunLoopMode")
        );
        eprintln!("native-test:modal-poll");
        if let Some(clean) = result {
            eprintln!("native-test:finished={clean}");
        }
    }

    pub fn observed_reply(reply: NSApplicationTerminateReply) {
        eprintln!("native-test:reply={}", reply.0);
        if reply == NSApplicationTerminateReply::TerminateNow {
            ADAPTER.with(|slot| {
                let result = slot.borrow().as_ref().unwrap().shutdown.result().unwrap();
                eprintln!("native-test:finished={result}");
            });
        }
    }

    pub fn run(mut context: tauri::Context<tauri::Wry>) {
        let case = std::env::args().nth(1).unwrap();
        context.config_mut().app.windows.clear(); // No webview, VM, profile or token.
        context
            .config_mut()
            .identifier
            .push_str(".native-termination-test");
        let shutdown = Arc::new(TerminalShutdown::default());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let vm = if case == "failed" {
            let vm = LocalVm::new(Arc::new(FailedStartup));
            runtime.block_on(async {
                vm.start();
                while vm.status() != LocalVmStatus::Failed {
                    tokio::task::yield_now().await;
                }
            });
            vm
        } else {
            LocalVm::unavailable()
        };
        let engine = EngineState::with_transport(vm.clone());
        let clusters = ClusterSession::default();
        let send = SendOwner::new(Arc::new(AgentFolder::new(None)), clusters.clone());
        let workers = Arc::new(AtomicUsize::new(0));
        let joined = Arc::new(AtomicBool::new(false));
        let request: Arc<dyn Fn() + Send + Sync> = {
            let (shutdown, workers, case) = (shutdown.clone(), workers.clone(), case.clone());
            Arc::new(move || {
                if shutdown.request(&vm, &engine) {
                    assert_eq!(workers.fetch_add(1, Ordering::SeqCst), 0);
                    eprintln!("native-test:worker");
                    if case == "deadline" {
                        std::thread::spawn(|| loop {
                            std::thread::park(); // Hung cleanup: exercise the real 30s bound.
                        });
                        return;
                    }
                    let (shutdown, vm, send, clusters) =
                        (shutdown.clone(), vm.clone(), send.clone(), clusters.clone());
                    let (joined, existing) = (joined.clone(), case == "existing");
                    std::thread::spawn(move || {
                        // Hold existing cleanup until the native request joins it,
                        // independent of how long Tauri takes to initialize.
                        while existing && !joined.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        std::thread::sleep(Duration::from_millis(250));
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        runtime.block_on(shutdown.finish(&clusters, &send, &vm));
                        // Deliberately no AppHandle::exit or Tauri UI queue here.
                    });
                } else {
                    joined.store(true, Ordering::SeqCst);
                }
            })
        };
        if case == "existing" || case == "completed" {
            request();
            if case == "completed" {
                assert_eq!(shutdown.wait_timeout(Duration::from_secs(5)), Some(true));
            }
        }
        let app = tauri::Builder::default()
            .setup(move |_| {
                let ns_app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
                if case == "missing-delegate" {
                    let delegate = ns_app.delegate().unwrap();
                    ns_app.setDelegate(None);
                    assert!(install(shutdown, request).is_err());
                    ns_app.setDelegate(Some(&delegate));
                    eprintln!("native-test:install-rejected");
                    std::process::exit(0);
                }
                let delegate = ns_app.delegate().unwrap();
                let object: &AnyObject = delegate.as_ref();
                let before = object.class();
                install(shutdown, request)?;
                assert!(std::ptr::eq(&*delegate, &*ns_app.delegate().unwrap()));
                assert_eq!(before.instance_size(), object.class().instance_size());
                Ok(())
            })
            .build(context)
            .unwrap();
        app.run(move |_, event| match event {
            tauri::RunEvent::Ready => {
                // Ready holds Tao's event-handler mutex. Real AppKit Quit arrives
                // outside that callback; reentrant terminate would deadlock its
                // inherited applicationWillTerminate -> LoopDestroyed dispatch.
                let timer = CFRunLoopTimer::new(
                    unsafe { CFAbsoluteTimeGetCurrent() },
                    0.0,
                    0,
                    0,
                    terminate,
                    std::ptr::null_mut::<CFRunLoopTimerContext>(),
                );
                // CFRunLoop retains this one-shot timer until its next-turn firing.
                CFRunLoop::get_main().add_timer(&timer, unsafe { kCFRunLoopDefaultMode });
            }
            // Wry maps Tao's LoopDestroyed directly to this event.
            tauri::RunEvent::Exit => eprintln!("native-test:Exit/LoopDestroyed"),
            _ => {}
        });
    }
}
