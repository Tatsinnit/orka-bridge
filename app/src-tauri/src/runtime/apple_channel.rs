//! Revocable, single-descriptor guest capability. Wrappers never retain an FD.
use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
// Allow measured container teardown while reserving native fallback inside AppKit30.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(18);

#[derive(Clone)]
pub(crate) struct GuestSlot {
    inner: Arc<Inner>,
}
struct Inner {
    state: Mutex<State>,
    idle: Condvar,
}
struct State {
    socket: Option<Arc<UnixStream>>,
    sealed: bool,
    active: bool,
    shutdown_deadline: Option<Instant>,
    close: Option<CloseSignal>,
}
struct CloseSignal {
    receiver: tokio::sync::watch::Receiver<bool>,
    grace: Duration,
    origin: Option<Arc<Mutex<Option<Instant>>>>,
}
struct Scoped {
    slot: GuestSlot,
    socket: Option<Arc<UnixStream>>,
}
impl Drop for Scoped {
    fn drop(&mut self) {
        // Drop the scoped FD reference BEFORE notifying the final-close observer.
        self.socket.take();
        let mut state = self.slot.inner.state.lock().unwrap();
        state.active = false;
        self.slot.inner.idle.notify_all();
    }
}
impl GuestSlot {
    pub(crate) fn new(socket: UnixStream) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    socket: Some(Arc::new(socket)),
                    sealed: false,
                    active: false,
                    shutdown_deadline: None,
                    close: None,
                }),
                idle: Condvar::new(),
            }),
        }
    }
    pub(crate) fn channel(&self) -> Channel {
        Channel {
            slot: self.clone(),
            deadline: None,
        }
    }
    fn acquire(&self) -> io::Result<Scoped> {
        let mut state = self.inner.state.lock().unwrap();
        if state.sealed {
            return Err(io::ErrorKind::NotConnected.into());
        }
        if state.active {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let socket = state.socket.clone().ok_or(io::ErrorKind::NotConnected)?;
        state.active = true;
        Ok(Scoped {
            slot: self.clone(),
            socket: Some(socket),
        })
    }
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn bind_close(
        &self,
        receiver: tokio::sync::watch::Receiver<bool>,
        origin: Option<Arc<Mutex<Option<Instant>>>>,
    ) {
        self.inner.state.lock().unwrap().close = Some(CloseSignal {
            receiver,
            grace: SHUTDOWN_GRACE,
            origin,
        });
    }
    pub(crate) fn shutdown_deadline(&self) -> Option<Instant> {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(close) = &state.close {
            let closed = *close.receiver.borrow();
            if closed {
                let origin = close
                    .origin
                    .as_ref()
                    .and_then(|at| *at.lock().unwrap())
                    .unwrap_or_else(Instant::now);
                let end = origin + close.grace;
                state.shutdown_deadline =
                    Some(state.shutdown_deadline.map_or(end, |old| old.min(end)));
            }
        }
        state.shutdown_deadline
    }
    pub(crate) fn prepare_shutdown(&self, end: Instant) {
        let mut state = self.inner.state.lock().unwrap();
        state.shutdown_deadline = Some(state.shutdown_deadline.map_or(end, |old| old.min(end)));
    }
    pub(crate) fn seal(&self) {
        self.inner.state.lock().unwrap().sealed = true;
    }
    pub(crate) fn revoke(&self, end: Instant) -> io::Result<()> {
        self.seal();
        let socket = self.inner.state.lock().unwrap().socket.take();
        // No mutex is held during shutdown; this wakes a blocked scoped read.
        if let Some(socket) = socket {
            // Darwin reports ENOTCONN after the observed helper has closed its end.
            if let Err(error) = socket.shutdown(Shutdown::Both) {
                if error.kind() != io::ErrorKind::NotConnected {
                    return Err(error);
                }
            }
            drop(socket);
        }
        let mut state = self.inner.state.lock().unwrap();
        while state.active {
            let remaining = end
                .checked_duration_since(Instant::now())
                .ok_or(io::ErrorKind::TimedOut)?;
            state = self.inner.idle.wait_timeout(state, remaining).unwrap().0;
        }
        Ok(())
    }
}
pub(crate) struct Channel {
    slot: GuestSlot,
    pub(crate) deadline: Option<Instant>,
}
impl Channel {
    fn budget(&self, seconds: u64) -> io::Result<Duration> {
        let budget = Duration::from_secs(seconds);
        let shutdown = self.slot.shutdown_deadline();
        let deadline = match (self.deadline, shutdown) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match deadline {
            Some(end) => end
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .map(|d| d.min(budget))
                .ok_or_else(|| io::ErrorKind::TimedOut.into()),
            None => Ok(budget),
        }
    }
    fn io(
        &self,
        write: bool,
        mut action: impl FnMut(&UnixStream) -> io::Result<usize>,
    ) -> io::Result<usize> {
        let scoped = self.slot.acquire()?;
        let socket = scoped.socket.as_ref().unwrap();
        let seconds = if write { 30 } else { 120 };
        let end = Instant::now() + self.budget(seconds)?;
        loop {
            let remaining = end
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or(io::ErrorKind::TimedOut)?;
            // Observe an existing close watch even when this syscall began
            // before shutdown admission, without renewing either deadline.
            let slice = self
                .budget(seconds)?
                .min(remaining)
                .min(Duration::from_millis(100));
            if write {
                socket.set_write_timeout(Some(slice))?;
            } else {
                socket.set_read_timeout(Some(slice))?;
            }
            match action(socket) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
}
impl Read for Channel {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.io(false, |mut s| s.read(bytes))
    }
}
impl Write for Channel {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.io(true, |mut s| s.write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        self.io(true, |_| Ok(0)).map(|_| ())
    }
}
#[cfg(test)]
#[path = "apple_channel_tests.rs"]
pub(crate) mod tests;
