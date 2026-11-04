use bridge_runtime_protocol::{ProtocolError, Uuid};
use serde::{Deserialize, Serialize};
use std::{any::Any, fs::File, path::PathBuf, sync::Arc};

#[derive(Debug)]
pub enum RuntimeError {
    Io(std::io::Error),
    Busy,
    RetainedState,
    Unavailable,
    Closed,
    Protocol(ProtocolError),
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Runtime operation failed: {self:?}")
    }
}
impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Protocol(e) => Some(e),
            _ => None,
        }
    }
}
impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<ProtocolError> for RuntimeError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackendKind {
    Apple,
    Qemu,
}
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub installation_id: Uuid,
    pub generation: u64,
    pub boot_nonce: Uuid,
    /// Backends retain/inherit this lock capability for the complete VM lifetime,
    /// including parent death; never reopen a pathname to reconstruct ownership.
    pub ownership: Arc<File>,
    pub root: PathBuf,
    pub state_path: PathBuf,
    pub state_bytes: u64,
    pub backend: BackendKind,
    pub machine_id: Vec<u8>,
    pub mac: [u8; 6],
}
/// A backend-owned capability, not a PID which could be reused by the OS.
pub struct BackendHandle(Box<dyn Any + Send>);
impl BackendHandle {
    pub fn new<T: Any + Send>(handle: T) -> Self {
        Self(Box::new(handle))
    }
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}
pub struct VmLease {
    pub spec: LaunchSpec,
    pub(super) handle: BackendHandle,
}
/// Every successful stop/pause/resume observes completion. A failed start must
/// clean up anything it created before returning; no lease is reconstructed by PID.
/// Native implementations must bind guest lifetime to the app and preserve the
/// supplied ownership lock until that guest has stopped, even on parent death.
pub trait VmBackend: Send {
    fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError>;
    fn start(&mut self, spec: &LaunchSpec) -> Result<BackendHandle, RuntimeError>;
    fn stop(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError>;
    fn pause(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError>;
    fn resume(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError>;
}
