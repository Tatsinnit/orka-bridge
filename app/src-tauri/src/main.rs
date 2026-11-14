#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use bridge_engine::agent_folder::{AgentFolder, AgentPair, DocumentId};
use bridge_engine::agents::Agents;
use bridge_engine::chat::{ChatError, ChatOwner, ChatSnapshot};
use bridge_engine::engine::{types::*, EngineState};
use bridge_engine::local_vm::{self, ExamplePair, LocalVm, LocalVmStatus};
use bridge_engine::send::{SendOwner, SendStatus};
use bridge_engine::setup::{SetupError, SetupSnapshot, SetupStore};
use bridge_engine::shutdown::TerminalShutdown;
use std::sync::Arc;
use tauri::Manager;

mod lifecycle;

#[tauri::command]
async fn engine_call(
    request: EngineRequest,
    state: tauri::State<'_, EngineState>,
) -> Result<EngineReply, EngineError> {
    state.call(request).await
}
#[tauri::command]
async fn setup_status(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<SetupSnapshot, SetupError> {
    local_vm::setup_status(Arc::clone(state.inner()), vm.inner()).await
}
#[tauri::command]
async fn setup_select_local(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<SetupSnapshot, SetupError> {
    local_vm::select_local(Arc::clone(state.inner()), vm.inner()).await
}
#[tauri::command]
async fn setup_cancel_local(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<SetupSnapshot, SetupError> {
    local_vm::cancel_local(Arc::clone(state.inner()), vm.inner()).await
}
#[tauri::command]
async fn setup_change_local(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<SetupSnapshot, SetupError> {
    local_vm::change_local(Arc::clone(state.inner()), vm.inner()).await
}
#[tauri::command]
fn setup_requirements() -> Option<bridge_engine::setup::SetupRequirements> {
    bridge_engine::setup::setup_requirements()
}
#[tauri::command]
async fn setup_preflight(
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<Option<bridge_engine::setup::preflight::Preflight>, &'static str> {
    if vm.status() == LocalVmStatus::NotIntegrated
        || bridge_engine::setup::setup_requirements().is_none()
    {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(bridge_engine::setup::preflight::observe)
        .await
        .map_err(|_| "Computer requirements could not be read.")
}
#[tauri::command]
async fn setup_open_help(topic: bridge_engine::setup::help::HelpTopic) -> Result<(), &'static str> {
    tauri::async_runtime::spawn_blocking(move || bridge_engine::setup::help::open_help(topic))
        .await
        .map_err(|_| "Could not open setup help.")?
}
#[tauri::command]
fn setup_example() -> ExamplePair {
    local_vm::example()
}

async fn document_folder(
    agents: Arc<Agents>,
    id: DocumentId,
) -> Result<Arc<AgentFolder>, &'static str> {
    tauri::async_runtime::spawn_blocking(move || agents.get(id))
        .await
        .map_err(|_| "The agent is unavailable.")?
}
#[tauri::command]
async fn overview_files(
    state: tauri::State<'_, Arc<Agents>>,
    run: tauri::State<'_, Arc<bridge_engine::example_run::ExampleRun>>,
) -> Result<bridge_engine::overview::SavedFiles, &'static str> {
    let agents = state.inner().clone();
    let run = run.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        bridge_engine::overview::saved_files_with_history(&agents, &run)
    })
    .await
    .map_err(|_| "Could not read saved files.")
}
#[tauri::command]
async fn overview_agents(
    vm: tauri::State<'_, Arc<LocalVm>>,
    run: tauri::State<'_, Arc<bridge_engine::example_run::ExampleRun>>,
) -> Result<bridge_engine::overview::LiveAgents, &'static str> {
    Ok(bridge_engine::overview::live_agents(vm.inner().clone(), run.inner().clone()).await)
}
#[tauri::command]
async fn overview_places(
    session: tauri::State<'_, bridge_engine::clusters::commands::ClusterSession>,
    send: tauri::State<'_, Arc<SendOwner>>,
) -> Result<bridge_engine::overview_places::Places, &'static str> {
    let send = send.inner().clone();
    let history = tauri::async_runtime::spawn_blocking(move || send.history())
        .await
        .ok()
        .and_then(Result::ok);
    Ok(bridge_engine::overview_places::list(session.inner(), history).await)
}
#[tauri::command]
async fn agents_list(
    state: tauri::State<'_, Arc<Agents>>,
) -> Result<Vec<bridge_engine::agents::AgentEntry>, &'static str> {
    let agents = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agents.list())
        .await
        .map_err(|_| "The agents are unavailable.")?
}
#[tauri::command]
async fn agent_selected(state: tauri::State<'_, Arc<Agents>>) -> Result<DocumentId, &'static str> {
    let agents = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agents.selected())
        .await
        .map_err(|_| "The agents are unavailable.")?
}
#[tauri::command]
async fn agent_open(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<Agents>>,
) -> Result<AgentPair, &'static str> {
    let agents = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agents.get(document_id)?.open())
        .await
        .map_err(|_| "The local agent folder is unavailable.")?
}
#[tauri::command]
async fn agent_save(
    document_id: DocumentId,
    content: String,
    state: tauri::State<'_, Arc<Agents>>,
) -> Result<AgentPair, &'static str> {
    let agents = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || agents.get(document_id)?.save(content))
        .await
        .map_err(|_| "The local agent folder is unavailable.")?
}

