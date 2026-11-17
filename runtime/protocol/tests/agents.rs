// Exercise native registration without Tauri system libraries.
extern crate self as bridge_engine;
#[path = "../../../app/src-tauri/src/agent_folder.rs"]
pub mod agent_folder;
#[path = "../../../app/src-tauri/src/agent_reveal.rs"]
pub mod agent_reveal;
#[path = "../../../app/src-tauri/src/agents.rs"]
pub mod agents;
#[path = "../../../app/src-tauri/src/runtime/image_cache.rs"]
pub mod image_cache;
#[path = "../../../app/src-tauri/src/runtime/image_types.rs"]
pub mod image_types;
#[path = "../../../app/src-tauri/src/runtime/snapshot.rs"]
pub mod snapshot;
#[path = "../../../app/src-tauri/src/runtime/spawn_guard.rs"]
pub mod spawn_guard;
pub mod runtime {
    pub use crate::{image_cache, image_types, snapshot, spawn_guard};
}
include!("../../../app/src-tauri/tests/agents.rs");
