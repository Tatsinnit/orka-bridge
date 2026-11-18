// Private Apple helper supervision. Not VM registration or a serving-stage handoff.
use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command, ExitStatus, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) const SHUTDOWN_FALLBACK: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub enum HelperError {
    Io(io::ErrorKind),
    Protocol,
    Busy,
    Poisoned,
    ExitUnobserved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppleStopOutcome {
    GuestStopped,
    ForcedStop,
    HardStopped,
}
#[derive(Clone, Copy)]
struct Budgets {
    read: Duration,
    write: Duration,
    exit: Duration,
    cleanup: Duration,
}
struct Session {
    child: Child,
    control: Option<UnixStream>,
    next: u64,
    cleanup_attempted: bool,
    budgets: Budgets,
    stop_deadline: Option<Instant>,
}
impl Session {
    fn cap_end(&self, end: Instant) -> Instant {
        self.stop_deadline.map_or(end, |limit| limit.min(end))
    }
}
/// One persistent owner per native backend. Never reconstruct it from a cached
/// ownership Arc after failure: the same worker must retain its poison status.
/// Spawn/operations run on the native blocking worker, not a Tokio executor.
pub struct AppleHelperOwner {
    ownership: Arc<File>,
    budgets: Budgets,
    session: Option<Session>,
    poisoned: bool,
    unobserved: bool,
    graceful_end: Option<Instant>,
    stop_outcome: Option<AppleStopOutcome>,
}
impl AppleHelperOwner {
    pub fn new(ownership: Arc<File>) -> Self {
        Self {
            ownership,
            budgets: Budgets {
                read: Duration::from_secs(120),
                write: Duration::from_secs(30),
                exit: Duration::from_secs(45),
                cleanup: Duration::from_secs(10),
            },
            session: None,
            poisoned: false,
            unobserved: false,
            graceful_end: None,
            stop_outcome: None,
        }
    }
    fn available(&self) -> Result<(), HelperError> {
        if self.unobserved {
            Err(HelperError::ExitUnobserved)
        } else if self.poisoned {
            Err(HelperError::Poisoned)
        } else {
            Ok(())
        }
    }
    /// `command` is trusted native configuration, never renderer input. The later
    /// adapter resolves a fixed bundled executable and verified image capabilities.
    pub fn start(&mut self, mut command: Command, config: File) -> Result<UnixStream, HelperError> {
        self.available()?;
        if self.session.is_some() {
            return Err(HelperError::Busy);
        }
        self.graceful_end = None;
        self.stop_outcome = None;
        let (session, guest) = super::spawn_guard::run_blocking(|| -> io::Result<_> {
            // Darwin sets socket CLOEXEC after socketpair: all app spawns share
            // this guard until the temporary helper ends have been closed.
            let (control, helper_control) = UnixStream::pair()?;
            let (guest, helper_guest) = UnixStream::pair()?;
            let owner = self.ownership.try_clone()?;
            guest.set_read_timeout(Some(self.budgets.read))?;
            guest.set_write_timeout(Some(self.budgets.write))?;
            let fds = [
                config.as_raw_fd(),
                helper_control.as_raw_fd(),
                helper_guest.as_raw_fd(),
                owner.as_raw_fd(),
            ];
            command
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            for (name, fd) in [
                "--config-fd",
                "--supervisor-fd",
                "--guest-fd",
                "--ownership-fd",
            ]
            .into_iter()
            .zip(fds)
            {
                command.arg(name).arg(fd.to_string());
            }
            // SAFETY: descriptors remain owned through spawn. The child closure
            // only calls async-signal-safe fcntl; parent CLOEXEC flags are unchanged.
            unsafe {
                command.pre_exec(move || {
                    for fd in fds {
                        if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
            let child = command.spawn()?;
            drop((config, helper_control, helper_guest, owner));
            Ok((
                Session {
                    child,
                    control: Some(control),
                    next: 1,
                    cleanup_attempted: false,
                    budgets: self.budgets,
                    stop_deadline: None,
                },
                guest,
            ))
        })
        .map_err(io_error)?;
        self.session = Some(session);
        let mut magic = [0; 5];
        let result = read_until(
            self.session.as_ref().unwrap().control.as_ref().unwrap(),
            &mut magic,
            Instant::now() + self.budgets.read,
        )
        .and_then(|()| {
            if magic == *b"BRVM\x01" {
                Ok(())
            } else {
                Err(HelperError::Protocol)
            }
        });
        match result {
            Ok(()) => Ok(guest),
            Err(error) => Err(self.fail(error)),
        }
    }
    fn exchange(&mut self, opcode: u8, end: Instant) -> Result<u8, HelperError> {
        self.available()?;
        let session = self
            .session
            .as_mut()
            .ok_or(HelperError::Io(io::ErrorKind::NotConnected))?;
        let id = session.next;
        session.next = id.checked_add(1).ok_or(HelperError::Protocol)?;
        let mut request = [0; 9];
        request[0] = opcode;
        request[1..].copy_from_slice(&id.to_le_bytes());
        let control = session.control.as_ref().ok_or(HelperError::Poisoned)?;
        write_until(
            control,
            &request,
            end.min(Instant::now() + self.budgets.write),
        )?;
        let mut reply = [0; 9];
        read_until(control, &mut reply, end)?;
        // N proves natural termination only for the S racing a prepared stop.
        // Neither another operation nor an unprepared stop may claim it.
        let valid_opcode =
            reply[0] == b'K' || (reply[0] == b'N' && opcode == b'S' && self.graceful_end.is_some());
        if !valid_opcode || reply[1..] != id.to_le_bytes() {
            return Err(HelperError::Protocol);
        }
        Ok(reply[0])
    }
    fn operation(&mut self, opcode: u8) -> Result<(), HelperError> {
        self.available()?;
        if self.session.is_none() {
            return Err(HelperError::Io(io::ErrorKind::NotConnected));
        }
        match self.exchange(opcode, Instant::now() + self.budgets.read) {
            Ok(_) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }
    pub fn pause(&mut self) -> Result<(), HelperError> {
        self.operation(b'P')
    }
    pub fn resume(&mut self) -> Result<(), HelperError> {
        self.operation(b'R')
    }
    pub(crate) fn bound_stop(&mut self, end: Instant) {
        if let Some(session) = self.session.as_mut() {
            session.stop_deadline = Some(session.cap_end(end));
        }
    }
    /// Arm expected natural termination before asking the guest to power off.
    /// G is preparation only; its ACK never proves termination or a guest flush.
    pub fn prepare_shutdown(&mut self, end: Instant) -> Result<(), HelperError> {
        self.available()?;
        if let Some(previous) = self.graceful_end {
            let end = previous.min(end);
            self.graceful_end = Some(end);
            self.bound_stop(end + SHUTDOWN_FALLBACK);
            return Ok(()); // Never renew a prepared budget or replay G.
        }
        self.graceful_end = Some(end);
        self.bound_stop(end + SHUTDOWN_FALLBACK);
        // Reserve fallback time inside AppKit's existing 30-second window.
        // Unprepared startup-failure cleanup retains its original budgets.
        if let Some(session) = self.session.as_mut() {
            session.budgets.exit = session.budgets.exit.min(Duration::from_secs(5));
            session.budgets.cleanup = session.budgets.cleanup.min(Duration::from_secs(2));
        }
        // Draining a cancelled guest call can consume all grace before the
        // session mutex becomes available. Still arm native observation with
        // a bounded preparation exchange; never renew the stored guest grace.
        let now = Instant::now();
        let preparation_end = if end <= now {
            now + Duration::from_secs(1)
        } else {
            end
        };
        let preparation_end = self
            .session
            .as_ref()
            .map_or(preparation_end, |s| s.cap_end(preparation_end));
        match self.exchange(b'G', preparation_end) {
            Ok(_) => Ok(()),
            Err(error) => Err(self.fail(error)),
        }
    }
    pub fn stop_outcome(&self) -> Option<AppleStopOutcome> {
        self.stop_outcome
    }
    /// Prepared shutdown waits for natural native disposal and child exit first.
    /// Only timeout sends S. K is forced; N proves native stop won the race.
    /// Both receipts still require an observed successful helper exit.
    pub fn stop(&mut self) -> Result<(), HelperError> {
        self.available()?;
        if self.session.is_none() {
            return Ok(());
        }
        if let Some(end) = self.graceful_end {
            let end = self.session.as_ref().unwrap().cap_end(end);
            match wait_until(&mut self.session.as_mut().unwrap().child, end) {
                Ok(Some(status)) if status.success() => {
                    self.session = None;
                    self.stop_outcome = Some(AppleStopOutcome::GuestStopped);
                    return Ok(());
                }
                Ok(Some(_)) => return Err(self.fail(HelperError::Io(io::ErrorKind::Other))),
                Err(error) => return Err(self.fail(io_error(error))),
                Ok(None) => {} // Grace exhausted; only now use hard stop.
            }
        }
        let session = self.session.as_ref().unwrap();
        let end = session.cap_end(Instant::now() + session.budgets.exit);
        let result = self.exchange(b'S', end).and_then(|receipt| {
            match wait_until(&mut self.session.as_mut().unwrap().child, end).map_err(io_error)? {
                Some(status) if status.success() => Ok(receipt),
                Some(_) => Err(HelperError::Io(io::ErrorKind::Other)),
                None => Err(HelperError::Io(io::ErrorKind::TimedOut)),
            }
        });
        match result {
            Ok(receipt) => {
                self.session = None;
                self.stop_outcome = Some(if receipt == b'N' {
                    AppleStopOutcome::GuestStopped
                } else if self.graceful_end.is_some() {
                    AppleStopOutcome::ForcedStop
                } else {
                    AppleStopOutcome::HardStopped
                });
                Ok(())
            }
            Err(error) => Err(self.fail(error)),
        }
    }
    fn fail(&mut self, error: HelperError) -> HelperError {
        self.poisoned = true;
        if self.cleanup() {
            error
        } else {
            self.unobserved = true;
            HelperError::ExitUnobserved
        }
    }
    fn cleanup(&mut self) -> bool {
        let Some(session) = self.session.as_mut() else {
            return true;
        };
        if session.cleanup_attempted {
            return false;
        }
        session.cleanup_attempted = true;
        session.control = None; // EOF before TERM; no hidden parent-end copies.
        let observed = (|| -> io::Result<bool> {
            if session.child.try_wait()?.is_some() {
                return Ok(true);
            }
            // The owned child is unreaped: its PID cannot have been reused.
            unsafe {
                libc::kill(session.child.id() as i32, libc::SIGTERM);
            }
            let end = session.cap_end(Instant::now() + session.budgets.cleanup);
            if wait_until(&mut session.child, end)?.is_some() {
                return Ok(true);
            }
            if session.child.try_wait()?.is_some() {
                return Ok(true);
            }
            let _ = session.child.kill();
            // An explicitly absent cleanup budget keeps the killed child
            // quarantined; do not turn it into a scheduling-dependent observation.
            if session.budgets.cleanup.is_zero() {
                return Ok(false);
            }
            let end = session.cap_end(Instant::now() + session.budgets.cleanup);
            Ok(wait_until(&mut session.child, end)?.is_some())
        })()
        .unwrap_or(false);
        if observed {
            self.session = None;
        }
        observed
    }
}
impl Drop for AppleHelperOwner {
    fn drop(&mut self) {
        if !self.cleanup() {
            // No new cleanup budget after an exhausted attempt. An unobserved
            // writer must keep the original lock, even after RuntimeWorker drops.
            std::mem::forget((self.session.take(), self.ownership.clone()));
        }
    }
}
fn io_error(error: io::Error) -> HelperError {
    HelperError::Io(if error.kind() == io::ErrorKind::WouldBlock {
        io::ErrorKind::TimedOut
    } else {
        error.kind()
    })
}
fn remaining(end: Instant) -> Result<Duration, HelperError> {
    end.checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or(HelperError::Io(io::ErrorKind::TimedOut))
}
fn read_until(
    mut stream: &UnixStream,
    mut bytes: &mut [u8],
    end: Instant,
) -> Result<(), HelperError> {
    while !bytes.is_empty() {
        // Darwin can reject socket options after the peer closes even when a
        // complete reply is buffered. This control channel has one reader, so
        // readiness also bounds the following read without touching its options.
        let mut descriptor = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let timeout = remaining(end)?.as_millis().clamp(1, i32::MAX as u128) as i32;
        // SAFETY: the stream owns this descriptor throughout poll and read.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io_error(error));
        }
        if ready == 0 {
            continue;
        }
        match stream.read(bytes) {
            Ok(0) => return Err(HelperError::Io(io::ErrorKind::UnexpectedEof)),
            Ok(n) => {
                bytes = &mut bytes[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}
fn write_until(mut stream: &UnixStream, mut bytes: &[u8], end: Instant) -> Result<(), HelperError> {
    while !bytes.is_empty() {
        stream
            .set_write_timeout(Some(remaining(end)?))
            .map_err(io_error)?;
        match stream.write(bytes) {
            Ok(0) => return Err(HelperError::Io(io::ErrorKind::WriteZero)),
            Ok(n) => {
                bytes = &bytes[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}
fn wait_until(child: &mut Child, end: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        // Even a late caller must observe an exited helper before sending S.
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= end {
            return Ok(None);
        }
        std::thread::sleep(
            Duration::from_millis(2).min(end.saturating_duration_since(Instant::now())),
        );
    }
}
#[cfg(test)]
pub mod failure_fixture {
    use super::*;
    // Exported only in test crates: the owner-only harness has no backend caller.
    pub fn shorten(owner: &mut AppleHelperOwner) {
        owner.budgets.read = Duration::from_millis(100);
        owner.budgets.cleanup = Duration::ZERO;
        if let Some(session) = owner.session.as_mut() {
            session.budgets.cleanup = Duration::ZERO;
        }
    }
}
