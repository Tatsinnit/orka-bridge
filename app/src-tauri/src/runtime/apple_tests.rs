use super::*;
fn config() -> File { tempfile::tempfile().unwrap() }
use std::{io::Read, os::fd::AsRawFd, sync::OnceLock};

// Synthetic descriptors/signals only: this ignored Mac test does not qualify VZ.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires the compiled Swift supervisor I/O test executable"]
fn swift_supervisor_peer() {
    use std::os::unix::process::ExitStatusExt;

    let executable = std::env::var_os("BRIDGE_TEST_SWIFT_PEER")
        .expect("BRIDGE_TEST_SWIFT_PEER must name the compiled Swift test executable");
    let executable = std::path::PathBuf::from(executable).canonicalize().unwrap();
    assert!(executable.is_file(), "Swift peer executable is missing");
    let budgets = Budgets {
        read: Duration::from_secs(5),
        write: Duration::from_secs(1),
        exit: Duration::from_secs(2),
        cleanup: Duration::from_secs(2),
    };
    let start = || {
        let directory = tempfile::tempdir().unwrap();
        let file = File::create(directory.path().join("lock")).unwrap();
        file.try_lock().unwrap();
        let mut owner = AppleHelperOwner::new(Arc::new(file));
        owner.budgets = budgets;
        let mut command = Command::new(&executable);
        command.arg("--synthetic-peer");
        // The actual owner supplies exactly four FD arguments and clears env/stdio.
        let guest = owner
            .start(command, config())
            .expect("Swift peer did not send BRVM readiness");
        let mut marker = [0];
        read_until(&guest, &mut marker, Instant::now() + budgets.read).unwrap();
        assert_eq!(marker, [0xAA], "fake driver did not enter running state");
        guest.set_write_timeout(Some(budgets.write)).unwrap();
        (directory, owner, guest)
    };
    let disposed = |guest: &mut UnixStream| {
        let end = Instant::now() + budgets.read;
        // Sole reader: poll bounds both buffered data and EOF after peer closure,
        // without setting socket options on a disconnected Darwin endpoint.
        let mut byte = || loop {
            let mut descriptor = libc::pollfd {
                fd: guest.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout = remaining(end).unwrap().as_millis().max(1) as i32;
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
            if ready < 0 && std::io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            assert!(ready > 0, "guest disposal data/EOF did not become readable");
            let mut buffer = [0];
            return (guest.read(&mut buffer).unwrap(), buffer[0]);
        };
        assert_eq!(byte(), (1, 0xBB), "missing guest disposal marker");
        assert_eq!(byte().0, 0, "guest FD not closed after disposal");
    };
    let unclean = |status: ExitStatus| {
        assert_eq!(
            status.code(),
            Some(1),
            "isolated EOF/TERM must exit 1: {status:?}"
        );
    };

    // Happy P/R/S: stop itself requires an observed successful child exit.
    {
        let (_directory, mut owner, mut guest) = start();
        owner.pause().unwrap();
        owner.resume().unwrap();
        owner.stop().unwrap();
        disposed(&mut guest);
        assert!(owner.session.is_none());
    }
    // EOF alone: no cleanup or signal before the child's status is observed.
    {
        let (_directory, mut owner, mut guest) = start();
        let session = owner.session.as_mut().unwrap();
        drop(session.control.take());
        disposed(&mut guest);
        unclean(
            wait_until(&mut session.child, Instant::now() + budgets.exit)
                .unwrap()
                .expect("EOF-only child did not exit"),
        );
        drop(owner);
    }
    // TERM alone: readiness has returned, the control endpoint remains open,
    // and the owned Child is unreaped when its PID is signalled.
    {
        let (_directory, mut owner, mut guest) = start();
        let session = owner.session.as_mut().unwrap();
        assert!(session.control.is_some());
        assert!(session.child.try_wait().unwrap().is_none());
        assert_eq!(
            unsafe { libc::kill(session.child.id() as i32, libc::SIGTERM) },
            0
        );
        disposed(&mut guest);
        unclean(
            wait_until(&mut session.child, Instant::now() + budgets.exit)
                .unwrap()
                .expect("TERM-only child did not exit"),
        );
        assert!(
            session.control.is_some(),
            "TERM probe accidentally used EOF"
        );
        drop(owner);
    }
    // Combined-event status probe, not a recovered actual Drop status. Keep the
    // Child in the owner until status is captured; never signal a reaped PID.
    {
        let (_directory, mut owner, mut guest) = start();
        let session = owner.session.as_mut().unwrap();
        drop(session.control.take());
        let early = session.child.try_wait().unwrap();
        if early.is_none() {
            assert_eq!(
                unsafe { libc::kill(session.child.id() as i32, libc::SIGTERM) },
                0
            );
        }
        disposed(&mut guest);
        let status = early
            .or_else(|| wait_until(&mut session.child, Instant::now() + budgets.exit).unwrap())
            .expect("combined-event child did not exit");
        assert!(
            status.code() == Some(1) || status.signal() == Some(libc::SIGTERM),
            "combined events allow only exit 1 or TERM after disposal: {status:?}"
        );
        drop(owner);
    }
    // Actual Drop has no retained ExitStatus: prove disposal/guest closure and
    // reap separately, without claiming to recover its discarded status.
    {
        let (_directory, owner, mut guest) = start();
        let pid = owner.session.as_ref().unwrap().child.id() as i32;
        drop(owner);
        disposed(&mut guest);
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1,
            "actual owner Drop did not reap the Swift child"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}

fn fixture() -> &'static std::path::Path {
    static FIXTURE: OnceLock<tempfile::TempDir> = OnceLock::new();
    FIXTURE
        .get_or_init(|| {
            let d = tempfile::tempdir().unwrap();
            let source = d.path().join("helper.rs");
            std::fs::write(
                &source,
                include_str!("../../tests/support/apple_helper_fixture.rs"),
            )
            .unwrap();
            let mut compiler = super::super::spawn_guard::run_blocking(|| {
                Command::new("rustc")
                    .arg("--edition=2021")
                    .arg(source)
                    .arg("-o")
                    .arg(d.path().join("helper"))
                    .spawn()
            })
            .unwrap();
            assert!(compiler.wait().unwrap().success());
            d
        })
        .path()
}
fn setup(mode: &str) -> (tempfile::TempDir, AppleHelperOwner, Command) {
    let d = tempfile::tempdir().unwrap();
    let file = File::create(d.path().join("lock")).unwrap();
    file.try_lock().unwrap();
    let mut owner = AppleHelperOwner::new(Arc::new(file));
    owner.budgets = Budgets {
        read: Duration::from_secs(2),
        write: Duration::from_secs(1),
        exit: Duration::from_millis(40),
        cleanup: Duration::from_millis(40),
    };
    let mut cmd = Command::new(fixture().join("helper"));
    cmd.arg(mode)
        .arg(d.path().join("pid"))
        .env("GH_TOKEN", "synthetic-not-an-account");
    (d, owner, cmd)
}
fn assert_reaped(d: &tempfile::TempDir) {
    let pid: i32 = std::fs::read_to_string(d.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1,
        "owned child was not reaped"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn closed_helper_control_preserves_buffered_reply_and_eof() {
    let (control, mut helper) = UnixStream::pair().unwrap();
    helper.write_all(b"BRVM\x01").unwrap();
    drop(helper);
    let mut reply = [0; 5];
    read_until(&control, &mut reply, Instant::now() + Duration::from_secs(2)).unwrap();
    assert_eq!(&reply, b"BRVM\x01");
    assert_eq!(
        read_until(&control, &mut [0], Instant::now() + Duration::from_secs(2)),
        Err(HelperError::Io(io::ErrorKind::UnexpectedEof))
    );
}

#[test]
fn helper_owns_only_selected_channels_and_cannot_inherit_ambient_tokens() {
    let (d, mut owner, cmd) = setup("normal");
    let parent_flags = unsafe { libc::fcntl(owner.ownership.as_raw_fd(), libc::F_GETFD) };
    let mut guest = owner.start(cmd, config()).unwrap();
    let mut bytes = [0; 11];
    guest.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"guest-only\n");
    owner.pause().unwrap();
    owner.resume().unwrap();
    assert_eq!(
        unsafe { libc::fcntl(owner.ownership.as_raw_fd(), libc::F_GETFD) },
        parent_flags
    );
    owner.stop().unwrap();
    assert_reaped(&d);
}
#[test]
fn supervisor_eof_reaches_helper_without_any_termination_signal() {
    let (d, mut owner, cmd) = setup("normal");
    let _guest = owner.start(cmd, config()).unwrap();
    let session = owner.session.as_mut().unwrap();
    session.control = None;
    assert!(
        wait_until(&mut session.child, Instant::now() + Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .success()
    );
    owner.session = None;
    assert_reaped(&d);
}

#[test]
fn idle_pause_and_resume_do_not_poison_a_safe_owner() {
    let (d, mut owner, cmd) = setup("normal");
    let disconnected = HelperError::Io(std::io::ErrorKind::NotConnected);
    assert_eq!(owner.pause().unwrap_err(), disconnected);
    assert_eq!(owner.resume().unwrap_err(), disconnected);
    let _guest = owner.start(cmd, config()).unwrap();
    owner.stop().unwrap();
    assert_eq!(owner.pause().unwrap_err(), disconnected);
    assert_eq!(owner.resume().unwrap_err(), disconnected);
    let mut again = Command::new(fixture().join("helper"));
    again.arg("normal").arg(d.path().join("pid"));
    let _next_guest = owner.start(again, config()).unwrap();
    owner.stop().unwrap();
    assert_reaped(&d);
}

#[test]
fn guest_parent_endpoint_is_not_inherited_by_the_helper() {
    let (d, mut owner, cmd) = setup("guest-eof");
    let mut guest = owner.start(cmd, config()).unwrap();
    let mut bytes = [0; 11];
    guest.read_exact(&mut bytes).unwrap();
    drop(guest);
    // Fixture cannot acknowledge until it observes guest-peer EOF; unlike Drop,
    // no TERM/KILL can make a hidden inherited parent endpoint pass this test.
    owner.pause().unwrap();
    owner.stop().unwrap();
    assert_reaped(&d);
}

#[test]
fn busy_owner_never_starts_another_process() {
    let (d, mut owner, cmd) = setup("normal");
    let _guest = owner.start(cmd, config()).unwrap();
    assert_eq!(
        owner.start(Command::new("/must-not-execute"), config()).unwrap_err(),
        HelperError::Busy
    );
    owner.stop().unwrap();
    assert_reaped(&d);
}
#[test]
fn bad_startup_permanently_poisons_owner_before_retry() {
    let (d, mut owner, cmd) = setup("bad-ready");
    assert_eq!(owner.start(cmd, config()).unwrap_err(), HelperError::Protocol);
    assert_eq!(
        owner.start(Command::new("/must-not-execute"), config()).unwrap_err(),
        HelperError::Poisoned
    );
    assert_reaped(&d);
}
#[test]
fn mismatched_reply_and_helper_failure_poison_without_replay() {
    for mode in ["stale", "error"] {
        let (d, mut owner, cmd) = setup(mode);
        let _guest = owner.start(cmd, config()).unwrap();
        assert_eq!(owner.pause().unwrap_err(), HelperError::Protocol);
        assert_eq!(owner.resume().unwrap_err(), HelperError::Poisoned);
        assert_eq!(
            owner.start(Command::new("/must-not-execute"), config()).unwrap_err(),
            HelperError::Poisoned
        );
        assert_reaped(&d);
    }
}
#[test]
fn response_fragments_do_not_restart_the_absolute_deadline() {
    let (d, mut owner, cmd) = setup("fragment");
    let _guest = owner.start(cmd, config()).unwrap();
    owner.budgets.read = Duration::from_millis(35);
    assert_eq!(
        owner.pause().unwrap_err(),
        HelperError::Io(std::io::ErrorKind::TimedOut)
    );
    assert_eq!(owner.resume().unwrap_err(), HelperError::Poisoned);
    assert_reaped(&d);
}
#[test]
fn stop_requires_observed_successful_exit_not_only_an_ack() {
    for mode in ["exit-bad", "hang-stop"] {
        let (d, mut owner, cmd) = setup(mode);
        let _guest = owner.start(cmd, config()).unwrap();
        assert!(owner.stop().is_err());
        assert_eq!(
            owner.start(Command::new("/must-not-execute"), config()).unwrap_err(),
            HelperError::Poisoned
        );
        assert_reaped(&d);
    }
}
#[test]
fn dropping_live_owner_reaps_before_releasing_its_lock() {
    let (d, mut owner, cmd) = setup("normal");
    let _guest = owner.start(cmd, config()).unwrap();
    let probe = File::open(d.path().join("lock")).unwrap();
    assert!(probe.try_lock().is_err());
    drop(owner);
    assert_reaped(&d);
    // Another app spawn may transiently inherit even CLOEXEC files between fork
    // and exec. Check release after in-flight guarded spawns cross that boundary.
    super::super::spawn_guard::run_blocking(|| probe.try_lock().unwrap());
}
#[test]
fn unobserved_start_exit_is_distinct_and_drop_retains_lock_without_retry() {
    let (d, mut owner, cmd) = setup("unobserved");
    // No observation budget: still an actual child, not a mocked wait result.
    // The fixture sends a bad header only after installing TERM-ignore and its PID receipt.
    owner.budgets.cleanup = Duration::ZERO;
    assert_eq!(owner.start(cmd, config()).unwrap_err(), HelperError::ExitUnobserved);
    assert_eq!(
        owner.start(Command::new("/must-not-execute"), config()).unwrap_err(),
        HelperError::ExitUnobserved
    );
    drop(owner);
    let probe = File::open(d.path().join("lock")).unwrap();
    assert!(
        probe.try_lock().is_err(),
        "Drop released retained installation ownership"
    );
    // Reap the exact test-owned child which the zero-budget observer retained.
    // A real unobserved exit keeps its capability quarantined until app exit.
    let pid: i32 = std::fs::read_to_string(d.path().join("pid"))
        .unwrap()
        .parse()
        .unwrap();
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        let mut status = 0;
        assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
    }
    assert!(
        probe.try_lock().is_err(),
        "quarantined parent capability was not retained"
    );
}
