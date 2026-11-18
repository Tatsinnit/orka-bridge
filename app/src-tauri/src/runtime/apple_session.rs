//! Serving composition for trusted native callers only; no app activation.
use crate::engine::vm::VmTransport;
use crate::runtime::{
    apple::AppleStopOutcome,
    apple_backend::{AppleBackend, AppleBackendHandle},
    apple_channel::{Channel, SHUTDOWN_GRACE},
    client::VmClient,
    image_install::InstalledImageSet,
    journal::Journal,
    types::RuntimeError,
    RuntimeWorker,
};
use bridge_runtime_protocol::*;
use std::{
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
pub struct AppleVmSession {
    worker: RuntimeWorker,
    transport: Option<VmTransport>,
    shutdown_deadline: Option<Instant>,
    shutdown_forced: bool,
}
impl AppleVmSession {
    pub fn launch(
        resource_root: PathBuf,
        journal: Journal,
        image: Arc<InstalledImageSet>,
        host_utc: u64,
    ) -> Result<Self, RuntimeError> {
        let worker =
            RuntimeWorker::new(Box::new(AppleBackend::new(resource_root)), journal, image)?;
        Self::start(worker, host_utc, None, None)
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn launch_retaining(
        resource_root: PathBuf,
        journal: Journal,
        image: Arc<InstalledImageSet>,
        host_utc: u64,
        custody: Arc<std::sync::Mutex<super::journal::JournalCustody>>,
        closed: tokio::sync::watch::Receiver<bool>,
        close_origin: Arc<std::sync::Mutex<Option<Instant>>>,
    ) -> Result<Self, RuntimeError> {
        let worker =
            RuntimeWorker::new(Box::new(AppleBackend::new(resource_root)), journal, image)?
                .retain_journal(custody);
        Self::start(worker, host_utc, Some(closed), Some(close_origin))
    }
    fn start(
        worker: RuntimeWorker,
        host_utc: u64,
        closed: Option<tokio::sync::watch::Receiver<bool>>,
        close_origin: Option<Arc<std::sync::Mutex<Option<Instant>>>>,
    ) -> Result<Self, RuntimeError> {
        let cancellation = closed.clone();
        let mut session = Self {
            worker,
            transport: None,
            shutdown_deadline: None,
            shutdown_forced: false,
        };
        let result = (|| {
            session.worker.start()?;
            let lease = session.worker.lease().ok_or(RuntimeError::Unavailable)?;
            let nonce = lease.spec.boot_nonce;
            let handle = lease
                .handle
                .get::<AppleBackendHandle>()
                .ok_or(RuntimeError::Unavailable)?;
            if let Some(closed) = closed {
                handle.guest.bind_close(closed, close_origin);
            }
            let mut channel = handle.guest.channel();
            channel.deadline = Some(Instant::now() + Duration::from_secs(120));
            let mut serving_hello = None;
            for stage in [Stage::Early, Stage::Serve] {
                let Frame::Hello(hello) = read_frame(&mut channel)? else {
                    return Err(ProtocolError::Stage.into());
                };
                // This consumes and flushes New before the first descriptor write.
                let request = session.worker.boot_request(&hello, host_utc)?;
                if stage == Stage::Serve {
                    serving_hello = Some(hello);
                }
                let id = request.request_id;
                channel.write_all(&encode(&Frame::Request(request))?)?;
                let Frame::Result(ready) = read_frame(&mut channel)? else {
                    return Err(ProtocolError::Stage.into());
                };
                if ready.boot_nonce != nonce
                    || ready.request_id != id
                    || !matches!(ready.body, ResultBody::Ready {})
                {
                    return Err(ProtocolError::Nonce.into());
                }
            }
            // One client, one request sequence; transport takes it only after Ready.
            channel.deadline = None;
            let client = VmClient::established_with_hello(
                Box::new(channel),
                nonce,
                serving_hello.as_ref().ok_or(RuntimeError::Unavailable)?,
            )?;
            session.transport = Some(VmTransport::with_client(client));
            Ok(())
        })();
        if let Err(error) = result {
            let close_timeout = matches!(&error, RuntimeError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut)
                && cancellation.as_ref().is_some_and(|closed| *closed.borrow())
                && session.worker.lease().is_some_and(|lease| {
                    lease
                        .handle
                        .get::<AppleBackendHandle>()
                        .is_some_and(|handle| {
                            handle
                                .guest
                                .shutdown_deadline()
                                .is_some_and(|end| end <= Instant::now())
                        })
                });
            let error = if close_timeout {
                RuntimeError::Closed
            } else {
                error
            };
            session.transport = None;
            if matches!(&error, RuntimeError::StopUnconfirmed(_)) {
                return Err(session.worker.startup_failed(error));
            }
            let owner = session
                .worker
                .lease()
                .and_then(|lease| lease.handle.get::<AppleBackendHandle>())
                .map(|handle| handle.owner.clone());
            let cleanup = session.worker.stop();
            let hard_cleanup = owner.as_ref().is_some_and(|owner| {
                matches!(
                    owner.lock().unwrap().stop_outcome(),
                    Some(AppleStopOutcome::HardStopped | AppleStopOutcome::ForcedStop)
                )
            });
            // Release this observer before dropping the worker: its journal can
            // explicitly unlock only after backend ownership references drop.
            drop(owner);
            if let Err(cleanup) = cleanup {
                return Err(session
                    .worker
                    .startup_failed(RuntimeError::StopUnconfirmed(format!(
                        "{error}; stopping the VM failed: {cleanup}"
                    ))));
            }
            return Err(if hard_cleanup {
                if matches!(&error, RuntimeError::Closed) {
                    RuntimeError::ShutdownForced
                } else {
                    RuntimeError::Startup(Box::new(crate::runtime::types::StartupFailure {
                        error: RuntimeError::ShutdownForced,
                        operation: crate::runtime::types::WorkspaceOperation::Stop,
                        diagnostic: format!(
                            "{error}; native hard cleanup completed, not a clean guest shutdown"
                        ),
                    }))
                }
            } else {
                error
            });
        }
        Ok(session)
    }
    pub fn transport(&self) -> Option<&VmTransport> {
        self.transport.as_ref()
    }
    pub fn stop(&mut self) -> Result<(), RuntimeError> {
        let owner = self
            .worker
            .lease()
            .and_then(|lease| lease.handle.get::<AppleBackendHandle>())
            .map(|handle| handle.owner.clone());
        if let (Some(transport), Some(lease)) = (&self.transport, self.worker.lease()) {
            let handle = lease
                .handle
                .get::<AppleBackendHandle>()
                .ok_or(RuntimeError::Unavailable)?;
            let end = *self
                .shutdown_deadline
                .get_or_insert_with(|| Instant::now() + SHUTDOWN_GRACE);
            // Close publication may precede this call and active I/O may have
            // consumed its grace. Keep the earlier, non-renewable budget.
            let end = handle
                .guest
                .shutdown_deadline()
                .map_or(end, |earlier| earlier.min(end));
            self.shutdown_deadline = Some(end);
            // Prepare native observation BEFORE the guest can power off. Release
            // the owner mutex before borrowing the single protocol client.
            let prepared = handle.prepare_shutdown(end);
            if prepared.is_ok() {
                // Reply loss is not native failure. Always observe the helper,
                // including with old guests that power off before replying.
                let _ = transport.borrow_client(|client| client.shutdown());
            }
        }
        self.transport = None;
        // A confirmed hard fallback clears the lease just like native stop.
        // Report it as unclean only AFTER custody has been accounted for.
        let stopped = self.worker.stop();
        if let Some(owner) = owner {
            self.shutdown_forced |= matches!(
                owner.lock().unwrap().stop_outcome(),
                Some(AppleStopOutcome::ForcedStop | AppleStopOutcome::HardStopped)
            );
        }
        stopped?;
        if self.shutdown_forced {
            eprintln!("Mac workspace required a hard stop; not a clean guest shutdown.");
            Err(RuntimeError::ShutdownForced)
        } else {
            Ok(())
        }
    }
    pub fn pause(&mut self) -> Result<(), RuntimeError> {
        let result = self.worker.pause();
        if result.is_err() {
            self.transport = None;
        }
        result
    }
    pub fn resume(&mut self) -> Result<(), RuntimeError> {
        let result = self.worker.resume();
        if result.is_err() {
            self.transport = None;
        }
        result
    }
}
fn read_frame(channel: &mut Channel) -> Result<Frame, RuntimeError> {
    let mut bytes = Vec::new();
    for _ in 0..MAX_FRAME {
        let mut byte = [0];
        channel.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            let frame: Frame =
                serde_json::from_slice(&bytes).map_err(|_| ProtocolError::Invalid)?;
            frame.validate()?;
            return Ok(frame);
        }
    }
    Err(ProtocolError::Oversize.into())
}
#[cfg(test)]
#[path = "apple_session_tests.rs"]
mod tests;
