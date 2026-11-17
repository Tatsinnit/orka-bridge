use super::*;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};
#[test]
fn surviving_wrapper_has_no_fd_and_blocked_read_stops_without_io_lock_deadlock() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let client = slot.channel();
    let (configured, ready) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut b = [0];
        client.io(false, |mut socket| {
            configured.send(()).unwrap(); // Timeout is configured before revocation.
            socket.read(&mut b)
        })
    });
    ready.recv().unwrap();
    let mut second = slot.channel();
    assert_eq!(
        second.write(b"x").unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    slot.seal();
    // Shutdown must wake the read, but its thread need not be scheduled within
    // a cleanup budget. An active scoped reference may legitimately outlive it.
    if let Err(error) = slot.revoke(Instant::now()) {
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
    assert_eq!(reader.join().unwrap().unwrap(), 0);
    slot.revoke(Instant::now()).unwrap();
    assert_eq!(
        second.read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::NotConnected
    );
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}
#[test]
fn scoped_reference_timeout_never_reports_success() {
    let (socket, _) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let scoped = slot.acquire().unwrap();
    let socket = Arc::downgrade(scoped.socket.as_ref().unwrap());
    slot.seal();
    assert_eq!(
        slot.revoke(Instant::now()).unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(
        socket.upgrade().is_some(),
        "a scoped I/O reference still owns the descriptor"
    );
    drop(scoped);
    slot.revoke(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(
        socket.upgrade().is_none(),
        "the actual UnixStream was dropped and its descriptor closed"
    );
}
#[test]
fn drip_reads_do_not_renew_total_deadline() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let mut channel = slot.channel();
    channel.deadline = Some(Instant::now() + Duration::from_millis(60));
    let writer = std::thread::spawn(move || {
        for _ in 0..20 {
            if peer.write_all(b"x").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    });
    assert_eq!(
        channel.read_exact(&mut [0; 20]).unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    slot.seal();
    slot.revoke(Instant::now() + Duration::from_secs(1))
        .unwrap();
    writer.join().unwrap();
}
