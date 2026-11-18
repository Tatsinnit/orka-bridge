use super::*;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};
pub(crate) fn hold_scope(slot: &GuestSlot) -> impl Drop {
    slot.acquire().unwrap()
}
pub(crate) fn short_close(slot: &GuestSlot) {
    slot.inner
        .state
        .lock()
        .unwrap()
        .close
        .as_mut()
        .unwrap()
        .grace = Duration::from_millis(60);
}
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
fn close_watch_bounds_a_read_that_started_before_shutdown() {
    let (socket, _peer) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let (closed, close) = tokio::sync::watch::channel(false);
    slot.bind_close(close, None);
    short_close(&slot);
    let mut channel = slot.channel();
    let (result, observed) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        result.send(channel.read(&mut [0])).unwrap();
    });
    let until = Instant::now() + Duration::from_secs(2);
    while !slot.inner.state.lock().unwrap().active {
        assert!(Instant::now() < until);
        std::thread::yield_now();
    }
    closed.send_replace(true);
    let stopped = observed.recv_timeout(Duration::from_secs(1));
    slot.revoke(Instant::now() + Duration::from_secs(1))
        .unwrap();
    reader.join().unwrap();
    assert_eq!(
        stopped
            .expect("pre-existing read ignored close")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(slot.shutdown_deadline().unwrap() < Instant::now());
}
#[test]
fn idle_close_uses_publication_origin_even_when_observed_late() {
    let (socket, _peer) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let origin = Instant::now() - Duration::from_secs(20);
    let (_closed, close) = tokio::sync::watch::channel(true);
    slot.bind_close(close, Some(Arc::new(Mutex::new(Some(origin)))));
    assert_eq!(
        slot.shutdown_deadline(),
        Some(origin + Duration::from_secs(18))
    );
    assert_eq!(
        slot.channel().read(&mut [0]).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    let earlier = origin + Duration::from_secs(1);
    slot.prepare_shutdown(earlier);
    assert_eq!(slot.shutdown_deadline(), Some(earlier));
}
#[test]
fn scoped_reference_timeout_never_reports_success() {
    let (socket, _) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let socket = Arc::downgrade(slot.inner.state.lock().unwrap().socket.as_ref().unwrap());
    let scoped = hold_scope(&slot);
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
    // The channel was already established before shutdown preparation.
    slot.prepare_shutdown(Instant::now() + Duration::from_millis(60));
    slot.prepare_shutdown(Instant::now() + Duration::from_secs(60)); // Cannot renew.
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
