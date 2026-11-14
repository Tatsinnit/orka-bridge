extern crate self as bridge_engine;
#[path = "../../../app/src-tauri/src/agent_folder.rs"]
pub mod agent_folder;
#[path = "../../../app/src-tauri/src/agents.rs"]
pub mod agents;
#[path = "../../../app/src-tauri/src/catalog.rs"]
pub mod catalog;
#[cfg(debug_assertions)]
#[path = "../../../app/src-tauri/src/dev_image.rs"]
pub mod dev_image;
#[path = "../../../app/src-tauri/src/engine/mod.rs"]
pub mod engine;
#[path = "../../../app/src-tauri/src/example_run.rs"]
pub mod example_run;
#[path = "../../../app/src-tauri/src/local_results.rs"]
pub mod local_results;
#[path = "../../../app/src-tauri/src/local_vm.rs"]
pub mod local_vm;
#[path = "../../../app/src-tauri/src/overview.rs"]
pub mod overview;
#[path = "../../../app/src-tauri/src/pair_source.rs"]
pub mod pair_source;
#[path = "../../../app/src-tauri/src/runtime/mod.rs"]
pub mod runtime;
#[path = "../../../app/src-tauri/src/setup.rs"]
pub mod setup;
#[path = "../../../app/src-tauri/tests/example_run.rs"]
mod tests;
