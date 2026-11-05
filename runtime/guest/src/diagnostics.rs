//! Permanent, privacy-safe failure context for bridge-guest.
//!
//! Structured helpers accept only code-static stage/phase labels plus an
//! `io::ErrorKind` and a raw OS errno. Fixed legacy CLI lines use the same sink. It never formats an
//! `io::Error`'s `Display`/`Debug` output, its source, request data, command
//! output, descriptors or credentials, and it never changes control flow:
//! callers get back the exact `io::Result` they passed in, unchanged.
use std::io;

/// A fixed, caller-chosen location for a failure: which stage (early/serve/
/// cli) and which static-code-labeled phase within it.
#[derive(Clone, Copy)]
pub struct Context {
    pub stage: &'static str,
    pub phase: &'static str,
}
impl Context {
    pub const fn new(stage: &'static str, phase: &'static str) -> Self {
        Self { stage, phase }
    }
}

fn line(ctx: Context, kind: io::ErrorKind, errno: Option<i32>) -> String {
    reason_line(ctx, None, kind, errno)
}
fn reason_line(
    ctx: Context,
    reason: Option<&'static str>,
    kind: io::ErrorKind,
    errno: Option<i32>,
) -> String {
    let suffix = reason.map(|r| format!(".{r}")).unwrap_or_default();
    let errno = errno
        .map(|n| n.to_string())
        .unwrap_or_else(|| "none".into());
    format!(
        "bridge-guest: stage={} phase={}{suffix} kind={kind:?} errno={errno}",
        ctx.stage, ctx.phase
    )
}

const MAX_RECORD_BYTES: usize = 512;

