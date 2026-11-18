use super::{image_install::InstalledImageSet, image_types::ImageError};
use bridge_runtime_protocol::{ProtocolError, Uuid};
use serde::{Deserialize, Serialize};
use std::{any::Any, fs::File, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HostVirtualization {
    Ready,
    Disabled,
    HypervisorUnavailable,
    Unsupported,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PreflightFailure {
    VirtualizationDisabled,
    HypervisorUnavailable,
    VirtualizationUnsupported,
    InsufficientMemory,
}

/// The finite step a workspace failure belongs to. `Download` is reserved for a typed
/// network failure; no native acquisition path exists to report progress for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceOperation {
    Requirements,
    Download,
    Verify,
    Prepare,
    Start,
    Services,
    Stop,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceCause {
    Permission,
    NoSpace,
    Integrity,
    WorkspaceUpdateRequired,
    WorkspaceVersionUnsupported,
    InUse,
    Network,
    Preparation,
    GuestConnection,
    Services,
    StopUnconfirmed,
    ForcedStop,
    Unknown,
}
/// Bounded renderer-safe facts. Private diagnostics never contribute to these fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFailure {
    pub operation: WorkspaceOperation,
    pub cause: WorkspaceCause,
    pub codes: Vec<u32>,
}
impl WorkspaceFailure {
    pub(crate) fn new(
        operation: WorkspaceOperation,
        cause: WorkspaceCause,
        codes: impl IntoIterator<Item = u32>,
    ) -> Self {
        let mut unique = Vec::new();
        for code in codes {
            if unique.len() < 4 && !unique.contains(&code) {
                unique.push(code);
            }
        }
        Self {
            operation,
            cause,
            codes: unique,
        }
    }
    /// Classify typed errors only; Display/Debug text is private and never inspected.
    pub fn from_error(error: &RuntimeError, operation: WorkspaceOperation) -> Self {
        use {WorkspaceCause as C, WorkspaceOperation as O};
        let stopping = operation == O::Stop;
        let (operation, cause, code) = match error {
            RuntimeError::Startup(context) => {
                return Self::from_error(
                    &context.error,
                    if stopping { O::Stop } else { context.operation },
                );
            }
            RuntimeError::Preflight(_) => (O::Requirements, C::Unknown, None),
            RuntimeError::Image(ImageError::Network(_)) => (O::Download, C::Network, None),
            RuntimeError::Infrastructure(_) => (O::Services, C::Services, None),
            RuntimeError::StopUnconfirmed(_) => (O::Stop, C::StopUnconfirmed, None),
            RuntimeError::ShutdownForced => (O::Stop, C::ForcedStop, None),
            RuntimeError::Io(error) => (
                operation,
                match error.kind() {
                    std::io::ErrorKind::PermissionDenied => C::Permission,
                    std::io::ErrorKind::StorageFull => C::NoSpace,
                    _ => C::Unknown,
                },
                error
                    .raw_os_error()
                    .and_then(|code| u32::try_from(code).ok()),
            ),
            RuntimeError::Busy => (operation, C::InUse, None),
            RuntimeError::RetainedState => (operation, C::Preparation, None),
            RuntimeError::Protocol(_) => (operation, C::GuestConnection, None),
            RuntimeError::Unavailable | RuntimeError::Closed => (operation, C::Unknown, None),
            RuntimeError::Image(error) => (
                operation,
                match error {
                    ImageError::Invalid(_)
                    | ImageError::Unsupported
                    | ImageError::Incompatible(_)
                    | ImageError::Integrity
                    | ImageError::LimitExceeded
                    | ImageError::UnsafePath
                    | ImageError::RetainedImage => C::Integrity,
                    ImageError::WorkspaceUpdateRequired => C::WorkspaceUpdateRequired,
                    ImageError::WorkspaceVersionUnsupported => C::WorkspaceVersionUnsupported,
                    ImageError::NoSpace => C::NoSpace,
                    ImageError::Busy => C::InUse,
                    ImageError::Storage(std::io::ErrorKind::PermissionDenied) => C::Permission,
                    ImageError::Storage(std::io::ErrorKind::StorageFull) => C::NoSpace,
                    ImageError::Storage(_) => C::Preparation,
                    // Acknowledged cancellation never reaches a failure snapshot.
                    ImageError::Cancelled | ImageError::Network(_) => C::Unknown,
                },
                None,
            ),
        };
        // A confirmed forced exit is unclean, but has no unobserved writer.
        let operation = if stopping { O::Stop } else { operation };
        let cause = if operation == O::Stop && cause != C::ForcedStop {
            C::StopUnconfirmed
        } else {
            cause
        };
        Self::new(operation, cause, code)
    }
}
/// A typed startup failure with private diagnostic context, including earlier errors.
#[derive(Debug)]
pub struct StartupFailure {
    pub error: RuntimeError,
    pub operation: WorkspaceOperation,
    pub diagnostic: String,
}

#[derive(Debug)]
pub enum RuntimeError {
    Io(std::io::Error),
    Busy,
    RetainedState,
    Unavailable,
    Preflight(PreflightFailure),
    Closed,
    /// A native writer may still be alive; never admit another startup.
    StopUnconfirmed(String),
    /// Native termination was confirmed, but required destructive cleanup.
    ShutdownForced,
    Protocol(ProtocolError),
    Image(ImageError),
    /// The guest started but its services did not become ready; text is private.
    Infrastructure(String),
    Startup(Box<StartupFailure>),
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Infrastructure(detail) => f.write_str(detail),
            Self::Startup(context) => f.write_str(&context.diagnostic),
            _ => write!(f, "Runtime operation failed: {self:?}"),
        }
    }
}
impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Protocol(e) => Some(e),
            Self::Startup(context) => Some(&context.error),
            _ => None,
        }
    }
}
#[derive(Debug)]
pub(crate) struct UnconfirmedStop(pub String);
impl std::fmt::Display for UnconfirmedStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for UnconfirmedStop {}
impl From<PreflightFailure> for RuntimeError {
    fn from(failure: PreflightFailure) -> Self {
        Self::Preflight(failure)
    }
}
impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        if e.get_ref()
            .is_some_and(|inner| inner.is::<UnconfirmedStop>())
        {
            Self::StopUnconfirmed(e.to_string())
        } else {
            Self::Io(e)
        }
    }
}
impl From<ProtocolError> for RuntimeError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}
impl From<ImageError> for RuntimeError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
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
    /// Backends retain this Arc for the complete VM lifetime, including while a
    /// helper that inherited its descriptor may run; inheritance covers parent
    /// death. Never reopen a pathname to reconstruct ownership. A journal dropped
    /// with the only Arc unlocks it, which on Unix also releases inherited copies.
    pub ownership: Arc<File>,
    /// A committed private image, revalidated under the same installation owner.
    pub image: Arc<InstalledImageSet>,
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
    pub(crate) handle: BackendHandle,
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
#[cfg(test)]
mod workspace_failure_tests {
    use super::*;
    use crate::runtime::image_types::{ImageError as I, Incompatibility};
    use std::io::{Error, ErrorKind};
    use WorkspaceCause as C;
    use WorkspaceOperation as O;

