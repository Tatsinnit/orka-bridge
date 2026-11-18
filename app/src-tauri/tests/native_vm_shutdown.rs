//! Opt-in physical-Mac check. CI builds this driver but cannot boot nested VZ.
//! Uses the production workspace factory/owner; no model sign-in or credentials.
//!
//! This is a shutdown driver, not a guest inspector. Set BRIDGE_SHUTDOWN_TEST_ROOT
//! to a fresh disposable workspace root (a verified image cache may be prewarmed)
//! and BRIDGE_SHUTDOWN_TEST_RESOURCES to the helper's
//! resources directory, then run this ignored test with --nocapture. An external
//! observer must supply test-only console input/output and guest shell access;
//! the production helper deliberately exposes neither. Never use a user's disks.
//! Boot 1 permits console preparation. At boot2-ready, write a 64 MiB sentinel in
//! /var/lib/bridge, record sha256sum and full dmesg, then create quit2 without
//! syncing the sentinel. Continuously capture the console until the VM stops and
//! check it for kernel filesystem errors. At boot3-ready, compare its checksum and inspect full
//! dmesg again before quit3. Create quit1 after preparation. Any kernel filesystem
//! error or checksum mismatch fails the external check, regardless of this test's
//! result. The stopped markers prove only completed production shutdown calls.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a physical Mac, disposable workspace and external guest data/kernel-log inspection"]
async fn workspace_shutdown_reopens_the_same_disks() {
    use bridge_engine::local_vm::{configured, LocalVmStatus};
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };
    let root =
        PathBuf::from(std::env::var_os("BRIDGE_SHUTDOWN_TEST_ROOT").expect("disposable root"));
    let resources = PathBuf::from(
        std::env::var_os("BRIDGE_SHUTDOWN_TEST_RESOURCES").expect("helper resources"),
    );
    assert!(root.is_absolute() && resources.is_absolute());
    let data = root.join("data");
    let local = root.join("local");
    assert!(
        !data.exists() && !local.join("local-vm-v1").exists(),
        "use a fresh disposable workspace root; only an image cache may be reused"
    );
    for boot in 1..=3 {
        for marker in [
            format!("quit{boot}"),
            format!("boot{boot}-ready"),
            format!("boot{boot}-stopped"),
        ] {
            assert!(
                !root.join(marker).exists(),
                "stale observer marker; use a new run root"
            );
        }
    }
    std::fs::create_dir_all(&root).unwrap();
    // Boot 1 permits test-only console preparation. Boots 2 and 3 are the
    // write/quit/reopen comparison, using the same installed workspace identity.
    for boot in 1..=3 {
        let vm = configured(
            Some(resources.clone()),
            Some(data.clone()),
            Some(local.clone()),
        );
        vm.start();
        let until = Instant::now() + Duration::from_secs(300);
        loop {
            match vm.status() {
                LocalVmStatus::Ready => break,
                LocalVmStatus::Failed | LocalVmStatus::NotIntegrated => {
                    let stopped = vm.shutdown().await;
                    panic!("boot {boot} failed; shutdown={stopped:?}");
                }
                _ => {}
            }
            if Instant::now() >= until {
                let stopped = vm.shutdown().await;
                panic!("boot {boot} did not become ready; shutdown={stopped:?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Err(error) = std::fs::write(root.join(format!("boot{boot}-ready")), b"ready\n") {
            let stopped = vm.shutdown().await;
            panic!("boot {boot} ready marker failed: {error}; shutdown={stopped:?}");
        }
        // The external observer writes this only after recording the sentinel
        // checksum and full guest dmesg. Do not equate Ready with that evidence.
        let until = Instant::now() + Duration::from_secs(300);
        while !root.join(format!("quit{boot}")).exists() {
            if Instant::now() >= until {
                let stopped = vm.shutdown().await;
                panic!("boot {boot} guest inspection missing; shutdown={stopped:?}");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let started = Instant::now();
        let stopped = vm.shutdown().await;
        println!(
            "boot={boot} shutdown={stopped:?} elapsed={:?}",
            started.elapsed()
        );
        stopped.expect("forced or unconfirmed shutdown is not a clean qualification");
        vm.shutdown()
            .await
            .expect("second stop must remain clean and idempotent");
        drop(vm); // Return the journal before the next factory opens the same root.
        std::fs::write(root.join(format!("boot{boot}-stopped")), b"clean\n").unwrap();
    }
}
