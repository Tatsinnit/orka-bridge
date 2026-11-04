use tauri::utils::config::{Updater, WebviewUrl};

#[test]
fn native_window_uses_local_assets_and_fits_a_laptop() {
    let config: tauri::Config = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    assert_eq!(config.identifier, "ai.orka.bridge");
    assert_eq!(config.app.windows.len(), 1);
    let window = &config.app.windows[0];
    assert_eq!(window.label, "main");
    assert_eq!(window.title, "Bridge");
    assert!(matches!(&window.url, WebviewUrl::App(path) if path.to_str() == Some("index.html")));
    assert!(window.decorations && window.resizable && !window.fullscreen);
    assert_eq!(window.min_width, Some(960.0));
    assert_eq!(window.min_height, Some(640.0));
    assert!(window.width >= 960.0 && window.height >= 640.0);
    assert!(!config.app.with_global_tauri);
    assert!(config.app.security.capabilities.is_empty());
    // An empty configured list also enables capabilities discovered on disk.
    assert!(!std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("capabilities")
        .exists());
    assert_eq!(config.app.security.csp.as_ref().unwrap().to_string(),
        "default-src 'self'; connect-src ipc: http://ipc.localhost; style-src 'self'; font-src 'self'; object-src 'none'; frame-src 'none'; form-action 'none'");
    assert!(matches!(
        config.bundle.create_updater_artifacts,
        Updater::Bool(false)
    ));
}

#[test]
fn release_context_embeds_fonts_and_all_license_notices() {
    let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
    assert_eq!(context.config().identifier, "ai.orka.bridge");
    let assets = context.assets();
    for name in [
        "index.html",
        "licenses/shell-MIT.txt",
        "licenses/Orka-MIT.txt",
        "licenses/Inter-OFL.txt",
        "licenses/JetBrains-Mono-OFL.txt",
    ] {
        assert!(!assets
            .get(&name.into())
            .unwrap_or_else(|| panic!("missing asset: {name}"))
            .is_empty());
    }
    let names: Vec<_> = assets.iter().map(|(name, _)| name.into_owned()).collect();
    for font in ["inter-latin-", "jetbrains-mono-latin-"] {
        assert!(names
            .iter()
            .any(|name| name.contains(font) && name.ends_with(".woff2")));
    }
}
