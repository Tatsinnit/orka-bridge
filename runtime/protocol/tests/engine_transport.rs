extern crate self as bridge_engine;
#[path = "../../../app/src-tauri/src/runtime/client.rs"]
pub mod client;
#[path = "../../../app/src-tauri/src/engine/mod.rs"]
pub mod engine;
#[path = "../../../app/src-tauri/src/runtime/snapshot.rs"]
pub mod snapshot;
pub mod runtime {
    pub use crate::{client, snapshot};
}
include!("../../../app/src-tauri/tests/engine.rs");