// No stderr mutex, capacity wait, write_all, or retry. Regular files and
// non-TTY devices are deliberately unsupported: O_NONBLOCK does not bound
// their writes. Backpressure/open failures drop records; a partial socket or
// TTY write may leave a truncated line joined to the next surviving record.
fn emit(mut record: String) {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    if record.len() >= MAX_RECORD_BYTES {
        return;
    }
    record.push('\n');
    // Pin the inherited sink without changing its shared open-file flags.
    // SAFETY: fcntl accepts any descriptor number and reports a closed fd as EBADF.
    let fd = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
    if fd < 0 {
        return;
    }
    // SAFETY: successful F_DUPFD_CLOEXEC returned a new owned descriptor.
    let source = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut meta = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: source stays live and meta is writable for fstat.
    if unsafe { libc::fstat(source.as_raw_fd(), meta.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: fstat succeeded and initialized meta.
    let mode = unsafe { meta.assume_init() }.st_mode & libc::S_IFMT;
    if mode == libc::S_IFSOCK {
        // Per-call flags preserve inherited O_NONBLOCK and suppress SIGPIPE.
        // SAFETY: the owned socket and record bytes remain live through send.
        let _ = unsafe {
            libc::send(
                source.as_raw_fd(),
                record.as_ptr().cast(),
                record.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        return;
    }
    // SAFETY: isatty only inspects the live descriptor.
    if mode != libc::S_IFIFO
        && !(mode == libc::S_IFCHR && unsafe { libc::isatty(source.as_raw_fd()) } == 1)
    {
        return;
    }
    // A dup shares O_NONBLOCK with its parent; reopening the pinned proc fd
    // creates a separate open-file description. Root is required by the guest;
    // permission failures for other callers drop rather than use a blocking fd.
    let path = format!("/proc/self/fd/{}\0", source.as_raw_fd());
    // SAFETY: path is NUL-terminated; no create flag or controlling-TTY acquisition.
    let fd = unsafe {
        libc::open(
            path.as_ptr().cast(),
            libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY,
        )
    };
    if fd < 0 {
        return;
    }
    // SAFETY: successful open returned a new owned descriptor.
    let sink = unsafe { OwnedFd::from_raw_fd(fd) };
    // Pipe records fit Linux PIPE_BUF, so lack of space yields EAGAIN rather
    // than a partial/blocking write. Rust's unchanged SIGPIPE default is ignored.
    // SAFETY: the nonblocking sink and record bytes remain live through write.
    let _ = unsafe { libc::write(sink.as_raw_fd(), record.as_ptr().cast(), record.len()) };
}

/// Emits a code-static legacy CLI line through the same lossy sink.
pub fn fixed_line(text: &'static str) {
    emit(text.to_owned());
}

/// Logs only allowlisted metadata; sink backpressure never waits for space.
pub fn log(ctx: Context, kind: io::ErrorKind, errno: Option<i32>) {
    emit(line(ctx, kind, errno));
}

/// Keeps a shared I/O helper's fixed reason attached to its caller's phase.
pub fn log_reason(ctx: Context, reason: &'static str, kind: io::ErrorKind, errno: Option<i32>) {
    emit(reason_line(ctx, Some(reason), kind, errno));
}
pub fn note_reason(ctx: Context, reason: &'static str, err: &io::Error) {
    log_reason(ctx, reason, err.kind(), err.raw_os_error());
}

/// Logs the kind/errno of a real `io::Error` without formatting anything
/// else about it (no Display, no source).
pub fn note(ctx: Context, err: &io::Error) {
    log(ctx, err.kind(), err.raw_os_error());
}

/// Logs `result`'s error (if any) and returns it completely unchanged.
pub fn context<T>(ctx: Context, result: io::Result<T>) -> io::Result<T> {
    if let Err(e) = &result {
        note(ctx, e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_child(mut child: std::process::Child) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("diagnostics blocked under stderr backpressure");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    fn diagnostic_child(stderr: std::process::Stdio) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::backpressure_child",
                "--nocapture",
            ])
            .env("BRIDGE_TEST_BACKPRESSURE", "1")
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap()
    }
    #[test]
    fn backpressure_child() {
        if std::env::var_os("BRIDGE_TEST_BACKPRESSURE").is_none() {
            return;
        }
        if std::env::var_os("BRIDGE_TEST_RECORD_BOUNDARY").is_some() {
            emit("x".repeat(MAX_RECORD_BYTES));
            emit("y".repeat(MAX_RECORD_BYTES - 1));
            return;
        }
        if std::env::var_os("BRIDGE_TEST_SIGPIPE_DEFAULT").is_some() {
            // SAFETY: this isolated test child changes only its own signal disposition.
            assert_ne!(
                unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) },
                libc::SIG_ERR
            );
        }
        let lock = if std::env::var_os("BRIDGE_TEST_STDERR_LOCK").is_some() {
            let (held_tx, held_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                let stderr = io::stderr();
                let _lock = stderr.lock();
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            held_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .unwrap();
            Some((release_tx, thread))
        } else {
            None
        };
        let ctx = Context::new("serve", "hello-write");
        log(ctx, io::ErrorKind::Other, None);
        log_reason(ctx, "poll-hup", io::ErrorKind::Other, None);
        let error = context::<()>(ctx, Err(io::Error::from_raw_os_error(13))).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(13));
        if let Some((release, thread)) = lock {
            release.send(()).unwrap();
            thread.join().unwrap();
        }
    }
    #[test]
    fn held_rust_stderr_lock_does_not_block_diagnostics() {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::backpressure_child",
                "--nocapture",
            ])
            .env("BRIDGE_TEST_BACKPRESSURE", "1")
            .env("BRIDGE_TEST_STDERR_LOCK", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        assert!(wait_child(child).success());
    }
    #[test]
    fn record_size_is_bounded_and_oversize_is_dropped_whole() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::backpressure_child",
                "--nocapture",
            ])
            .env("BRIDGE_TEST_BACKPRESSURE", "1")
            .env("BRIDGE_TEST_RECORD_BOUNDARY", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            out.stderr,
            format!("{}\n", "y".repeat(MAX_RECORD_BYTES - 1)).as_bytes()
        );
    }
    #[test]
    fn closed_socket_does_not_raise_sigpipe_even_with_default_disposition() {
        use std::os::{fd::OwnedFd, unix::net::UnixStream};
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "diagnostics::tests::backpressure_child",
                "--nocapture",
            ])
            .env("BRIDGE_TEST_BACKPRESSURE", "1")
            .env("BRIDGE_TEST_SIGPIPE_DEFAULT", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(OwnedFd::from(writer)))
            .spawn()
            .unwrap();
        assert!(wait_child(child).success());
    }
    #[test]
    fn full_stderr_pipe_drops_records_without_changing_shared_flags() {
        use std::{
            fs::File,
            io::Write,
            os::fd::{AsRawFd, FromRawFd},
        };
        let mut fds = [-1; 2];
        // SAFETY: two writable entries receive distinct owned pipe descriptors.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: the successful pipe call transferred ownership of both descriptors.
        let _reader = unsafe { File::from_raw_fd(fds[0]) };
        let mut writer = unsafe { File::from_raw_fd(fds[1]) };
        let fd = writer.as_raw_fd();
        // SAFETY: fd is live; fcntl reads/changes only this test pipe's flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        loop {
            match writer.write(&[0; 4096]) {
                Ok(n) => assert!(n > 0),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) }, 0);
        assert!(wait_child(diagnostic_child(writer.try_clone().unwrap().into())).success());
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, flags);
    }
    #[test]
    fn full_stderr_socket_drops_records_without_changing_shared_flags() {
        use std::{
            io::Write,
            os::{
                fd::{AsRawFd, OwnedFd},
                unix::net::UnixStream,
            },
        };
        let (_reader, mut writer) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        loop {
            match writer.write(&[0; 4096]) {
                Ok(n) => assert!(n > 0),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        writer.set_nonblocking(false).unwrap();
        let fd = writer.as_raw_fd();
        // SAFETY: the owned socket descriptor is live for both observations.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        let stderr = OwnedFd::from(writer.try_clone().unwrap());
        assert!(wait_child(diagnostic_child(stderr.into())).success());
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, flags);
    }
    fn pty_pair() -> (std::fs::File, std::fs::File) {
        use std::{os::fd::FromRawFd, ptr};
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: output pointers are writable; optional name/termios/winsize are null.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                )
            },
            0
        );
        // SAFETY: openpty returned two distinct owned descriptors.
        unsafe {
            (
                std::fs::File::from_raw_fd(master),
                std::fs::File::from_raw_fd(slave),
            )
        }
    }
    #[test]
    fn full_stderr_tty_drops_records_without_changing_shared_flags() {
        use std::{io::Write, os::fd::AsRawFd};
        let (_master, mut slave) = pty_pair();
        let fd = slave.as_raw_fd();
        // SAFETY: the slave descriptor stays owned and live throughout the test.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        loop {
            match slave.write(&[0; 4096]) {
                Ok(n) => assert!(n > 0),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) }, 0);
        assert!(wait_child(diagnostic_child(slave.try_clone().unwrap().into())).success());
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFL) }, flags);
    }
    #[test]
    fn writable_tty_keeps_the_safe_records() {
        use std::{io::Read, os::fd::AsRawFd};
        let (mut master, slave) = pty_pair();
        assert!(wait_child(diagnostic_child(slave.into())).success());
        let mut bytes = [0; 1024];
        // SAFETY: master stays live, and setting this owned test descriptor nonblocking is local.
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(
            unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let mut captured = Vec::new();
        loop {
            match master.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => captured.extend_from_slice(&bytes[..n]),
                Err(e)
                    if e.raw_os_error() == Some(libc::EIO)
                        || e.kind() == io::ErrorKind::WouldBlock =>
                {
                    break
                }
                Err(e) => panic!("{e}"),
            }
        }
        let got = std::str::from_utf8(&captured).unwrap();
        assert!(got.contains("stage=serve phase=hello-write kind=Other errno=none"));
        assert!(got.contains("phase=hello-write.poll-hup"));
        assert_eq!(got.lines().count(), 3);
    }
    #[test]
    fn writable_socket_keeps_the_safe_records() {
        use std::{
            io::Read,
            os::{fd::OwnedFd, unix::net::UnixStream},
        };
        let (mut reader, writer) = UnixStream::pair().unwrap();
        assert!(wait_child(diagnostic_child(OwnedFd::from(writer).into())).success());
        let mut got = String::new();
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(
            got.lines().collect::<Vec<_>>(),
            [
                "bridge-guest: stage=serve phase=hello-write kind=Other errno=none",
                "bridge-guest: stage=serve phase=hello-write.poll-hup kind=Other errno=none",
                "bridge-guest: stage=serve phase=hello-write kind=PermissionDenied errno=13",
            ]
        );
    }
    #[test]
    fn regular_file_sink_is_dropped_instead_of_attempting_blocking_disk_io() {
        let file = tempfile::tempfile().unwrap();
        assert!(wait_child(diagnostic_child(file.try_clone().unwrap().into())).success());
        assert_eq!(file.metadata().unwrap().len(), 0);
    }
    #[test]
    fn permission_denied_reopen_drops_instead_of_falling_back_to_blocking_fd() {
        use std::{
            fs::File,
            io::Read,
            os::{fd::FromRawFd, unix::fs::PermissionsExt},
        };
        // Root bypasses inode permission checks, so this is a nonroot-only fixture.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let mut fds = [-1; 2];
        // SAFETY: two writable entries receive distinct owned pipe descriptors.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both returned descriptors are transferred to owned Files exactly once.
        let mut reader = unsafe { File::from_raw_fd(fds[0]) };
        let writer = unsafe { File::from_raw_fd(fds[1]) };
        writer
            .set_permissions(std::fs::Permissions::from_mode(0o000))
            .unwrap();
        assert!(wait_child(diagnostic_child(writer.into())).success());
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert!(got.is_empty());
    }
    #[test]
    fn closed_pipe_sink_cannot_terminate_the_process() {
        use std::{fs::File, os::fd::FromRawFd};
        let mut fds = [-1; 2];
        // SAFETY: two writable entries receive distinct owned pipe descriptors.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: both descriptors are owned exactly once from the successful pipe call.
        let reader = unsafe { File::from_raw_fd(fds[0]) };
        let writer = unsafe { File::from_raw_fd(fds[1]) };
        drop(reader);
        assert!(wait_child(diagnostic_child(writer.into())).success());
    }
    #[test]
    fn exact_shape_with_errno() {
        let ctx = Context::new("early", "port-open");
        let got = line(ctx, io::ErrorKind::PermissionDenied, Some(13));
        assert_eq!(
            got,
            "bridge-guest: stage=early phase=port-open kind=PermissionDenied errno=13"
        );
    }

    #[test]
    fn exact_shape_without_errno() {
        let ctx = Context::new("serve", "wait");
        let got = line(ctx, io::ErrorKind::TimedOut, None);
        assert_eq!(
            got,
            "bridge-guest: stage=serve phase=wait kind=TimedOut errno=none"
        );
    }

    #[test]
    fn stage_and_phase_are_restricted_to_the_caller_literals() {
        for stage in ["early", "serve", "cli"] {
            for phase in ["probe", "boot-read", "frame-validation"] {
                let got = line(Context::new(stage, phase), io::ErrorKind::Other, None);
                assert!(got.starts_with(&format!("bridge-guest: stage={stage} phase={phase} ")));
            }
        }
    }

    #[test]
    fn secret_custom_error_content_never_appears() {
        let secret = "sk-super-secret-token-value";
        let err = io::Error::other(secret);
        let ctx = Context::new("early", "initialize");
        let got = line(ctx, err.kind(), err.raw_os_error());
        assert!(!got.contains(secret));
        assert!(!got.contains(&err.to_string()));
        assert_eq!(
            got,
            "bridge-guest: stage=early phase=initialize kind=Other errno=none"
        );
    }

    #[test]
    fn context_preserves_the_original_error_by_kind_and_errno() {
        let original: io::Result<()> = Err(io::Error::from_raw_os_error(2));
        let ctx = Context::new("serve", "port-canonicalize");
        let round_tripped = context(ctx, original);
        let e = round_tripped.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.raw_os_error(), Some(2));
    }

    #[test]
    fn context_preserves_a_custom_payload_by_downcast() {
        #[derive(Debug)]
        struct Marker(u32);
        impl std::fmt::Display for Marker {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "marker")
            }
        }
        impl std::error::Error for Marker {}
        let original: io::Result<()> = Err(io::Error::other(Marker(7)));
        let got = context(Context::new("cli", "invalid-input"), original).unwrap_err();
        let marker = got.into_inner().unwrap().downcast::<Marker>().unwrap();
        assert_eq!(marker.0, 7);
    }

    #[test]
    fn context_passes_ok_through_untouched() {
        let original: io::Result<u32> = Ok(42);
        assert_eq!(
            context(Context::new("early", "probe"), original).unwrap(),
            42
        );
    }
}
