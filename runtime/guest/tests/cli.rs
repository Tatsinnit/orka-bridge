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
fn actual_socket_peer_is_checked() {
    let (a, _) = std::os::unix::net::UnixStream::pair().unwrap();
    let uid = unsafe { libc::geteuid() };
    assert_eq!(bridge_guest::peer::check(&a).is_ok(), uid == 0);
}