    fn classify(error: RuntimeError, op: O) -> (O, C, Vec<u32>) {
        let f = WorkspaceFailure::from_error(&error, op);
        (f.operation, f.cause, f.codes)
    }
    #[test]
    fn confirmed_forced_stop_is_not_classified_as_an_unobserved_writer() {
        assert_eq!(
            classify(RuntimeError::ShutdownForced, O::Stop),
            (O::Stop, C::ForcedStop, vec![])
        );
    }
    #[test]
    fn every_typed_error_selects_its_documented_finite_category() {
        let denied = || Error::from(ErrorKind::PermissionDenied);
        let cases = vec![
            (RuntimeError::Io(denied()), O::Start, C::Permission),
            (
                RuntimeError::Io(Error::from(ErrorKind::StorageFull)),
                O::Prepare,
                C::NoSpace,
            ),
            (RuntimeError::Io(Error::other("x")), O::Start, C::Unknown),
            (RuntimeError::Busy, O::Start, C::InUse),
            (RuntimeError::RetainedState, O::Start, C::Preparation),
            (RuntimeError::Unavailable, O::Start, C::Unknown),
            (RuntimeError::Closed, O::Start, C::Unknown),
            (
                RuntimeError::Protocol(ProtocolError::Invalid),
                O::Start,
                C::GuestConnection,
            ),
            (
                RuntimeError::Image(I::Invalid("x")),
                O::Verify,
                C::Integrity,
            ),
            (RuntimeError::Image(I::Unsupported), O::Verify, C::Integrity),
            (
                RuntimeError::Image(I::Incompatible(Incompatibility::Target)),
                O::Verify,
                C::Integrity,
            ),
            (RuntimeError::Image(I::Integrity), O::Verify, C::Integrity),
            (
                RuntimeError::Image(I::LimitExceeded),
                O::Verify,
                C::Integrity,
            ),
            (RuntimeError::Image(I::UnsafePath), O::Verify, C::Integrity),
            (
                RuntimeError::Image(I::RetainedImage),
                O::Verify,
                C::Integrity,
            ),
            (
                RuntimeError::Image(I::WorkspaceUpdateRequired),
                O::Verify,
                C::WorkspaceUpdateRequired,
            ),
            (
                RuntimeError::Image(I::WorkspaceVersionUnsupported),
                O::Verify,
                C::WorkspaceVersionUnsupported,
            ),
            (RuntimeError::Image(I::NoSpace), O::Prepare, C::NoSpace),
            (RuntimeError::Image(I::Busy), O::Prepare, C::InUse),
            (
                RuntimeError::Image(I::Storage(ErrorKind::PermissionDenied)),
                O::Prepare,
                C::Permission,
            ),
            (
                RuntimeError::Image(I::Storage(ErrorKind::StorageFull)),
                O::Prepare,
                C::NoSpace,
            ),
            (
                RuntimeError::Image(I::Storage(ErrorKind::NotFound)),
                O::Prepare,
                C::Preparation,
            ),
            (RuntimeError::Image(I::Cancelled), O::Prepare, C::Unknown),
        ];
        for (error, operation, cause) in cases {
            let label = format!("{error:?}");
            assert_eq!(
                classify(error, operation),
                (operation, cause, vec![]),
                "{label}"
            );
        }
    }
    #[test]
    fn typed_origin_overrides_the_latest_progress_operation() {
        assert_eq!(
            classify(
                RuntimeError::Preflight(PreflightFailure::InsufficientMemory),
                O::Verify
            ),
            (O::Requirements, C::Unknown, vec![])
        );
        assert_eq!(
            classify(RuntimeError::Image(ImageError::Network("tls")), O::Prepare),
            (O::Download, C::Network, vec![])
        );
        assert_eq!(
            classify(RuntimeError::Infrastructure("detail".into()), O::Start),
            (O::Services, C::Services, vec![])
        );
        assert_eq!(
            classify(RuntimeError::StopUnconfirmed("detail".into()), O::Start),
            (O::Stop, C::StopUnconfirmed, vec![])
        );
        for error in [
            RuntimeError::Infrastructure("services".into()),
            RuntimeError::Image(ImageError::Network("transport")),
            RuntimeError::Startup(Box::new(StartupFailure {
                error: RuntimeError::Unavailable,
                operation: O::Start,
                diagnostic: "private".into(),
            })),
        ] {
            assert_eq!(
                classify(error, O::Stop),
                (O::Stop, C::StopUnconfirmed, vec![])
            );
        }
        // A failed stop is never reported as an ordinary cause: custody is unconfirmed.
        assert_eq!(
            classify(RuntimeError::Io(Error::other("x")), O::Stop),
            (O::Stop, C::StopUnconfirmed, vec![])
        );
    }
    #[test]
    fn startup_context_keeps_the_original_error_operation_and_os_code() {
        let denied_code = if cfg!(windows) { 5 } else { 13 };
        let wrapped = RuntimeError::Startup(Box::new(StartupFailure {
            error: RuntimeError::Io(Error::from_raw_os_error(denied_code)),
            operation: O::Start,
            diagnostic: "Details: C:\\secret\\token code: 5 NoSpace".into(),
        }));
        assert_eq!(
            wrapped.to_string(),
            "Details: C:\\secret\\token code: 5 NoSpace"
        );
        assert_eq!(
            classify(wrapped, O::Prepare),
            (O::Start, C::Permission, vec![denied_code as u32])
        );
    }
    #[test]
    fn text_never_supplies_a_category_or_code() {
        let noisy = "NoSpace StorageFull PermissionDenied code: 5 (os error 28) token=ghp_x";
        for error in [
            RuntimeError::Io(Error::other(noisy)),
            RuntimeError::Startup(Box::new(StartupFailure {
                error: RuntimeError::Io(Error::other(noisy)),
                operation: O::Start,
                diagnostic: noisy.into(),
            })),
        ] {
            assert_eq!(classify(error, O::Start), (O::Start, C::Unknown, vec![]));
        }
    }
    #[test]
    fn codes_are_unsigned_unique_and_capped_at_four() {
        let failure = WorkspaceFailure::new(O::Start, C::Unknown, [5, 5, 1, 2, 3, 4, 6]);
        assert_eq!(failure.codes, vec![5, 1, 2, 3]);
        let negative = classify(RuntimeError::Io(Error::from_raw_os_error(-1)), O::Start);
        assert_eq!(negative.2, Vec::<u32>::new());
    }
    #[test]
    fn serializes_camel_case_for_the_renderer() {
        let failure = WorkspaceFailure::new(O::Download, C::NoSpace, [112]);
        assert_eq!(
            serde_json::to_value(failure).unwrap(),
            serde_json::json!({"operation": "download", "cause": "noSpace", "codes": [112]})
        );
        let stopped = WorkspaceFailure::new(O::Stop, C::StopUnconfirmed, []);
        assert_eq!(
            serde_json::to_value(stopped).unwrap(),
            serde_json::json!({"operation": "stop", "cause": "stopUnconfirmed", "codes": []})
        );
    }
}
