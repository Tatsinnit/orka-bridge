pub mod client;
pub mod identity;
pub mod journal;
pub mod snapshot;
pub mod types;
pub mod wire;

use bridge_runtime_protocol::*;
use journal::{Journal, StatePhase};
use std::{mem::ManuallyDrop, sync::Mutex};
use types::*;

/// Serialized owner of backend capabilities and the installation lock.
/// No backend implementation or endpoint is shipped by this core.
pub struct RuntimeWorker {
    backend: ManuallyDrop<Box<dyn VmBackend>>,
    journal: ManuallyDrop<Journal>,
    lease: ManuallyDrop<Option<VmLease>>,
    next_stage: Option<Stage>,
    early_guest: Option<Uuid>,
}
impl RuntimeWorker {
    pub fn new(backend: Box<dyn VmBackend>, journal: Journal) -> Self {
        Self {
            backend: ManuallyDrop::new(backend),
            journal: ManuallyDrop::new(journal),
            lease: ManuallyDrop::new(None),
            next_stage: None,
            early_guest: None,
        }
    }
    pub fn lease(&self) -> Option<&VmLease> {
        self.lease.as_ref()
    }
    pub fn start(&mut self) -> Result<(), RuntimeError> {
        if self.lease.is_some() {
            return Err(RuntimeError::Busy);
        }
        self.journal.verify_state()?;
        if self.journal.record().phase == StatePhase::Empty {
            self.journal.require_fresh_creation()?;
            self.journal.set_phase(StatePhase::Preparing)?;
            self.backend
                .create_state(&self.journal.spec(Uuid::new_v4()))?;
            self.journal.finish_preparation()?;
        }
        let spec = self.journal.begin_boot()?;
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
                ManuallyDrop::drop(&mut self.journal);
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
