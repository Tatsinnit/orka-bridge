use crate::runtime::types::RuntimeError;
use bridge_runtime_protocol::Uuid;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilePin {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootAsset {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub format: String,
    pub compression: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RootSelector {
    pub kind: String,
    pub value: Uuid,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Driver {
    pub name: String,
    pub built_in: bool,
}
/// A projection of publisher-validated metadata, not a second publisher schema.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectBoot {
    pub kernel: BootAsset,
    pub initrd: BootAsset,
    pub root_selector: RootSelector,
    pub command_line: String,
    pub kernel_release: String,
    pub required_drivers: Vec<Driver>,
    pub provenance: BTreeMap<String, String>,
}
/// Supplied by trusted host selection after independent archive/receipt authentication.
/// Never deserialize a downloaded manifest and treat its own digests as this authority.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewedWindowsPin {
    pub runtime_files: Vec<FilePin>,
    pub image: FilePin,
    pub endpoint_sha256: String,
    pub direct_boot: DirectBoot,
}
/// Trusted host-owned stable trees; file sharing does not pin ancestor directories.
/// Hostile same-user junction/rename races require a separate filesystem sandbox.
#[derive(Clone)]
pub struct WindowsInputPaths {
    pub runtime: PathBuf,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub factory: PathBuf,
}
pub struct VerifiedWindowsInputs {
    pub(super) paths: WindowsInputPaths,
    inventory: Vec<FilePin>,
    _held: Vec<File>,
}
pub(super) fn invalid() -> RuntimeError {
    io::Error::new(io::ErrorKind::InvalidData, "invalid Windows runtime inputs").into()
}
fn hash_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn relative(s: &str) -> bool {
    !s.is_empty()
        && !s.contains(['\\', ':', '\0'])
        && Path::new(s)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && s.split('/')
            .all(|c| !c.is_empty() && !c.ends_with(['.', ' ']) && !c.chars().any(char::is_control))
}
fn regular(path: &Path) -> Result<(), RuntimeError> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err(invalid());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(invalid());
        }
    }
    Ok(())
}
fn checked(path: &Path, bytes: u64, sha: &str) -> Result<File, RuntimeError> {
    if bytes == 0 || !hash_valid(sha) || !path.is_absolute() {
        return Err(invalid());
    }
    regular(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    // Retain read sharing only, including across child startup and DLL loading.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let mut file = options.open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() != bytes {
        return Err(invalid());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    if format!("{:x}", digest.finalize()) != sha {
        return Err(invalid());
    }
    Ok(file)
}
fn inventory(root: &Path) -> Result<BTreeSet<String>, RuntimeError> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) -> Result<(), RuntimeError> {
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(invalid());
            }
        }
        for entry in fs::read_dir(dir)? {
            let p = entry?.path();
            let m = fs::symlink_metadata(&p)?;
            if m.is_dir() {
                walk(base, &p, out)?
            } else {
                regular(&p)?;
                let name = p
                    .strip_prefix(base)
                    .map_err(|_| invalid())?
                    .to_str()
                    .ok_or_else(invalid)?
                    .replace('\\', "/");
                // The independent expected inventory, not this manifest, is authority here.
                if name != "runtime-manifest.json" {
                    out.insert(name);
                }
            }
        }
        Ok(())
    }
    let mut found = BTreeSet::new();
    walk(root, root, &mut found)?;
    Ok(found)
}
impl VerifiedWindowsInputs {
    pub fn executable(&self, name: &str) -> Result<PathBuf, RuntimeError> {
        if !["qemu-system-x86_64.exe", "qemu-img.exe"].contains(&name) {
            return Err(invalid());
        }
        Ok(self.paths.runtime.join("bin").join(name))
    }
    /// Call immediately before each launch, not merely at input construction.
    pub fn recheck_runtime(&self) -> Result<(), RuntimeError> {
        self.check_runtime().map(|_| ())
    }
    fn check_runtime(&self) -> Result<Vec<File>, RuntimeError> {
        let names: BTreeSet<_> = self.inventory.iter().map(|r| r.name.clone()).collect();
        if names != inventory(&self.paths.runtime)? {
            return Err(invalid());
        }
        self.inventory
            .iter()
            .map(|r| checked(&self.paths.runtime.join(&r.name), r.bytes, &r.sha256))
            .collect()
    }
}
pub fn verify_windows_inputs(
    pin: &ReviewedWindowsPin,
    paths: WindowsInputPaths,
) -> Result<VerifiedWindowsInputs, RuntimeError> {
    let b = &pin.direct_boot;
    let command = format!(
        "root=UUID={} ro console=ttyS0,115200n8 noresume",
        b.root_selector.value
    );
    let drivers: BTreeSet<_> = b.required_drivers.iter().map(|d| d.name.as_str()).collect();
    let wanted = BTreeSet::from([
        "virtio_pci",
        "virtio_blk",
        "virtio_console",
        "virtio_rng",
        "virtio_net",
        "ext4",
    ]);
    if b.kernel.format != "linux-bzimage"
        || b.initrd.format != "initramfs"
        || !matches!(
            b.initrd.compression.as_deref(),
            Some("zstd" | "gzip" | "xz" | "cpio-prefix")
        )
        || b.root_selector.kind != "filesystem-uuid"
        || b.root_selector.value.is_nil()
        || b.command_line != command
        || !b.kernel_release.ends_with("-amd64")
        || drivers != wanted
        || b.required_drivers.len() != wanted.len()
        || !relative(&b.kernel.name)
        || !relative(&b.initrd.name)
    {
        return Err(invalid());
    }
    for key in [
        "diskSha256",
        "bootReceiptSha256",
        "buildReceiptSha256",
        "inputSha256",
        "kernelConfigSha256",
        "modulesSha256",
        "endpointSha256",
        "earlyHookSha256",
        "buildHookSha256",
        "extractorSha256",
        "inspectorSha256",
    ] {
        if !b.provenance.get(key).is_some_and(|s| hash_valid(s)) {
            return Err(invalid());
        }
    }
    if b.provenance.get("diskSha256") != Some(&pin.image.sha256)
        || b.provenance.get("endpointSha256") != Some(&pin.endpoint_sha256)
    {
        return Err(invalid());
    }
    let held = vec![
        checked(&paths.kernel, b.kernel.bytes, &b.kernel.sha256)?,
        checked(&paths.initrd, b.initrd.bytes, &b.initrd.sha256)?,
        checked(&paths.factory, pin.image.bytes, &pin.image.sha256)?,
    ];
    let mut inputs = verify_windows_runtime(&pin.runtime_files, paths.runtime.clone())?;
    inputs.paths = paths;
    inputs._held.extend(held);
    Ok(inputs)
}
/// Runtime archive authority is independent of the separately installed image.
pub fn verify_windows_runtime(
    files: &[FilePin],
    runtime: PathBuf,
) -> Result<VerifiedWindowsInputs, RuntimeError> {
    let mut names = BTreeSet::new();
    for row in files {
        if !relative(&row.name) || !names.insert(row.name.to_lowercase()) {
            return Err(invalid());
        }
        if row.name.to_lowercase().ends_with(".dll")
            && (row.name.split('/').count() != 2 || !row.name.starts_with("bin/"))
        {
            return Err(invalid());
        }
    }
    if !["bin/qemu-system-x86_64.exe", "bin/qemu-img.exe"]
        .iter()
        .all(|n| names.contains(*n))
    {
        return Err(invalid());
    }
    if !runtime.is_absolute() {
        return Err(invalid());
    }
    let mut inputs = VerifiedWindowsInputs {
        paths: WindowsInputPaths {
            runtime,
            kernel: PathBuf::new(),
            initrd: PathBuf::new(),
            factory: PathBuf::new(),
        },
        inventory: files.to_vec(),
        _held: Vec::new(),
    };
    inputs._held.extend(inputs.check_runtime()?);
    Ok(inputs)
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Observation {
    Yes,
    No,
    Unknown,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Usability {
    NotTested,
    Usable,
    Unusable,
}
#[derive(Clone, Copy, Debug)]
pub struct Prerequisites {
    pub feature: Observation,
    pub reboot_pending: Observation,
    pub hypervisor: Observation,
    pub usability: Usability,
}
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn startup_prerequisites(
    hypervisor: Observation,
    firmware: Observation,
    feature: Observation,
) -> super::super::types::HostVirtualization {
    use super::super::types::HostVirtualization as Virtualization;
    use Observation::*;
    match (hypervisor, firmware, feature) {
        (Yes, _, _) => Virtualization::Ready, // A running hypervisor may hide the firmware bit.
        (No, No, _) => Virtualization::Disabled,
        (No, Yes, _) | (Unknown, Yes, No) => Virtualization::HypervisorUnavailable,
        _ => Virtualization::Unknown, // Defer to real launch without claiming readiness.
    }
}
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
type GetCapability = unsafe extern "system" fn(u32, *mut std::ffi::c_void, u32, *mut u32) -> i32;
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn observe_hypervisor(get: GetCapability) -> Observation {
    // WHvGetCapability requires room for a 64-bit capability, even though
    // HypervisorPresent (code 0) is a 32-bit Win32 BOOL at the start of it.
    let (mut present, mut written) = (0u64, 0u32);
    let result = unsafe { get(0, (&mut present as *mut u64).cast(), 8, &mut written) };
    if result >= 0 && (4..=8).contains(&written) {
        if present as u32 != 0 {
            Observation::Yes
        } else {
            Observation::No
        }
    } else {
        Observation::Unknown
    }
}
/// Observe WHP itself; feature flags alone do not prove a running hypervisor.
#[cfg(all(windows, target_arch = "x86_64"))]
pub(crate) fn observe_startup_prerequisites() -> super::super::types::HostVirtualization {
    use std::ffi::c_void;
    use windows_sys::Win32::{
        Foundation::{FreeLibrary, GetLastError},
        System::Threading::{IsProcessorFeaturePresent, PF_VIRT_FIRMWARE_ENABLED},
    };
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
        fn GetProcAddress(
            module: *mut c_void,
            name: *const u8,
        ) -> Option<unsafe extern "system" fn() -> isize>;
    }
    let name: Vec<_> = "WinHvPlatform.dll".encode_utf16().chain(Some(0)).collect();
    // LOAD_LIBRARY_SEARCH_SYSTEM32 prevents loading a renderer/workspace DLL.
    let module = unsafe { LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), 0x800) };
    let (hypervisor, feature) = if module.is_null() {
        (
            Observation::Unknown,
            if unsafe { GetLastError() } == 126 {
                Observation::No
            } else {
                Observation::Unknown
            },
        )
    } else {
        let address = unsafe { GetProcAddress(module, c"WHvGetCapability".as_ptr().cast()) };
        let observed = match address {
            None => (Observation::Unknown, Observation::No),
            Some(address) => {
                let get: GetCapability = unsafe { std::mem::transmute(address) };
                (observe_hypervisor(get), Observation::Yes)
            }
        };
        unsafe {
            FreeLibrary(module);
        }
        observed
    };
    let firmware = if unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED) } != 0 {
        Observation::Yes
    } else {
        Observation::No
    };
    startup_prerequisites(hypervisor, firmware, feature)
}
impl Prerequisites {
    pub fn usable(&self) -> bool {
        self.usability == Usability::Usable && self.reboot_pending != Observation::Yes
    }
}
#[cfg(test)]
mod prerequisite_tests {
    use super::*;
    #[test]
    fn capability_query_uses_the_documented_64_bit_output_buffer() {
        unsafe extern "system" fn capability(
            code: u32,
            output: *mut std::ffi::c_void,
            bytes: u32,
            written: *mut u32,
        ) -> i32 {
            if code != 0 || bytes != 8 {
                return 0x8007007au32 as i32;
            }
            unsafe {
                *output.cast::<u32>() = 1;
                *written = 4;
            }
            0
        }
        assert_eq!(observe_hypervisor(capability), Observation::Yes);
    }
    #[test]
    fn startup_diagnosis_never_calls_running_hypervisor_firmware_disabled() {
        use super::super::super::types::HostVirtualization as V;
        use Observation::*;
        for (hypervisor, firmware, feature, expected) in [
            (Yes, No, Yes, V::Ready),
            (Yes, Unknown, Yes, V::Ready),
            (No, No, Yes, V::Disabled),
            (No, Yes, Yes, V::HypervisorUnavailable),
            (Unknown, Yes, No, V::HypervisorUnavailable),
            (Unknown, No, No, V::Unknown),
            (Unknown, Yes, Yes, V::Unknown),
            (No, Unknown, Yes, V::Unknown),
        ] {
            assert_eq!(
                startup_prerequisites(hypervisor, firmware, feature),
                expected,
                "{hypervisor:?} / {firmware:?} / {feature:?}"
            );
        }
    }
}
