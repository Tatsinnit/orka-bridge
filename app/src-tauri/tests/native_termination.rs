// Cargo's harness-free target gives AppKit the real main thread. Each scenario
// terminates only its own child process, never the host session or a real VM.
#[cfg(target_os = "macos")]
#[path = "../src/lifecycle/macos.rs"]
mod macos;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    const CASES: [&str; 6] = [
        "clean",
        "failed",
        "existing",
        "deadline",
        "completed",
        "missing-delegate",
    ];
    if std::env::args()
        .nth(1)
        .is_some_and(|arg| CASES.contains(&arg.as_str()))
    {
        macos::native_tests::run(tauri::generate_context!());
        return;
    }
    for case in CASES {
        let start = Instant::now();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg(case)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() >= Duration::from_secs(40) {
                child.kill().unwrap();
                panic!("{case} timed out: {:?}", child.wait_with_output().unwrap());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        let log = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{case}: status={}\n{log}", output.status);
        assert_eq!(output.status.code(), Some(0), "{case}: {log}"); // AppKit status is NOT a clean-stop assertion.
        if case == "missing-delegate" {
            assert!(log.contains("native-test:install-rejected"), "{log}");
            continue;
        }
        let count = |marker: &str| log.matches(marker).count();
        assert_eq!(count("native-test:worker"), 1, "{case}: {log}");
        assert_eq!(count("native-test:Exit/LoopDestroyed"), 1, "{case}: {log}");
        assert_eq!(count("native-test:reply="), 1, "{case}: {log}");
        let reply = if case == "completed" { 1 } else { 2 };
        assert!(
            log.contains(&format!("native-test:reply={reply}")),
            "{case}: {log}"
        );
        if case != "completed" {
            assert!(log.contains("native-test:modal-poll"), "{case}: {log}");
        }
        if case == "deadline" {
            assert!(
                start.elapsed() >= Duration::from_secs(29),
                "deadline fired prematurely"
            );
            assert!(log.contains("unconfirmed"), "{log}");
            assert!(!log.contains("native-test:finished="), "{log}");
        } else {
            assert!(
                log.contains(&format!("native-test:finished={}", case != "failed")),
                "{case}: {log}"
            );
            if case == "failed" {
                assert!(log.contains("unclean"), "{log}");
            }
        }
    }
    println!("Native termination passed: real terminate:, modal reply, one final exit; no host logout or VM.");
}
