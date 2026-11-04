// Exercise the actual desktop core without Tauri system libraries.
// Native desktop CI compiles these same sources in the application as well.
extern crate self as bridge_engine;
#[allow(dead_code)]
pub mod engine {
    pub mod types {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../app/src-tauri/src/engine/types.rs"
        ));
    }
}
#[path = "../../../app/src-tauri/src/runtime/mod.rs"]
pub mod runtime;
include!("../../../app/src-tauri/tests/runtime_core.rs");
