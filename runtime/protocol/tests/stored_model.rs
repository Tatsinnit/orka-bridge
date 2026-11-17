extern crate self as bridge_engine;
#[path = "../../../app/src-tauri/src/catalog.rs"]
pub mod catalog;
#[cfg(debug_assertions)]
#[path = "../../../app/src-tauri/src/dev_image.rs"]
pub mod dev_image;
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
#[path = "../../../app/src-tauri/src/setup.rs"]
pub mod setup;
