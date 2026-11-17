//! Read-only host observations. No files, credentials, subprocesses or VM ownership.
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Platform {
    Windows,
    Macos,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Memory {
    Ready,
    Insufficient,
    Unknown,
}
pub use crate::runtime::types::{
    HostVirtualization as Virtualization, PreflightFailure as Failure,
};
impl Failure {
    pub fn summary(self) -> &'static str {
        match self {
            Self::VirtualizationDisabled => {
                "Virtualization is turned off or unavailable on this PC."
            }
            Self::HypervisorUnavailable => "Windows Hypervisor Platform isn't available.",
            Self::VirtualizationUnsupported => "Virtualization isn't supported on this Mac.",
            Self::InsufficientMemory => {
                "This computer has less installed memory than the local cluster needs."
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    platform: Platform,
    virtualization: Virtualization,
    memory: Memory,
    installed_memory_bytes: Option<u64>,
    required_memory_bytes: u64,
}
impl Preflight {
    pub fn new(platform: Platform, virtualization: Virtualization, installed: Option<u64>) -> Self {
        let installed = installed.filter(|bytes| *bytes > 0);
        let required = bridge_runtime_protocol::MEMORY_BYTES;
        Self {
            platform,
            virtualization,
            memory: match installed {
                Some(bytes) if bytes < required => Memory::Insufficient,
                Some(_) => Memory::Ready,
                None => Memory::Unknown,
            },
            installed_memory_bytes: installed,
            required_memory_bytes: required,
        }
    }
    pub fn failure(&self) -> Option<Failure> {
        match self.virtualization {
            Virtualization::Disabled => Some(Failure::VirtualizationDisabled),
            Virtualization::HypervisorUnavailable => Some(Failure::HypervisorUnavailable),
            Virtualization::Unsupported => Some(Failure::VirtualizationUnsupported),
            _ if self.memory == Memory::Insufficient => Some(Failure::InsufficientMemory),
            _ => None,
        }
    }
}
/// Keep all image/runtime IO and progress publication inside this boundary.
/// Unknown observations defer to real launch, not a fictional successful check.
pub fn before_image_work<T>(
    preflight: Option<Preflight>,
    work: impl FnOnce() -> T,
) -> Result<T, Failure> {
    if let Some(failure) = preflight.and_then(|p| p.failure()) {
        return Err(failure);
    }
    Ok(work())
}
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn installed_bytes(kibibytes: u64) -> Option<u64> {
    kibibytes.checked_mul(1024).filter(|bytes| *bytes > 0)
}
#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn observe_native() -> Option<Preflight> {
    use windows_sys::Win32::System::SystemInformation::GetPhysicallyInstalledSystemMemory;
    let mut kibibytes = 0;
    // SMBIOS installed RAM, not GlobalMemoryStatusEx's smaller OS-usable total.
    let memory = (unsafe { GetPhysicallyInstalledSystemMemory(&mut kibibytes) } != 0)
        .then(|| installed_bytes(kibibytes))
        .flatten();
    Some(Preflight::new(
        Platform::Windows,
        crate::runtime::windows::inputs::observe_startup_prerequisites(),
        memory,
    ))
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(crate) fn observe_native() -> Option<Preflight> {
    fn read<T: Copy + Default>(name: &std::ffi::CStr) -> Option<T> {
        let mut value = T::default();
        let mut size = std::mem::size_of::<T>();
        let result = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut T).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (result == 0 && size == std::mem::size_of::<T>()).then_some(value)
    }
    let virtualization = match read::<i32>(c"kern.hv_support") {
        Some(1) => Virtualization::Ready,
        Some(0) => Virtualization::Unsupported,
        _ => Virtualization::Unknown,
    };
    Some(Preflight::new(
        Platform::Macos,
        virtualization,
        read::<u64>(c"hw.memsize"),
    ))
}
#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
)))]
pub(crate) fn observe_native() -> Option<Preflight> {
    None
}

/// The public setup command stays unavailable in unactivated release builds.
/// The native factory uses its private observation only after selecting a source.
pub fn observe() -> Option<Preflight> {
    #[cfg(debug_assertions)]
    return observe_native();
    #[cfg(not(debug_assertions))]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_memory_boundary_and_unknowns_do_not_invent_a_reserve() {
        for (bytes, expected) in [
            (Some(6442450943), Memory::Insufficient),
            (Some(6442450944), Memory::Ready),
            (Some(8589934592), Memory::Ready),
            (Some(0), Memory::Unknown),
            (None, Memory::Unknown),
        ] {
            let p = Preflight::new(Platform::Windows, Virtualization::Unknown, bytes);
            assert_eq!(p.memory, expected);
            assert_eq!(
                p.failure(),
                (expected == Memory::Insufficient).then_some(Failure::InsufficientMemory)
            );
        }
    }

    #[test]
    fn confirmed_failures_never_enter_image_work() {
        for (virtualization, bytes, expected) in [
            (
                Virtualization::Disabled,
                None,
                Failure::VirtualizationDisabled,
            ),
            (
                Virtualization::HypervisorUnavailable,
                None,
                Failure::HypervisorUnavailable,
            ),
            (
                Virtualization::Unsupported,
                None,
                Failure::VirtualizationUnsupported,
            ),
            (Virtualization::Ready, Some(1), Failure::InsufficientMemory),
        ] {
            let p = Preflight::new(Platform::Windows, virtualization, bytes);
            let result = before_image_work(Some(p), || panic!("image work or progress entered"));
            assert_eq!(result, Err(expected));
        }
        for p in [
            None,
            Some(Preflight::new(
                Platform::Macos,
                Virtualization::Unknown,
                None,
            )),
            Some(Preflight::new(
                Platform::Windows,
                Virtualization::Ready,
                Some(6442450944),
            )),
        ] {
            assert_eq!(before_image_work(p, || 42), Ok(42));
        }
    }

    #[test]
    fn observations_serialize_only_finite_public_fields() {
        let p = Preflight::new(
            Platform::Macos,
            Virtualization::Unsupported,
            Some(8589934592),
        );
        assert_eq!(
            serde_json::to_value(p).unwrap(),
            serde_json::json!({
                "platform": "macos", "virtualization": "unsupported", "memory": "ready",
                "installedMemoryBytes": 8589934592u64, "requiredMemoryBytes": 6442450944u64
            })
        );
        assert_eq!(
            serde_json::to_value(Failure::InsufficientMemory).unwrap(),
            "insufficientMemory"
        );
    }

    #[test]
    fn kibibytes_cannot_overflow_or_turn_a_failed_read_into_zero_ram() {
        assert_eq!(installed_bytes(0), None);
        assert_eq!(installed_bytes(u64::MAX), None);
        assert_eq!(installed_bytes(6291456), Some(6442450944));
    }

    #[test]
    fn native_and_public_observations_stay_separate() {
        #[cfg(not(any(
            all(windows, target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        )))]
        assert!(observe_native().is_none());
        #[cfg(any(
            all(windows, target_arch = "x86_64"),
            all(target_os = "macos", target_arch = "aarch64")
        ))]
        assert!(observe_native().is_some());
    }

    #[test]
    fn unavailable_builds_do_not_report_readiness() {
        #[cfg(not(all(
            debug_assertions,
            any(
                all(windows, target_arch = "x86_64"),
                all(target_os = "macos", target_arch = "aarch64")
            )
        )))]
        assert!(observe().is_none());
    }
}
