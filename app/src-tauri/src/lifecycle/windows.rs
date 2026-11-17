//! Intercept session end before Tao's hidden target can exit without cleanup.
use bridge_engine::shutdown::TerminalShutdown;
use std::{
    io::{self, Write},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::Threading::GetCurrentThreadId,
    UI::{
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            EnumThreadWindows, GetClassNameW, WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION,
        },
    },
};

const SUBCLASS_ID: usize = 1;
struct Context {
    shutdown: Arc<TerminalShutdown>,
    request: Arc<dyn Fn() + Send + Sync>,
}

/// Must run on Tao's native event thread, before starting the VM.
pub fn install(
    shutdown: Arc<TerminalShutdown>,
    request: Arc<dyn Fn() + Send + Sync>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut target: HWND = std::ptr::null_mut();
    unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(find_target),
            &mut target as *mut HWND as LPARAM,
        );
        if target.is_null() {
            return Err(io::Error::other(
                "Tao thread event-target window not found on native thread",
            )
            .into());
        }
        let mut existing = 0;
        if GetWindowSubclass(target, Some(subclass), SUBCLASS_ID, &mut existing) != 0 {
            return Err(io::Error::other("Windows session-end subclass already installed").into());
        }
        let context = Arc::into_raw(Arc::new(Context { shutdown, request }));
        if SetWindowSubclass(target, Some(subclass), SUBCLASS_ID, context as usize) == 0 {
            drop(Arc::from_raw(context));
            // SetWindowSubclass does not promise to set GetLastError.
            return Err(
                io::Error::other("Could not subclass Tao's thread event-target window").into(),
            );
        }
    }
    Ok(())
}

unsafe extern "system" fn find_target(hwnd: HWND, found: LPARAM) -> i32 {
    let mut name = [0u16; 64];
    let length = GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32);
    // Exact class in pinned Tao 0.37.2, not a title or a renderer window.
    if name
        .iter()
        .copied()
        .take(length as usize)
        .eq("Tao Thread Event Target".encode_utf16())
    {
        *(found as *mut HWND) = hwnd;
        return 0;
    }
    1
}

