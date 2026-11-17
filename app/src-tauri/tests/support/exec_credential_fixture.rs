use serde_json::{json, Value};
use std::{path::PathBuf, sync::OnceLock};

pub fn plugin() -> &'static PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let exe = dir.join(format!(
            "credential-fixture{}",
            std::env::consts::EXE_SUFFIX
        ));
        let source = dir.join("plugin.rs");
        std::fs::write(&source, include_str!("credential_plugin.rs")).unwrap();
        assert!(std::process::Command::new("rustc")
            .args(["--edition=2021", "-o"])
            .arg(&exe)
            .arg(&source)
            .status()
            .unwrap()
            .success());
        exe
    })
}
pub fn response(version: &str) -> Value {
    json!({"kind":"ExecCredential","apiVersion":version,"status":{"token":"fixture-bearer","expirationTimestamp":"2099-01-01T00:00:00Z"}})
}
pub fn exec(version: &str, value: Value) -> Value {
    json!({"exec":{"apiVersion":version,"command":plugin(),"interactiveMode":"Never","env":[{"name":"FIXTURE_RESPONSE","value":value.to_string()}]}})
}
