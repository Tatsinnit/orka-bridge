use super::*;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
#[ignore = "subprocess fixture: invoked by process_death_at_each_boundary"]
fn crash_child() {
    let mut scene = Scene::new(if cfg!(target_os = "macos") {
        ImageTarget::MacosArm64
    } else {
        ImageTarget::WindowsAmd64
    });
    scene.replace(None).unwrap();
    panic!("crash point was not reached");
}
#[test]
fn process_death_at_each_boundary_releases_ownership_without_adopting_unknown_state() {
    for point in [
        "acquired",
        "intent",
        "model-archived",
        "account-archived",
        "source-retained",
        "destination-created",
        "destination-recorded",
        "image-prepared",
    ] {
        let home = tempfile::tempdir().unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "workspace_upgrade::tests::process::crash_child",
            ])
            .env("BRIDGE_UPGRADE_CRASH_HOME", home.path())
            .env("BRIDGE_UPGRADE_CRASH_POINT", point)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let start = Instant::now();
        let ready = home.path().join("ready.json");
        while !ready.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "child exited before {point}"
            );
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "did not reach {point}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let receipt: serde_json::Value = serde_json::from_slice(&fs::read(ready).unwrap()).unwrap();
        let data = PathBuf::from(receipt["data"].as_str().unwrap());
        let local = PathBuf::from(receipt["local"].as_str().unwrap());
        let source: Uuid = serde_json::from_value(receipt["source"].clone()).unwrap();
        let owner = WorkspaceUpgrade::open(&data, &local).unwrap();
        if point == "acquired" {
            assert_eq!(owner.recover().unwrap(), UpgradeStatus::None);
            assert!(data.join("setup-v1/model-v1").exists());
            continue;
        }
        let intent = owner.read_intent().unwrap().unwrap();
        let retained = local.join(&intent.retained);
        let source_path = if retained.exists() {
            retained
        } else {
            local.join("local-vm-v1")
        };
        assert_eq!(
            installation(&source_path).unwrap().as_ref(),
            Some(&intent.source)
        );
        assert_eq!(read_record(&source_path).unwrap().installation_id, source);
        assert_eq!(
            fs::read(source_path.join("efi-stand-in")).unwrap(),
            b"firmware-state"
        );
        if point == "destination-created" {
            assert!(
                owner.recover().is_err(),
                "unrecorded canonical identity must be refused"
            );
        } else {
            assert_ne!(owner.recover().unwrap(), UpgradeStatus::None);
        }
        assert!(admit(&local.join("local-vm-v1")).is_err());
    }
}
