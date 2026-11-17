pub mod acquire;
#[cfg(unix)]
pub mod apple;
#[cfg(unix)]
pub mod apple_backend;
#[cfg(unix)]
pub(crate) mod apple_channel;
#[cfg(unix)]
pub mod apple_input;
#[cfg(target_os = "macos")]
pub mod apple_session;
pub mod client;
pub mod credential_store;
pub mod credentials;
pub mod handshake;
pub mod identity;
pub mod image_cache;
pub mod image_install;
pub mod image_types;
pub mod journal;
pub mod manifest;
pub mod setup_adapter;
pub mod snapshot;
pub mod spawn_guard;
pub mod types;
pub mod windows;
pub mod wire;

use bridge_runtime_protocol::*;
use image_install::InstalledImageSet;
use journal::{Journal, StatePhase};
use std::{
    mem::ManuallyDrop,
    sync::{Arc, Mutex},
};
use types::*;

/// Serialized owner of backend capabilities and the installation lock.
/// No backend implementation or endpoint is shipped by this core.
pub struct RuntimeWorker {
    backend: ManuallyDrop<Box<dyn VmBackend>>,
    journal: ManuallyDrop<Journal>,
    image: Arc<InstalledImageSet>,
    lease: ManuallyDrop<Option<VmLease>>,
    next_stage: Option<Stage>,
    early_guest: Option<Uuid>,
    retry_custody: Option<Arc<Mutex<journal::JournalCustody>>>,
}
impl RuntimeWorker {
    pub fn new(
        backend: Box<dyn VmBackend>,
        journal: Journal,
        image: Arc<InstalledImageSet>,
    ) -> Result<Self, RuntimeError> {
        journal.validate_image(&image)?;
        Ok(Self {
            backend: ManuallyDrop::new(backend),
            journal: ManuallyDrop::new(journal),
            image,
            lease: ManuallyDrop::new(None),
            next_stage: None,
            early_guest: None,
            retry_custody: None,
        })
    }
    #[cfg(any(
        test,
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    // Protocol integration tests import this module without the native app owner.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn retain_journal(mut self, custody: Arc<Mutex<journal::JournalCustody>>) -> Self {
        self.retry_custody = Some(custody);
        self
    }
    #[cfg(any(test, windows, target_os = "macos"))]
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn startup_failed(mut self, error: RuntimeError) -> RuntimeError {
        if matches!(error, RuntimeError::StopUnconfirmed(_)) {
            std::mem::forget(self);
            return error;
        }
        if let Err(cleanup) = self.stop() {
            std::mem::forget(self);
            return RuntimeError::StopUnconfirmed(format!(
                "{error}; stopping the VM failed: {cleanup}"
            ));
        }
        error
    }
    pub fn lease(&self) -> Option<&VmLease> {
        self.lease.as_ref()
    }
    pub fn start(&mut self) -> Result<(), RuntimeError> {
        if self.lease.is_some() {
            return Err(RuntimeError::Busy);
        }
        self.journal.verify_state()?;
        self.journal.validate_image(&self.image)?;
        if self.journal.record().phase == StatePhase::Empty {
            self.journal.require_fresh_creation()?;
            self.journal.set_phase(StatePhase::Preparing)?;
            self.backend
                .create_state(&self.journal.spec(Uuid::new_v4(), self.image.clone()))?;
            self.journal.finish_preparation()?;
        }
        let spec = self.journal.begin_boot(self.image.clone())?;
        let handle = self.backend.start(&spec)?;
        *self.lease = Some(VmLease { spec, handle });
        self.next_stage = Some(Stage::Early);
        self.early_guest = None;
        Ok(())
    }
    pub fn boot_request(
        &mut self,
        hello: &StageHello,
        host_utc: u64,
    ) -> Result<WireRequest, RuntimeError> {
        let lease = self.lease.as_ref().ok_or(RuntimeError::Unavailable)?;
        let stage = self.next_stage.ok_or(ProtocolError::Stage)?;
        validate_hello(hello, stage, lease.spec.boot_nonce)?;
        if self.early_guest == Some(hello.guest_nonce) {
            return Err(ProtocolError::Nonce.into());
        }
        // Consume and flush before the caller can send a new descriptor.
        let mode = self.journal.consume_creation(&lease.spec)?;
        let mode = if stage == Stage::Serve {
            StateCreation::Existing
        } else {
            mode
        };
        let request = WireRequest {
            version: VERSION,
            boot_nonce: lease.spec.boot_nonce,
            request_id: if stage == Stage::Early { 1 } else { 2 },
            body: RequestBody::Boot(BootDescriptor {
                installation_id: lease.spec.installation_id,
                generation: lease.spec.generation,
                boot_nonce: lease.spec.boot_nonce,
                state_creation: mode,
                state_fs_uuid: lease.spec.installation_id,
                state_bytes: STATE_BYTES,
                host_utc,
            }),
        };
        self.next_stage = if stage == Stage::Early {
            self.early_guest = Some(hello.guest_nonce);
            Some(Stage::Serve)
        } else {
            None
        };
        Ok(request)
    }
    pub fn stop(&mut self) -> Result<(), RuntimeError> {
        if let Some(lease) = self.lease.as_ref() {
            self.backend.stop(&lease.handle)?;
        }
        *self.lease = None;
        self.next_stage = None;
        Ok(())
    }
    pub fn pause(&mut self) -> Result<(), RuntimeError> {
        self.backend
            .pause(&self.lease.as_ref().ok_or(RuntimeError::Unavailable)?.handle)
    }
    pub fn resume(&mut self) -> Result<(), RuntimeError> {
        self.backend
            .resume(&self.lease.as_ref().ok_or(RuntimeError::Unavailable)?.handle)
    }
}
impl Drop for RuntimeWorker {
    fn drop(&mut self) {
        if self.stop().is_ok() {
            // SAFETY: these fields are dropped exactly once, only after observed
            // stop. A failed or panicking stop instead quarantines all ownership
            // resources until process exit; it must not unlock an unconfirmed VM.
            unsafe {
                ManuallyDrop::drop(&mut self.lease);
                ManuallyDrop::drop(&mut self.backend);
                let journal = ManuallyDrop::take(&mut self.journal);
                if let Some(custody) = self.retry_custody.take() {
                    if let Ok(mut held) = custody.lock() {
                        held.0 = Some(journal);
                    }
                }
            }
        }
    }
}
#[derive(Default)]
pub struct RuntimeState(Mutex<Owner>);
#[derive(Default)]
struct Owner {
    worker: Option<RuntimeWorker>,
    closed: bool,
}
impl RuntimeState {
    pub fn with_worker(worker: RuntimeWorker) -> Self {
        Self(Mutex::new(Owner {
            worker: Some(worker),
            closed: false,
        }))
    }
    pub fn start(&self) -> Result<(), RuntimeError> {
        let mut owner = self.0.lock().map_err(|_| RuntimeError::Closed)?;
        if owner.closed {
            return Err(RuntimeError::Closed);
        }
        owner
            .worker
            .as_mut()
            .ok_or(RuntimeError::Unavailable)?
            .start()
    }
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        let mut owner = self.0.lock().map_err(|_| RuntimeError::Closed)?;
        owner.closed = true;
        if let Some(worker) = &mut owner.worker {
            worker.stop()?;
        }
        Ok(())
    }
}
