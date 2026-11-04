extern crate self as bridge_engine;
pub mod engine {
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../app/src-tauri/src/engine/types.rs"
        ));
    }
}
#[path = "../../../app/src-tauri/src/runtime/client.rs"]
pub mod client;
#[path = "../../../app/src-tauri/src/runtime/snapshot.rs"]
pub mod snapshot;
pub mod runtime {
    pub use crate::{client, snapshot};
}
include!("../../../app/src-tauri/tests/runtime_transport.rs");
