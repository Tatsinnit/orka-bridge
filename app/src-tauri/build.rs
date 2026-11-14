fn main() {
    println!("cargo:rerun-if-changed=native/macos");
    println!("cargo:rerun-if-changed=../../scripts/build-vm-helper-macos.sh");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Every Mac shell includes this inactive arm64 resource, including Intel
        // shells. Tauri copies it during try_build, so generate it first.
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory missing"),
        );
        let status = std::process::Command::new("bash")
            .arg(manifest.join("../../scripts/build-vm-helper-macos.sh"))
            .status()
            .expect("Apple helper build requires bash and the Xcode Swift compiler");
        assert!(status.success(), "failed to build signed Apple helper");
    }
    let attributes =
        tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(&[
            "engine_call",
            "setup_status",
            "workspace_catalog",
            "setup_select_local",
            "setup_cancel_local",
            "setup_change_local",
            "setup_example",
            "agent_run",
            "agent_run_status",
            "agent_chat_status",
            "agent_chat_send",
            "agent_chat_reconnect",
            "agent_chat_stop",
            "agent_chat_new",
            "agents_list",
            "overview_files",
            "overview_agents",
            "agent_selected",
            "agent_open",
            "agent_save",
            "agent_reveal",
            "cluster_places",
            "cluster_contexts",
            "cluster_reload",
            "cluster_discover",
            "cluster_use",
            "send_preflight",
            "send_start",
            "send_status",
            "send_stop",
            "setup_requirements",
            "setup_preflight",
            "setup_open_help",
        ]));
    tauri_build::try_build(attributes).expect("failed to build Bridge");
}