#[tauri::command]
async fn agent_reveal(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<Agents>>,
) -> Result<(), &'static str> {
    let agents = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        bridge_engine::agent_reveal::reveal(&agents, document_id)
    })
    .await
    .map_err(|_| "Could not show the agent folder.")?
}

#[tauri::command]
async fn workspace_catalog(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<bridge_engine::catalog::CatalogReply, &'static str> {
    vm.workspace_catalog(Arc::clone(state.inner())).await
}
#[derive(serde::Serialize)]
struct ModelReply {
    ready: bool,
}
#[tauri::command]
async fn setup_connect_model(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<ModelReply, SetupError> {
    let model = vm.connect_model(Arc::clone(state.inner())).await?;
    Ok(ModelReply { ready: model.ready })
}

#[tauri::command]
async fn setup_remove_model(
    state: tauri::State<'_, Arc<SetupStore>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<ModelReply, SetupError> {
    let model = vm.remove_model(Arc::clone(state.inner())).await?;
    Ok(ModelReply { ready: model.ready })
}

#[tauri::command]
fn agent_run_status(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<bridge_engine::example_run::ExampleRun>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> bridge_engine::example_run::RunSnapshot {
    state.status_document(vm.inner(), document_id)
}
#[tauri::command]
async fn agent_run(
    document_id: DocumentId,
    bundled_example: Option<bool>,
    agents: tauri::State<'_, Arc<Agents>>,
    state: tauri::State<'_, Arc<bridge_engine::example_run::ExampleRun>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<bridge_engine::example_run::RunSnapshot, &'static str> {
    if bundled_example == Some(true) {
        return state.start_first_test(vm.inner().clone(), document_id);
    }
    let folder = document_folder(agents.inner().clone(), document_id).await?;
    Ok(state.start_document(vm.inner().clone(), folder))
}

#[tauri::command]
fn agent_chat_status(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<ChatOwner>>,
) -> ChatSnapshot {
    state.status(document_id)
}
#[tauri::command]
async fn agent_chat_send(
    document_id: DocumentId,
    message: String,
    agents: tauri::State<'_, Arc<Agents>>,
    state: tauri::State<'_, Arc<ChatOwner>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<ChatSnapshot, ChatError> {
    let folder = document_folder(agents.inner().clone(), document_id)
        .await
        .map_err(|_| ChatError::Source)?;
    state.send(vm.inner().clone(), folder, message)
}
#[tauri::command]
fn agent_chat_reconnect(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<ChatOwner>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<ChatSnapshot, ChatError> {
    state.reconnect(vm.inner().clone(), document_id)
}
#[tauri::command]
fn agent_chat_stop(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<ChatOwner>>,
    vm: tauri::State<'_, Arc<LocalVm>>,
) -> Result<ChatSnapshot, ChatError> {
    state.stop(vm.inner().clone(), document_id)
}
#[tauri::command]
fn agent_chat_new(
    document_id: DocumentId,
    state: tauri::State<'_, Arc<ChatOwner>>,
) -> Result<ChatSnapshot, ChatError> {
    state.new_conversation(document_id)
}

#[tauri::command]
fn cluster_places(
    state: tauri::State<'_, bridge_engine::clusters::commands::ClusterSession>,
) -> bridge_engine::clusters::commands::ClusterPlaceSnapshot {
    state.places()
}
#[tauri::command]
async fn cluster_contexts(
    state: tauri::State<'_, bridge_engine::clusters::commands::ClusterSession>,
) -> Result<Vec<bridge_engine::clusters::ContextView>, bridge_engine::clusters::ClusterFailure> {
    state.contexts().await
}
#[tauri::command]
async fn cluster_reload(
    state: tauri::State<'_, bridge_engine::clusters::commands::ClusterSession>,
) -> Result<Vec<bridge_engine::clusters::ContextView>, bridge_engine::clusters::ClusterFailure> {
    state.reload().await
}
#[tauri::command]
async fn cluster_discover(
    context_id: String,
    state: tauri::State<'_, bridge_engine::clusters::commands::ClusterSession>,
) -> Result<Vec<bridge_engine::clusters::TargetView>, bridge_engine::clusters::ClusterFailure> {
    state.discover(&context_id).await
}
#[tauri::command]
async fn cluster_use(
    target_id: String,
    sa_name: Option<String>,
    state: tauri::State<'_, bridge_engine::clusters::commands::ClusterSession>,
) -> Result<bridge_engine::clusters::commands::AccessView, &'static str> {
    Ok(state.use_target(&target_id, sa_name).await)
}

#[tauri::command]
async fn send_preflight(
    document_id: DocumentId,
    target_id: String,
    provider_id: String,
    agents: tauri::State<'_, Arc<Agents>>,
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<bridge_engine::send::SendCheck, &'static str> {
    let folder = document_folder(agents.inner().clone(), document_id).await?;
    Ok(state
        .preflight_document(folder, &target_id, &provider_id)
        .await)
}
#[tauri::command]
async fn send_start(
    document_id: DocumentId,
    target_id: String,
    provider_id: String,
    expected_source: bridge_runtime_protocol::SnapshotId,
    agents: tauri::State<'_, Arc<Agents>>,
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<SendStatus, &'static str> {
    let folder = document_folder(agents.inner().clone(), document_id).await?;
    Ok(state
        .start_document(folder, &target_id, &provider_id, expected_source)
        .await)
}
#[tauri::command]
fn send_status(document_id: DocumentId, state: tauri::State<'_, Arc<SendOwner>>) -> SendStatus {
    state.status_document(document_id)
}
#[tauri::command]
async fn send_history(
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<Vec<bridge_engine::send_summary::AttemptSummary>, &'static str> {
    let owner = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || owner.history())
        .await
        .map_err(|_| "Saved Send summaries could not be read.")?
        .map_err(|_| {
            "Saved Send summaries could not be read. They are unavailable, not passing results."
        })
}
#[tauri::command]
async fn send_history_details(
    send_id: String,
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<bridge_engine::send_summary::DetailView, &'static str> {
    let owner = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || owner.history_details(&send_id))
        .await
        .map_err(|_| "storage")?
}
#[tauri::command]
async fn send_history_remove(
    send_id: String,
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<bridge_engine::send_summary::DetailView, &'static str> {
    state.history_remove(&send_id).await
}
#[tauri::command]
async fn send_history_check(
    send_id: String,
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<bridge_engine::send_summary::DetailView, &'static str> {
    state.inner().history_check(&send_id).await
}
#[tauri::command]
async fn send_stop(
    document_id: DocumentId,
    send_id: String,
    state: tauri::State<'_, Arc<SendOwner>>,
) -> Result<SendStatus, &'static str> {
    state.stop_document(document_id, &send_id).await
}

