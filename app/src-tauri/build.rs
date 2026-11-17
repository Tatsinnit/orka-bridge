include!("app_commands.rs");

macro_rules! command_names {
    ($($command:ident),* $(,)?) => {
        &[$(stringify!($command)),*]
    };
}

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
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(with_app_commands!(command_names)));
    tauri_build::try_build(attributes).expect("failed to build Bridge");
}
