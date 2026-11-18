use std::{fs::File, io::{Read, Write}, os::fd::FromRawFd, os::unix::net::UnixStream, time::Duration};
unsafe extern "C" { fn fcntl(fd: i32, command: i32, ...) -> i32; fn signal(sig: i32, handler: usize) -> usize; }
fn main() {
    let a: Vec<_> = std::env::args().collect();
    assert_eq!(a.len(), 11);
    assert_eq!((&a[3][..], &a[5][..], &a[7][..], &a[9][..]), ("--config-fd", "--supervisor-fd", "--guest-fd", "--ownership-fd"));
    let config = unsafe { File::from_raw_fd(a[4].parse().unwrap()) };
    assert!(config.metadata().unwrap().is_file());
    let fds: Vec<i32> = [6,8,10].map(|i| a[i].parse().unwrap()).into();
    assert!(std::env::vars_os().next().is_none(), "ambient environment reached helper");
    for &fd in &fds { assert_eq!(unsafe { fcntl(fd, 1) } & 1, 0); }
    let mut control = unsafe { UnixStream::from_raw_fd(fds[0]) };
    let mut guest = unsafe { UnixStream::from_raw_fd(fds[1]) };
    let _ownership = unsafe { File::from_raw_fd(fds[2]) };
    std::fs::write(&a[2], std::process::id().to_string()).unwrap();
    eprintln!("PRIVATE_PAYLOAD_NOT_A_DIAGNOSTIC");
    if a[1] == "unobserved" || a[1] == "hang-stop" { unsafe { signal(15, 1); } }
    if a[1] == "bad-ready" || a[1] == "unobserved" { control.write_all(b"BAD!!").unwrap(); }
    else { control.write_all(b"BRVM\x01").unwrap(); }
    guest.write_all(b"guest-only\n").unwrap();
    if a[1] == "unobserved" { std::thread::sleep(Duration::from_secs(60)); return; }
    if a[1] == "guest-eof" { let mut byte = [0]; assert_eq!(guest.read(&mut byte).unwrap(), 0); }
    let mut expected = 1u64;
    loop {
        let mut frame = [0; 9];
        if control.read_exact(&mut frame).is_err() { return; }
        assert!(matches!(frame[0], b'G' | b'P' | b'R' | b'S'));
        assert_eq!(u64::from_le_bytes(frame[1..].try_into().unwrap()), expected);
        expected += 1;
        let operation = frame[0];
        if operation == b'S' && a[1] == "lost-stop-receipt" { return; }
        frame[0] = if a[1] == "error" { b'E' }
            else if a[1] == "unexpected-natural" || (operation == b'S' && a[1].starts_with("natural-")) { b'N' }
            else { b'K' };
        if a[1] == "stale" || (operation == b'S' && a[1] == "natural-stale") { frame[1..].fill(0); }
        if a[1] == "fragment" {
            for b in frame { if control.write_all(&[b]).is_err() { return; } std::thread::sleep(Duration::from_millis(15)); }
        } else if control.write_all(&frame).is_err() { return; }
        if operation == b'G' && a[1] == "guest-stop" { return; }
        if operation == b'G' && a[1] == "guest-error" { std::process::exit(7); }
        if operation == b'S' {
            if a[1] == "hang-stop" || a[1] == "natural-hang" { std::thread::sleep(Duration::from_secs(60)); }
            if a[1] == "exit-bad" || a[1] == "natural-exit-bad" { std::process::exit(7); }
            return;
        }
    }
}
