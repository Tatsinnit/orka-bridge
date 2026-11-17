use std::os::{fd::FromRawFd, unix::fs::MetadataExt};

// PROBE_RECV_FLAGS is supplied by the fixture compiler using native libc values.
unsafe extern "C" {
    fn fcntl(fd: i32, command: i32, ...) -> i32;
    fn recv(fd: i32, buffer: *mut std::ffi::c_void, length: usize, flags: i32) -> isize;
}
fn main() {
    let a: Vec<_> = std::env::args().skip(1).collect();
    let account = a == ["config", "get", "user", "--host", "github.com"];
    assert!(account || a == ["version"] || a == ["auth", "token", "--hostname", "github.com"]);
    let path = std::env::current_exe().unwrap();
    let name = path.file_name().unwrap().to_str().unwrap();
    let (fd, marker) = name
        .strip_prefix("probe-")
        .unwrap()
        .split_once('-')
        .unwrap();
    let fd: i32 = fd.parse().unwrap();
    let mut inherited = false;
    if unsafe { fcntl(fd, 1) } != -1 {
        // SAFETY: F_GETFD above confirmed this descriptor is open. This
        // single-threaded probe takes ownership and never uses it elsewhere.
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let actual = file.metadata().unwrap();
        let mut bytes = vec![0; marker.len()];
        // SAFETY: the buffer is writable for its length. MSG_PEEK | MSG_DONTWAIT
        // neither consumes data nor changes shared flags on a reused socket.
        let received =
            unsafe { recv(fd, bytes.as_mut_ptr().cast(), bytes.len(), PROBE_RECV_FLAGS) };
        inherited = received == bytes.len() as isize && bytes == marker.as_bytes();
        eprintln!("fd={fd} expected-marker={marker} actual={}:{} received={received} marker-match={inherited}", actual.dev(), actual.ino());
    }
    if inherited {
        print!("INHERITED_PRIVATE_FD");
    } else if account {
        print!("fixture-login");
    } else {
        print!("FIXTURE_ONLY_clean");
    }
}
