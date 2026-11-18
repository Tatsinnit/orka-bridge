use bridge_engine::{
    engine::EngineState, local_vm::LocalVm, send::SendOwner, shutdown::TerminalShutdown,
};
use std::sync::Arc;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

mod close_notice;
use close_notice::{CloseAction, CloseNotice};
use tauri_plugin_dialog::DialogExt;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub struct HideOnClose(pub bool);

fn hide_or_stop(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.hide().is_ok() {
            return;
        }
    }
    eprintln!("Could not hide Bridge; stopping the workspace instead.");
    stop(app);
}

/// Explain close-to-tray once, without blocking the native event loop or changing Quit.
pub fn hide(app: &tauri::AppHandle) {
    if app.state::<Arc<TerminalShutdown>>().closing() {
        return;
    }
    let running =
        app.state::<Arc<LocalVm>>().status() == bridge_engine::local_vm::LocalVmStatus::Ready;
    match app.state::<CloseNotice>().admit(running) {
        CloseAction::Quit => stop(app),
        CloseAction::Hide => hide_or_stop(app),
        CloseAction::Pending => {}
        CloseAction::Explain => {
            let Some(window) = app.get_webview_window("main") else {
                hide_or_stop(app);
                return;
            };
            let handle = app.clone();
            let location = if cfg!(target_os = "macos") {
                "menu bar"
            } else {
                "tray"
            };
            app.dialog()
                .message(format!(
                "Bridge keeps your workspace running in the {location}. Quit from there to stop it."
            ))
                .title("Bridge is still running")
                .parent(&window)
                .show(move |acknowledged| {
                    // Quit can win while the information dialog is open.
                    let shutdown = handle.state::<Arc<TerminalShutdown>>();
                    if shutdown.closing() || shutdown.completed() {
                        return;
                    }
                    if let Err(error) = handle.state::<CloseNotice>().finish(acknowledged) {
                        eprintln!("Could not remember Bridge's close notice: {error}");
                    }
                    hide_or_stop(&handle);
                });
        }
    }
}

/// Activation only exposes the existing webview; it never re-enters VM setup.
pub fn show(app: &tauri::AppHandle) {
    if app.state::<Arc<TerminalShutdown>>().closing() {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        if let Err(error) = window
            .unminimize()
            .and_then(|_| window.show())
            .and_then(|_| window.set_focus())
        {
            eprintln!("Could not open Bridge: {error}");
        }
    }
}

/// Admission is barred on the caller's thread, before the one cleanup worker is queued.
pub fn stop(app: &tauri::AppHandle) {
    let shutdown = app.state::<Arc<TerminalShutdown>>().inner().clone();
    let vm = app.state::<Arc<LocalVm>>().inner().clone();
    if !shutdown.request(&vm, &app.state::<EngineState>()) {
        return;
    }
    let send = app.state::<Arc<SendOwner>>().inner().clone();
    let clusters = app
        .state::<bridge_engine::clusters::commands::ClusterSession>()
        .inner()
        .clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        shutdown.finish(&clusters, &send, &vm).await;
        shutdown.exit_unless_native(|clean| handle.exit(if clean { 0 } else { 1 }));
    });
}

pub fn install(
    app: &tauri::AppHandle,
    shutdown: Arc<TerminalShutdown>,
    integrated: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Hooks and a usable menu must exist before hidden mode or saved startup is admitted.
    #[cfg(any(windows, target_os = "macos"))]
    {
        let handle = app.clone();
        let request: Arc<dyn Fn() + Send + Sync> = Arc::new(move || stop(&handle));
        #[cfg(windows)]
        windows::install(shutdown, request)?;
        #[cfg(target_os = "macos")]
        macos::install(shutdown, request)?;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = shutdown;
    if integrated {
        #[cfg(windows)]
        {
            // Muda's predefined Windows Quit posts WM_QUIT without ExitRequested.
            // A custom item must bar admission and join cleanup before leaving Tao.
            let quit = MenuItem::with_id(app, "quit", "Quit Bridge", true, None::<&str>)?;
            let file = tauri::menu::Submenu::with_items(app, "File", true, &[&quit])?;
            app.set_menu(Menu::with_items(app, &[&file])?)?;
            app.on_menu_event(|app, event| {
                if event.id().as_ref() == "quit" {
                    stop(app);
                }
            });
        }
        let open = MenuItem::with_id(app, "open", "Open Bridge", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "stop-quit", "Quit Bridge", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        let icon = app
            .default_window_icon()
            .ok_or("Bridge's tray icon is missing")?
            .clone();
        TrayIconBuilder::with_id("bridge")
            .icon(icon)
            .icon_as_template(cfg!(target_os = "macos"))
            .tooltip("Bridge")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| match event.id().as_ref() {
                "open" => show(app),
                "stop-quit" => stop(app),
                _ => {}
            })
            .build(app)?;
    }
    app.manage(CloseNotice::new(app.path().app_data_dir().ok()));
    app.manage(HideOnClose(integrated));
    Ok(())
}
