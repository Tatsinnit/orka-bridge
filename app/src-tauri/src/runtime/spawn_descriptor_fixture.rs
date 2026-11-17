use crate::runtime::spawn_guard::{run_blocking, tests::HeldGate};

fn probe_binary() -> std::path::PathBuf {
    use std::{process::Command, sync::OnceLock};
    static PROBE: OnceLock<tempfile::TempDir> = OnceLock::new();
    PROBE
        .get_or_init(|| {
            let d = tempfile::tempdir().unwrap();
            let source = d.path().join("probe.rs");
            std::fs::write(
                &source,
                // The probe is compiled without Cargo dependencies. Supply native
                // constants from libc rather than hard-coding Linux/Darwin values.
                format!(
                    "const PROBE_RECV_FLAGS: i32 = {};\n{}",
                    libc::MSG_PEEK | libc::MSG_DONTWAIT,
                    include_str!("../../tests/support/fd_probe_fixture.rs")
                ),
            )
            .unwrap();
            let mut compiler = std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        run_blocking(|| {
                            Command::new("rustc")
                                .arg("--edition=2021")
                                .arg(source)
                                .arg("-o")
                                .arg(d.path().join("probe"))
                                .spawn()
                        })
                    })
                    .join()
                    .unwrap()
            })
            .unwrap();
            assert!(compiler.wait().unwrap().success());
            d
        })
        .path()
        .join("probe")
}

pub(crate) fn descriptor_window() -> (HeldGate, tempfile::TempDir, std::path::PathBuf) {
    use std::{io::Write, os::fd::AsRawFd, os::unix::net::UnixStream};
    let binary = probe_binary();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_owned();
    let (ready, entered) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        run_blocking(|| {
            let (mut a, b) = UnixStream::pair().unwrap();
            let fd = b.as_raw_fd();
            // Deterministically recreate Darwin's non-atomic CLOEXEC setup window,
            // even on Linux where socketpair normally sets it atomically.
            assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
            let marker = root.file_name().unwrap().to_str().unwrap();
            a.write_all(marker.as_bytes()).unwrap();
            let path = root.join(format!("probe-{fd}-{marker}"));
            std::fs::hard_link(binary, &path).unwrap();
            ready.send(path).unwrap();
            let _ = wait.recv();
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
            drop((a, b));
        })
    });
    let executable = entered.recv().unwrap();
    (
        HeldGate {
            release: Some(release),
            thread: Some(thread),
        },
        directory,
        executable,
    )
}

#[test]
fn probe_distinguishes_private_socket_from_reused_descriptor_number() {
    use std::{
        fs::File,
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::Command,
    };
    let binary = probe_binary();
    run_blocking(|| {
        let directory = tempfile::tempdir().unwrap();
        let (mut peer, mut private) = UnixStream::pair().unwrap();
        let unrelated = File::open("/dev/null").unwrap();
        let (_idle_peer, idle) = UnixStream::pair().unwrap();
        let (mut other_peer, mut other) = UnixStream::pair().unwrap();
        let marker = directory.path().file_name().unwrap().to_str().unwrap();
        // The positive case deliberately exercises dup2(fd, fd).
        let target = private.as_raw_fd();
        let path = directory.path().join(format!("probe-{target}-{marker}"));
        peer.write_all(marker.as_bytes()).unwrap();
        other_peer.write_all(&vec![b'x'; marker.len()]).unwrap();
        std::fs::hard_link(binary, &path).unwrap();
        for (source, expected) in [
            (private.as_raw_fd(), "INHERITED_PRIVATE_FD"),
            (unrelated.as_raw_fd(), "FIXTURE_ONLY_clean"),
            (idle.as_raw_fd(), "FIXTURE_ONLY_clean"),
            (other.as_raw_fd(), "FIXTURE_ONLY_clean"),
        ] {
            let flags = unsafe { libc::fcntl(source, libc::F_GETFL) };
            let mut command = Command::new(&path);
            command.arg("version");
            // SAFETY: only async-signal-safe descriptor operations in the child.
            // Sources stay owned through spawn; target is test-controlled.
            unsafe {
                command.pre_exec(move || {
                    if libc::dup2(source, target) == -1
                        || libc::fcntl(target, libc::F_SETFD, 0) == -1
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "descriptor probe exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                unsafe { libc::fcntl(source, libc::F_GETFL) },
                flags,
                "probe changed shared descriptor flags"
            );
            assert_eq!(
                output.stdout,
                expected.as_bytes(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // Peeking must leave both matching and unrelated queued bytes intact.
        let mut bytes = vec![0; marker.len()];
        private.set_nonblocking(true).unwrap();
        other.set_nonblocking(true).unwrap();
        private.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, marker.as_bytes());
        other.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, vec![b'x'; marker.len()]);
    });
}
