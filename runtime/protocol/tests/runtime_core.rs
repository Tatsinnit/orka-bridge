// Exercise the actual desktop core without Tauri system libraries.
#[cfg(debug_assertions)]
#[path = "../../../app/src-tauri/src/dev_image.rs"]
pub mod dev_image;

// Native desktop CI compiles these same sources in the application as well.
extern crate self as bridge_engine;
#[path = "../../../app/src-tauri/src/engine/mod.rs"]
pub mod engine;
#[path = "../../../app/src-tauri/src/runtime/mod.rs"]
pub mod runtime;
// The session has no SDK calls: exercise it with real Unix helper processes on
// Linux too, without making it part of every integration target's runtime stub.
#[cfg(all(unix, not(target_os = "macos")))]
#[path = "../../../app/src-tauri/src/runtime/apple_session.rs"]
mod apple_session;
include!("../../../app/src-tauri/tests/runtime_core.rs");
