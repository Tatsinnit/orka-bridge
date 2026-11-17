//! The native Windows owner; not setup activation or renderer configuration.
use super::{backend::Lease, job::OwnedQemuProcess};
use crate::runtime::{client::VmClient, handshake, types::RuntimeError, RuntimeWorker};
use std::sync::Arc;

pub struct WindowsSession {
    worker: RuntimeWorker,
    client: Option<VmClient>,
    owner: Arc<OwnedQemuProcess>,
    lease: Lease,
    shutdown_clean: bool,
}
impl WindowsSession {
    pub fn start(mut worker: RuntimeWorker, host_utc: u64) -> Result<Self, RuntimeError> {
        let result = (|| {
            worker.start()?;
            let lease = worker
                .lease()
                .ok_or(RuntimeError::Unavailable)?
                .handle
                .get::<Lease>()
                .ok_or(RuntimeError::Unavailable)?
                .clone();
            let owner = lease.owner()?;
            let channel = lease.guest()?;
            let client = handshake::establish(&mut worker, Box::new(channel), host_utc)?;
            Ok((lease, owner, client))
        })();
        let (lease, owner, client) = match result {
            Ok(ready) => ready,
            Err(error) => return Err(worker.startup_failed(error)),
        };
        Ok(Self {
            worker,
            client: Some(client),
            owner,
            lease,
            shutdown_clean: false,
        })
    }
    pub fn client_mut(&mut self) -> Result<&mut VmClient, RuntimeError> {
        self.client.as_mut().ok_or(RuntimeError::Closed)
    }
    pub fn shutdown(&mut self) -> Result<(), RuntimeError> {
        if self.client.is_none() {
            self.worker.stop()?;
            return if self.shutdown_clean {
                Ok(())
            } else {
                Err(RuntimeError::Closed)
            };
        }
        // Begin the same original budget before a guest can acknowledge and
        // exit. GuestStream clamps its native I/O to this existing deadline.
        self.lease.prepare_shutdown()?;
        let ack = self.client.as_mut().expect("live client").shutdown();
        let stopped = self.worker.stop().and_then(|()| {
            if self.owner.exit_code()? == 0 {
                Ok(())
            } else {
                Err(io_error("QEMU exited unsuccessfully".into()))
            }
        });
        self.client = None;
        let result = match (ack, stopped) {
            (Err(ack), Err(stop)) => Err(io_error(format!(
                "guest shutdown failed: {ack}; shutdown completion failed: {stop}"
            ))),
            (Err(ack), Ok(())) => Err(ack.into()),
            (Ok(()), stop) => stop,
        };
        self.shutdown_clean = result.is_ok();
        result
    }
    pub fn diagnostics(&self) -> Result<super::backend::Diagnostics, RuntimeError> {
        self.lease.diagnostics()
    }
}
fn io_error(message: String) -> RuntimeError {
    std::io::Error::other(message).into()
}
impl Drop for WindowsSession {
    fn drop(&mut self) {
        if self.shutdown().is_err() {
            // RuntimeWorker's existing failed-stop quarantine retains the
            // journal/backend bundle; this attempt never reconstructs a lease.
            let _ = self.owner.abort();
        }
    }
}