fn unclean(message: &str) -> ! {
    // Unlike eprintln!, a failed diagnostic write must not panic across WinAPI.
    let _ = writeln!(
        io::stderr(),
        "Windows session end: {message}; exiting uncleanly."
    );
    std::process::exit(1)
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| {
        let pointer = data as *const Context;
        // Hold a separate reference through callbacks/DefSubclassProc: either can
        // reenter and destroy the window, releasing the subclass's raw reference.
        Arc::increment_strong_count(pointer);
        let context = Arc::from_raw(pointer);
        match message {
            WM_QUERYENDSESSION => 1, // Consent promptly; another app can still cancel.
            WM_ENDSESSION if wparam == 0 => 0,
            WM_ENDSESSION => {
                let start = Instant::now();
                let result = context.shutdown.native_termination();
                if !context.shutdown.closing() {
                    (context.request)();
                }
                let result = result.or_else(|| {
                    context
                        .shutdown
                        .wait_timeout(Duration::from_secs(4).saturating_sub(start.elapsed()))
                });
                // Consume confirmed end. DefSubclassProc would let Tao exit(0).
                match result {
                    Some(true) => std::process::exit(0),
                    Some(false) => unclean("cleanup failed"),
                    None => unclean("cleanup unconfirmed after the four-second deadline"),
                }
            }
            WM_NCDESTROY => {
                if RemoveWindowSubclass(hwnd, Some(subclass), id) != 0 {
                    drop(Arc::from_raw(pointer));
                } // On removal failure retain the raw reference, never leave dangling data.
                DefSubclassProc(hwnd, message, wparam, lparam)
            }
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    })) {
        Ok(result) => result,
        Err(payload) => {
            std::mem::forget(payload); // Even a user-defined panic payload can panic on drop.
            unclean("session-end handler panicked; cleanup unconfirmed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bridge_engine::{
        agent_folder::AgentFolder,
        clusters::commands::ClusterSession,
        engine::{types::*, EngineState, TransportFuture},
        local_vm::{
            LocalFactory, LocalProgress, LocalSession, LocalVm, LocalVmStatus, StartFuture,
        },
        runtime::types::RuntimeError,
        send::SendOwner,
    };
    use std::{
        path::PathBuf,
        process::{Command, Stdio},
        sync::atomic::{AtomicUsize, Ordering},
        time::Instant,
    };
    use tauri::Manager;
    use tokio::sync::watch;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    struct Probe {
        marker: PathBuf,
        native_thread: u32,
        calls: AtomicUsize,
        stops: AtomicUsize,
        clean: bool,
    }
    struct Factory(Arc<Probe>);
    struct Session(Arc<Probe>);
    impl LocalFactory for Factory {
        fn launch(
            &self,
            _: watch::Receiver<bool>,
            _: Arc<dyn Fn(LocalProgress) + Send + Sync>,
        ) -> StartFuture<'_> {
            let probe = self.0.clone();
            Box::pin(async move { Ok(Box::new(Session(probe)) as Box<dyn LocalSession>) })
        }
    }
    impl LocalSession for Session {
        fn execute(&mut self, _: EngineRequest) -> TransportFuture<'_> {
            Box::pin(async { Err(EngineError::new(ErrorKind::Unavailable)) })
        }
        fn stop(&mut self) -> Result<(), RuntimeError> {
            assert_ne!(unsafe { GetCurrentThreadId() }, self.0.native_thread);
            let stops = self.0.stops.fetch_add(1, Ordering::SeqCst) + 1;
            std::fs::write(
                &self.0.marker,
                format!(
                    "calls={} stops={stops}",
                    self.0.calls.load(Ordering::SeqCst)
                ),
            )
            .unwrap();
            if self.0.clean {
                Ok(())
            } else {
                Err(RuntimeError::Unavailable)
            }
        }
    }

    // A lower subclass makes accidental forwarding observable, even if Tao changes.
    unsafe extern "system" fn downstream(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        match message {
            WM_QUERYENDSESSION => 19,
            WM_ENDSESSION => std::process::exit(92),
            WM_APP => 47,
            _ => DefSubclassProc(hwnd, message, wparam, lparam),
        }
    }

    // Removing interception, starting on query/cancel, forwarding confirmed end,
    // bypassing async completion, duplicating admission or reporting failure clean
    // each changes an exit code, marker or native message result below.
    #[test]
    fn native_session_messages_in_subprocesses() {
        let temp = tempfile::tempdir().unwrap();
        let child_test = format!(
            "{}::native_session_child",
            module_path!().split_once("::").unwrap().1
        );
        for (case, code, diagnostic, stopped) in [
            ("missing", 0, "", false),
            ("missing_icon", 0, "", false),
            ("menu_quit", 1, "cleanup was not reported as clean", true),
            ("query_cancel", 0, "", false),
            ("clean", 0, "", true),
            ("existing", 0, "", true),
            ("reentrant_destroy", 0, "", true),
            ("failed", 1, "cleanup failed", true),
            ("timeout", 1, "cleanup unconfirmed", false),
            ("panic", 1, "session-end handler panicked", false),
        ] {
            let marker = temp.path().join(case);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &child_test, "--nocapture"])
                .env("BRIDGE_WINDOWS_SESSION_TEST", case)
                .env("BRIDGE_WINDOWS_SESSION_MARKER", &marker)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let until = Instant::now() + Duration::from_secs(20);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= until {
                    child.kill().unwrap();
                    let output = child.wait_with_output().unwrap();
                    panic!("{case} hung: {output:?}");
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.code(), Some(code), "{case}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "{case}: {output:?}"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(diagnostic),
                "{case}: {output:?}"
            );
            if stopped {
                assert_eq!(
                    std::fs::read_to_string(marker).unwrap(),
                    "calls=1 stops=1",
                    "{case}"
                );
            } else {
                assert!(!marker.exists(), "{case} must not stop a VM");
            }
        }
    }

    #[test]
    fn native_session_child() {
        let Ok(case) = std::env::var("BRIDGE_WINDOWS_SESSION_TEST") else {
            return;
        };
        let shutdown = Arc::new(TerminalShutdown::default());
        if case == "missing" {
            assert!(install(shutdown, Arc::new(|| panic!("must not request"))).is_err());
            return;
        }
        // Real Tao hidden target, but no webview, saved setup, VM or OS logout.
        let mut context = tauri::generate_context!();
        context.config_mut().app.windows.clear();
        context
            .config_mut()
            .identifier
            .push_str(".native-session-test");
        if case == "missing_icon" {
            context.set_default_window_icon(None);
        }
        eprintln!("native-session {case}: building native app");
        let app = tauri::Builder::default()
            .any_thread()
            .build(context)
            .unwrap();
        eprintln!("native-session {case}: native app built");
        if case == "missing_icon" {
            let error = super::super::install(app.handle(), shutdown, true).unwrap_err();
            assert!(error.to_string().contains("tray icon is missing"));
            assert!(app.try_state::<super::super::HideOnClose>().is_none());
            return;
        }
        let probe = Arc::new(Probe {
            marker: std::env::var_os("BRIDGE_WINDOWS_SESSION_MARKER")
                .unwrap()
                .into(),
            native_thread: unsafe { GetCurrentThreadId() },
            calls: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            clean: !matches!(case.as_str(), "failed" | "menu_quit"),
        });
        if case == "menu_quit" {
            let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
            app.manage(vm.clone());
            app.manage(EngineState::with_transport(vm.clone()));
            let clusters = ClusterSession::default();
            app.manage(clusters.clone());
            app.manage(SendOwner::new(Arc::new(AgentFolder::new(None)), clusters));
            app.manage(shutdown.clone());
            // Observe the real event before the production handler; never call stop here.
            app.on_menu_event(move |_, event| {
                if event.id().as_ref() == "quit" {
                    probe.calls.fetch_add(1, Ordering::SeqCst);
                }
            });
            eprintln!("native-session menu_quit: building menu window");
            // Quit is a native-menu contract; WebView2 startup is not part of it.
            let window = tauri::WindowBuilder::new(&app, "menu-probe")
                .visible(false)
                .build()
                .unwrap();
            assert!(window.webviews().is_empty());
            eprintln!("native-session menu_quit: menu window built");
            super::super::install(app.handle(), shutdown, true).unwrap();
            tauri::async_runtime::block_on(async {
                vm.start();
                tokio::time::timeout(Duration::from_secs(2), async {
                    while vm.status() != LocalVmStatus::Ready {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            });
            let hwnd = window.hwnd().unwrap().0 as HWND;
            unsafe {
                let menu = GetMenu(hwnd);
                assert!(!menu.is_null());
                let file = GetSubMenu(menu, 0);
                assert!(!file.is_null());
                assert_eq!(GetMenuItemCount(file), 1);
                let quit = GetMenuItemID(file, 0);
                assert_ne!(quit, u32::MAX);
                assert_ne!(PostMessageW(hwnd, WM_COMMAND, quit as WPARAM, 0), 0);
            }
            eprintln!("native-session menu_quit: native Quit posted; entering event loop");
            // Predefined Quit posts WM_QUIT directly: it exits 0 without this VM's marker.
            app.run(|_, _| {});
            panic!("native File > Quit must exit through terminal cleanup");
        }
        let mut hwnd = std::ptr::null_mut();
        loop {
            hwnd = unsafe {
                FindWindowExW(
                    std::ptr::null_mut(),
                    hwnd,
                    windows_sys::core::w!("Tao Thread Event Target"),
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null());
            if unsafe {
                GetWindowThreadProcessId(hwnd, std::ptr::null_mut()) == GetCurrentThreadId()
            } {
                break;
            }
        }
        assert_ne!(
            unsafe { SetWindowSubclass(hwnd, Some(downstream), 41, 0) },
            0
        );
        let vm = LocalVm::new(Arc::new(Factory(probe.clone())));
        let engine = EngineState::with_transport(vm.clone());
        let clusters = ClusterSession::default();
        let send = SendOwner::new(Arc::new(AgentFolder::new(None)), clusters.clone());
        let (trigger, mut work) = watch::channel(false);
        let (ready, started) = std::sync::mpsc::channel();
        let worker_shutdown = shutdown.clone();
        let worker_vm = vm.clone();
        let timeout = case == "timeout";
        let worker = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    worker_vm.start();
                    tokio::time::timeout(Duration::from_secs(2), async {
                        while worker_vm.status() != LocalVmStatus::Ready {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .unwrap();
                    ready.send(()).unwrap();
                    if work.wait_for(|requested| *requested).await.is_err() {
                        return;
                    }
                    if timeout {
                        std::future::pending::<()>().await;
                    }
                    // Must progress on this separate runtime while SendMessage blocks the native thread.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    worker_shutdown.finish(&clusters, &send, &worker_vm).await;
                    worker_shutdown.exit_unless_native(|_| std::process::exit(93));
                });
        });
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        let callback_shutdown = shutdown.clone();
        let callback_probe = probe.clone();
        let panic = case == "panic";
        let destroy = case == "reentrant_destroy";
        let target = hwnd as usize;
        let request: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            assert!(!panic, "injected callback panic");
            callback_probe.calls.fetch_add(1, Ordering::SeqCst);
            if callback_shutdown.request(&vm, &engine) {
                trigger.send_replace(true);
            }
            if destroy {
                assert_ne!(unsafe { DestroyWindow(target as HWND) }, 0);
            }
        });
        let weak = Arc::downgrade(&request);
        install(shutdown.clone(), request).unwrap();
        assert!(
            install(shutdown.clone(), Arc::new(|| {})).is_err(),
            "duplicate installation must not replace context"
        );
        assert_eq!(unsafe { SendMessageW(hwnd, WM_APP, 0, 0) }, 47);
        let start = Instant::now();
        for _ in 0..2 {
            assert_eq!(unsafe { SendMessageW(hwnd, WM_QUERYENDSESSION, 0, 0) }, 1);
            assert_eq!(unsafe { SendMessageW(hwnd, WM_ENDSESSION, 0, 0) }, 0);
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "queries must not wait for cleanup"
        );
        if case == "query_cancel" {
            assert!(!shutdown.closing());
            assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
            assert_ne!(unsafe { DestroyWindow(hwnd) }, 0);
            assert!(
                weak.upgrade().is_none(),
                "WM_NCDESTROY must release subclass context"
            );
            worker.join().unwrap();
            return;
        }
        // Admit ordinary Quit just before native dispatch; it must join that worker.
        if case == "existing" {
            weak.upgrade().unwrap()();
        }
        unsafe {
            SendMessageW(hwnd, WM_ENDSESSION, 1, 0);
        }
        panic!("confirmed end must exit without returning to Tao");
    }
}
