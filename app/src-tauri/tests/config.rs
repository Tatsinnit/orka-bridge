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
    assert_eq!(window.min_width, Some(540.0));
    assert_eq!(window.min_height, Some(640.0));
    assert!(window.width >= 960.0 && window.height >= 640.0);
    assert!(!config.app.with_global_tauri);
    assert_eq!(
        serde_json::to_value(&config.app.security.capabilities).unwrap(),
        serde_json::json!([{
            "identifier": "engine",
            "description": "Typed engine and setup access from the main window only.",
            "windows": ["main"],
            "permissions": ["engine", "setup"],
            "local": true
        }])
    );
    // No implicit capability files may widen these finite command grants.
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
        "licenses/Tauri-API-MIT.txt",
        "licenses/Inter-OFL.txt",
        "licenses/JetBrains-Mono-OFL.txt",
        "licenses/git2-MIT.txt",
        "licenses/libgit2-COPYING.txt",
        "licenses/libz-sys-MIT.txt",
        "licenses/zlib-LICENSE.txt",
        "licenses/sha1-MIT.txt",
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

#[test]
fn agent_commands_are_granted_only_to_local_main_window() {
    use tauri::ipc::Origin;
    let mut context: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let authority = context.runtime_authority_mut();
    for command in [
        "workspace_catalog",
        "overview_files",
        "overview_agents",
        "overview_places",
        "cluster_places",
        "cluster_contexts",
        "cluster_discover",
        "cluster_use",
        "cluster_reload",
        "send_preflight",
        "send_start",
        "send_status",
        "send_history",
        "send_stop",
        "agent_run",
        "agent_run_status",
        "agent_chat_status",
        "agent_chat_send",
        "agent_chat_reconnect",
        "agent_chat_stop",
        "agent_chat_new",
        "agents_list",
        "agent_selected",
        "agent_open",
        "agent_save",
        "agent_reveal",
        "setup_cancel_local",
        "setup_change_local",
        "setup_open_help",
    ] {
        assert!(authority
            .resolve_access(command, "main", "main", &Origin::Local)
            .is_some());
        assert!(authority
            .resolve_access(command, "other", "other", &Origin::Local)
            .is_none());
        assert!(authority
            .resolve_access(
                command,
                "main",
                "main",
                &Origin::Remote {
                    url: "https://example.com".parse().unwrap()
                }
            )
            .is_none());
    }
    for command in ["agent_chat", "agent_chat_request", "agent_chat_delete"] {
        assert!(authority
            .resolve_access(command, "main", "main", &Origin::Local)
            .is_none());
    }
}
