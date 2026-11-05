use std::process::Command;
#[test]
fn help_and_invalid_modes_do_no_privileged_io() {
    let exe = env!("CARGO_BIN_EXE_bridge-guest");
    let help = Command::new(exe).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8(help.stdout).unwrap().contains("--early"));
    for args in [
        vec![],
        vec!["--serve", "/chosen-root"],
        vec!["--exec", "sh"],
    ] {
        let out = Command::new(exe).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
    }
}
#[test]
fn early_and_serve_fail_safely_before_privileged_io_for_nonroot() {
    // A root test runner must not execute either real privileged path.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    for (mode, stage) in [("--early", "early"), ("--serve", "serve")] {
        let out = Command::new(env!("CARGO_BIN_EXE_bridge-guest"))
            .arg(mode)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        assert!(out.stdout.is_empty());
        assert_eq!(String::from_utf8(out.stderr).unwrap().lines().collect::<Vec<_>>(),[
            format!("bridge-guest: stage={stage} phase=euid-precheck kind=PermissionDenied errno=none"),
            format!("bridge-guest: stage={stage} phase=run kind=Other errno=none"),
            "bridge-guest: incompatible state or unavailable guest operation".to_owned(),
        ]);
    }
}
#[test]
fn invalid_arguments_are_not_echoed_in_diagnostics() {
    let secret = "synthetic-credential-do-not-log";
    let out = Command::new(env!("CARGO_BIN_EXE_bridge-guest"))
        .arg(secret)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(!stderr.contains(secret));
    assert!(stderr.contains("stage=cli phase=arguments kind=InvalidInput errno=none"));
}
#[test]
fn legacy_errors_also_drop_under_stderr_backpressure() {
    use std::{
        io::Write,
        os::{fd::OwnedFd, unix::net::UnixStream},
        time::{Duration, Instant},
    };
    for (mode, code) in [("invalid", 2), ("--early", 1), ("--serve", 1)] {
        // Never run either real privileged path when the test runner is root.
        if code == 1 && unsafe { libc::geteuid() } == 0 {
            continue;
        }
        let (_reader, mut writer) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        loop {
            match writer.write(&[0; 4096]) {
                Ok(n) => assert!(n > 0),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        writer.set_nonblocking(false).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_bridge-guest"))
            .arg(mode)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(OwnedFd::from(writer)))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("legacy stderr output blocked process exit");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.code(), Some(code));
    }
}
#[test]
fn actual_socket_peer_is_checked() {
    let (a, _) = std::os::unix::net::UnixStream::pair().unwrap();
    let uid = unsafe { libc::geteuid() };
    assert_eq!(bridge_guest::peer::check(&a).is_ok(), uid == 0);
}
