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
            "workspace_upgrade_status",
            "workspace_upgrade_replace",
            "workspace_upgrade_retry",
            "workspace_upgrade_restore",
            "workspace_upgrade_transfer",
            "workspace_upgrade_remove",
            "workspace_upgrade_cancel",
            "workspace_catalog",
            "setup_select_local",
            "setup_cancel_local",
            "quit_bridge",
            "setup_change_local",
            "setup_reset_model",
            "setup_example",
            "agent_run",
            "agent_run_status",
            "agent_run_cancel",
            "agent_run_check_cancellation",
            "agent_chat_status",
            "agent_chat_send",
            "agent_chat_reconnect",
            "agent_chat_stop",
            "agent_chat_new",
            "agents_list",
            "overview_files",
            "overview_agents",
            "agent_selected",
            "agent_create",
            "agent_select",
            "agent_creation_root",
            "agent_open",
            "agent_save",
            "agent_reveal",
            "cluster_places",
            "cluster_reconnect",
            "cluster_select",
            "cluster_forget",
            "cluster_contexts",
            "cluster_reload",
            "cluster_discover",
            "cluster_service_accounts",
            "cluster_service_account_preview",
            "cluster_use",
            "send_preflight",
            "send_start",
            "send_status",
            "send_history",
            "send_history_details",
            "send_history_remove",
            "send_history_check",
            "send_history_cancel",
            "send_agent_focus",
            "send_agent_check",
            "send_cancel",
            "send_stop",
            "setup_requirements",
            "setup_preflight",
            "setup_open_help",
        ]));
    tauri_build::try_build(attributes).expect("failed to build Bridge");
}