fn main() {
    // Distinguish a native WebView startup stall from a frontend/navigation failure in debug walks.
    #[cfg(debug_assertions)]
    eprintln!("bridge-startup: main");
    let shutdown = Arc::new(TerminalShutdown::default());
    tauri::Builder::default()
        .on_page_load(|_, load| {
            if cfg!(debug_assertions) {
                eprintln!("bridge-startup: page {:?}", load.event());
            }
        })
        .manage(shutdown.clone())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            lifecycle::show(app)
        }))
        .setup(move |app| {
            #[cfg(debug_assertions)]
            eprintln!("bridge-startup: setup");
            let store = Arc::new(SetupStore::new(app.path().app_data_dir().ok()));
            let vm = local_vm::configured(
                app.path().resource_dir().ok(),
                app.path().app_local_data_dir().ok(),
            );
            #[cfg(debug_assertions)]
            eprintln!("bridge-startup: local VM configured");
            app.manage(if vm.status() == LocalVmStatus::NotIntegrated {
                EngineState::configured()
            } else {
                EngineState::with_transport(vm.clone())
            });
            app.manage(store.clone());
            app.manage(vm.clone());
            app.manage(ChatOwner::new(
                tauri::async_runtime::handle().inner().clone(),
            ));
            let session = bridge_engine::clusters::commands::ClusterSession::default();
            app.manage(session.clone());
            let agents = Agents::new(app.path().home_dir().ok(), app.path().app_data_dir().ok());
            let folder = agents.sample();
            app.manage(Arc::new(agents));
            app.manage(SendOwner::with_history_root(
                folder.clone(),
                session,
                app.path().app_local_data_dir().ok(),
            ));
            app.manage(bridge_engine::example_run::ExampleRun::with_history_root(
                tauri::async_runtime::handle().inner().clone(),
                bridge_engine::local_vm::MODEL_IMAGE_AVAILABLE,
                folder,
                app.path().app_local_data_dir().ok(),
            ));
            #[cfg(debug_assertions)]
            eprintln!("bridge-startup: installing lifecycle");
            lifecycle::install(
                app.handle(),
                shutdown.clone(),
                vm.status() != LocalVmStatus::NotIntegrated,
            )?;
            #[cfg(debug_assertions)]
            eprintln!("bridge-startup: ready");
            tauri::async_runtime::spawn(async move {
                if local_vm::resume_selected(store, &vm).await.is_err() {
                    eprintln!("The saved setup choice could not be read.");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_call,
            setup_status,
            setup_select_local,
            setup_cancel_local,
            setup_change_local,
            setup_requirements,
            setup_preflight,
            setup_open_help,
            setup_example,
            setup_connect_model,
            setup_remove_model,
            agent_run_status,
            agent_run,
            agent_chat_status,
            agent_chat_send,
            agent_chat_reconnect,
            agent_chat_stop,
            agent_chat_new,
            agents_list,
            overview_files,
            overview_agents,
            overview_places,
            agent_selected,
            agent_open,
            agent_save,
            agent_reveal,
            workspace_catalog,
            cluster_places,
            cluster_contexts,
            cluster_reload,
            cluster_discover,
            cluster_use,
            send_preflight,
            send_start,
            send_status,
            send_history,
            send_history_details,
            send_history_remove,
            send_history_check,
            send_stop
        ])
        .build(tauri::generate_context!())
        .expect("error while building Bridge")
        .run(|app, event| {
            if app.state::<Arc<TerminalShutdown>>().completed() {
                return;
            }
            let requested = match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::CloseRequested { api, .. },
                    ..
                } if label == "main" => {
                    api.prevent_close();
                    if app.state::<lifecycle::HideOnClose>().0 {
                        if let Some(window) = app.get_webview_window("main") {
                            if window.hide().is_ok() {
                                return;
                            }
                        }
                        eprintln!("Could not hide Bridge; stopping the workspace instead.");
                    }
                    true
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    lifecycle::show(app);
                    false
                }
                tauri::RunEvent::ExitRequested { api, .. } => {
                    api.prevent_exit();
                    true
                }
                _ => false,
            };
            if !requested {
                return;
            }
            lifecycle::stop(app);
        });
}
