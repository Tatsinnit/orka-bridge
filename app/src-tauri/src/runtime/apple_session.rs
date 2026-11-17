//! Serving composition for trusted native callers only; no app activation.
use super::{
    apple_backend::{AppleBackend, AppleBackendHandle},
    apple_channel::Channel,
    client::VmClient,
    image_install::InstalledImageSet,
    journal::Journal,
    types::RuntimeError,
    RuntimeWorker,
};
use crate::engine::vm::VmTransport;
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
        Self::start(worker, host_utc)
    }
    #[cfg(target_arch = "aarch64")]
    pub(crate) fn launch_retaining(
        resource_root: PathBuf,
        journal: Journal,
        image: Arc<InstalledImageSet>,
        host_utc: u64,
        custody: Arc<std::sync::Mutex<super::journal::JournalCustody>>,
    ) -> Result<Self, RuntimeError> {
        let worker =
            RuntimeWorker::new(Box::new(AppleBackend::new(resource_root)), journal, image)?
                .retain_journal(custody);
        Self::start(worker, host_utc)
    }
    fn start(worker: RuntimeWorker, host_utc: u64) -> Result<Self, RuntimeError> {
        let mut session = Self {
            worker,
            transport: None,
        };
        let result = (|| {
            session.worker.start()?;
            let lease = session.worker.lease().ok_or(RuntimeError::Unavailable)?;
            let nonce = lease.spec.boot_nonce;
            let handle = lease
                .handle
                .get::<AppleBackendHandle>()
                .ok_or(RuntimeError::Unavailable)?;
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
            session.transport = None;
            return Err(session.worker.startup_failed(error));
        }
        Ok(session)
    }
    pub fn transport(&self) -> Option<&VmTransport> {
        self.transport.as_ref()
    }
    pub fn stop(&mut self) -> Result<(), RuntimeError> {
        self.transport = None;
        self.worker.stop()
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
