#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bridge_engine::engine::{types::*, EngineState};
use tauri::Manager;

#[tauri::command]
async fn engine_call(
    request: EngineRequest,
    state: tauri::State<'_, EngineState>,
) -> Result<EngineReply, EngineError> {
    state.call(request).await
}

fn main() {
    tauri::Builder::default()
        .manage(EngineState::configured())
        .invoke_handler(tauri::generate_handler![engine_call])
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                )
            {
                window.state::<EngineState>().shutdown();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Bridge");
}
