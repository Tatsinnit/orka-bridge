fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(&["engine_call"]));
    tauri_build::try_build(attributes).expect("failed to build Bridge");
}
