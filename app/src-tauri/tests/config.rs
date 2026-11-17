use tauri::utils::config::{Updater, WebviewUrl};

include!("../app_commands.rs");

macro_rules! command_names {
    ($($command:ident),* $(,)?) => {
        &[$(stringify!($command)),*]
    };
}

const APP_COMMANDS: &[&str] = with_app_commands!(command_names);

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
        "licenses/tauri-plugin-dialog-MIT.txt",
        "licenses/tauri-plugin-fs-MIT.txt",
        "licenses/rfd-MIT.txt",
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

fn allowed_commands(permission: &str, identifier: &str) -> Vec<String> {
    let file: tauri::utils::acl::manifest::PermissionFile =
        toml::from_str(permission).expect("permission must be valid TOML");
    let permission = file
        .permission
        .into_iter()
        .find(|permission| permission.identifier == identifier)
        .unwrap_or_else(|| panic!("permission {identifier} is missing"));
    assert!(
        permission.commands.deny.is_empty(),
        "app permission must not mix allowed and denied commands"
    );
    assert!(
        !permission.commands.allow.is_empty(),
        "permission has no allowed commands"
    );
    permission.commands.allow
}

#[test]
fn app_commands_are_granted_only_to_local_main_window() {
    use tauri::ipc::Origin;
    let mut context: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let authority = context.runtime_authority_mut();
    let mut commands = allowed_commands(include_str!("../permissions/engine.toml"), "engine");
    commands.extend(allowed_commands(
        include_str!("../permissions/setup.toml"),
        "setup",
    ));
    let command_count = commands.len();
    commands.sort_unstable();
    commands.dedup();
    assert_eq!(commands.len(), command_count, "command grant is duplicated");

    let mut app_commands = APP_COMMANDS.to_vec();
    let app_command_count = app_commands.len();
    app_commands.sort_unstable();
    app_commands.dedup();
    assert_eq!(
        app_commands.len(),
        app_command_count,
        "app command is duplicated"
    );
    assert_eq!(
        commands, app_commands,
        "command grants must match app commands"
    );

    for command in app_commands {
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
    for command in [
        "agent_chat",
        "agent_chat_request",
        "agent_chat_delete",
        "plugin:dialog|open",
        "plugin:dialog|save",
        "plugin:fs|read_file",
        "plugin:fs|write_file",
    ] {
        assert!(authority
            .resolve_access(command, "main", "main", &Origin::Local)
            .is_none());
    }
}
