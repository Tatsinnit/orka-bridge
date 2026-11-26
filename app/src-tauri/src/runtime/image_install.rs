//! Private image copies and their commit selector, independent of lifecycle authority.
use super::{
    identity::{file_identity, FileIdentity},
    image_cache::{
        self, available_space, directory, read_directory, regular, single_link, storage,
    },
    image_types::*,
    manifest::Unique,
};
use bridge_runtime_protocol::Uuid;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::{
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
};
use tokio::sync::{mpsc, watch};

const MAX_BINDING: u64 = 256 * 1024;
const FILES: [&str; 3] = ["os.disk", "os.kernel", "os.initrd"];
#[cfg(test)]
type Point = Arc<dyn Fn(&str) -> Result<(), ImageError> + Send + Sync>;

/// The lifecycle owner supplies a held root and lock and asserts current live creation permission.
/// This does not reconstruct permission from a pathname, file ID or persisted record.
pub struct InstallationImageTarget<'owner> {
    root: &'owner Dir,
    owner: &'owner Arc<File>,
    id: Uuid,
    target: ImageTarget,
    compatibility: &'owner str,
    creation_allowed: bool,
    #[cfg(test)]
    point: Option<Point>,
    #[cfg(test)]
    available: Option<u64>,
}
impl<'owner> InstallationImageTarget<'owner> {
    pub(crate) fn from_owned_installation(
        root: &'owner Dir,
        owner: &'owner Arc<File>,
        id: Uuid,
        target: ImageTarget,
        compatibility: &'owner str,
        creation_allowed: bool,
    ) -> Self {
        Self {
            root,
            owner,
            id,
            target,
            compatibility,
            creation_allowed,
            #[cfg(test)]
            point: None,
            #[cfg(test)]
            available: None,
        }
    }
}
macro_rules! record {
    ($name:ident { $($field:ident:$ty:ty),* $(,)? }) => {
        #[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
        #[serde(rename_all="camelCase",deny_unknown_fields)]
        struct $name { $($field:$ty),* }
    };
}
record!(Identity {
    manifest_sha256: String,
    image_id: String,
    state_compatibility_id: String,
    build_source_commit: String
});
record!(Intent {
    version: u16,
    installation_id: Uuid,
    image_generation: u64,
    identity: Identity,
    target: String
});
record!(RootSelector {
    kind: String,
    value: String
});
record!(Kernel {
    name: String,
    bytes: u64,
    sha256: String,
    format: String
});
record!(Initrd {
    name: String,
    bytes: u64,
    sha256: String,
    format: String,
    compression: String
});
record!(Driver {
    name: String,
    built_in: bool
});
record!(Tool {
    name: String,
    version: String,
    executable_sha256: String
});
record!(Provenance {disk_sha256:String,boot_receipt_sha256:String,build_receipt_sha256:String,input_sha256:String,kernel_config_sha256:String,modules_sha256:String,endpoint_sha256:String,early_hook_sha256:String,build_hook_sha256:String,extractor_sha256:String,inspector_sha256:String,extraction_tools:Vec<Tool>});
record!(Boot {kernel:Kernel,initrd:Initrd,root_selector:RootSelector,kernel_release:String,command_line:String,required_drivers:Vec<Driver>,provenance:Provenance});
record!(Payload {
    path: String,
    identity: FileIdentity,
    initial_bytes: u64,
    initial_sha256: String,
    allocated_bytes: u64
});
record!(Binding {
    version: u16,
    image_generation: u64,
    installation_id: Uuid,
    installation_directory: FileIdentity,
    image_directory: FileIdentity,
    identity: Identity,
    target: String,
    launch: String,
    os_format: String,
    boot: Boot,
    os: Payload,
    kernel: Payload,
    initrd: Payload
});

/// Validated metadata only: clones retain neither installation locks nor mutable OS handles.
#[derive(Clone, Debug)]
pub struct InstalledImageSet {
    binding: Binding,
    identity: ImageIdentity,
}
impl InstalledImageSet {
    /// Binding self-consistency does not authenticate its claims. Reuse the
    /// app-selected manifest for immutable boot facts, not the writable OS bytes.
    #[cfg_attr(test, allow(dead_code))] // Some protocol harnesses omit the native caller.
    pub(crate) fn validate_pinned(&self, image: &ResolvedImage) -> Result<(), ImageError> {
        let manifest: serde_json::Value =
            serde_json::from_slice(image.manifest_bytes()).map_err(|_| ImageError::Integrity)?;
        let boot: Boot = serde_json::from_value(manifest["directBoot"].clone())
            .map_err(|_| ImageError::Integrity)?;
        let identity = image.identity();
        if self.identity.manifest_sha256 != identity.manifest_sha256
            || self.identity.image_id != identity.image_id
            || self.identity.state_compatibility_id != identity.state_compatibility_id
            || self.identity.build_source_commit != identity.build_source_commit
            || self.target() != image.target()
            || self.launch_mode() != image.launch_mode()
            || self.binding.boot != boot
            || self.binding.os.initial_bytes != image.disk_bytes()
            || self.binding.os.initial_sha256 != image.disk_sha256()
        {
            return Err(ImageError::Integrity);
        }
        Ok(())
    }
    pub fn installation_id(&self) -> Uuid {
        self.binding.installation_id
    }
    pub fn image_generation(&self) -> u64 {
        self.binding.image_generation
    }
    pub fn identity(&self) -> &ImageIdentity {
        &self.identity
    }
    pub fn target(&self) -> ImageTarget {
        if self.binding.target == "macos-arm64" {
            ImageTarget::MacosArm64
        } else {
            ImageTarget::WindowsAmd64
        }
    }
    pub fn launch_mode(&self) -> LaunchMode {
        if self.binding.launch == "uefi" {
            LaunchMode::Uefi
        } else {
            LaunchMode::LinuxDirect
        }
    }
    pub fn command_line(&self) -> &str {
        &self.binding.boot.command_line
    }
    pub fn root_selector(&self) -> &str {
        &self.binding.boot.root_selector.value
    }
    pub fn kernel_release(&self) -> &str {
        &self.binding.boot.kernel_release
    }
    pub fn disk_path(&self) -> &Path {
        Path::new("image/1/os.disk")
    }
    pub fn kernel_path(&self) -> &Path {
        Path::new("image/1/os.kernel")
    }
    pub fn initrd_path(&self) -> &Path {
        Path::new("image/1/os.initrd")
    }
    pub fn os_identity(&self) -> &FileIdentity {
        &self.binding.os.identity
    }
    #[cfg(windows)]
    pub(crate) fn kernel_pin(&self) -> (u64, &str) {
        (
            self.binding.kernel.initial_bytes,
            &self.binding.kernel.initial_sha256,
        )
    }
    #[cfg(windows)]
    pub(crate) fn initrd_pin(&self) -> (u64, &str) {
        (
            self.binding.initrd.initial_bytes,
            &self.binding.initrd.initial_sha256,
        )
    }
    pub fn kernel_identity(&self) -> &FileIdentity {
        &self.binding.kernel.identity
    }
    pub fn initrd_identity(&self) -> &FileIdentity {
        &self.binding.initrd.identity
    }
    pub fn os_format(&self) -> &str {
        &self.binding.os_format
    }
    pub fn kernel_format(&self) -> &str {
        &self.binding.boot.kernel.format
    }
    pub fn initrd_compression(&self) -> &str {
        &self.binding.boot.initrd.compression
    }
    pub fn endpoint_sha256(&self) -> &str {
        &self.binding.boot.provenance.endpoint_sha256
    }
    pub fn initial_os_sha256(&self) -> &str {
        &self.binding.os.initial_sha256
    }
    pub fn initial_os_bytes(&self) -> u64 {
        self.binding.os.initial_bytes
    }
    pub fn initial_os_allocated_bytes(&self) -> u64 {
        self.binding.os.allocated_bytes
    }
    /// Must be called under current lifecycle ownership before a backend action.
    pub fn revalidate(&self, target: &InstallationImageTarget<'_>) -> Result<(), ImageError> {
        let current = open_installed_image(target)?;
        if current.binding != self.binding {
            return Err(ImageError::RetainedImage);
        }
        Ok(())
    }
}
fn fid(file: &File) -> Result<FileIdentity, ImageError> {
    file_identity(file).map_err(|e| match e {
        super::types::RuntimeError::Io(e) => storage(e),
        _ => ImageError::Integrity,
    })
}
fn dir_id(dir: &Dir) -> Result<FileIdentity, ImageError> {
    fid(&dir.try_clone().map_err(storage)?.into_std_file())
}
fn target_name(target: ImageTarget) -> &'static str {
    match target {
        ImageTarget::MacosArm64 => "macos-arm64",
        ImageTarget::WindowsAmd64 => "windows-amd64",
    }
}
fn read_file(dir: &Dir, name: &str) -> Result<File, ImageError> {
    let file = regular(dir, name, false, false)?;
    single_link(&file)?;
    Ok(file)
}
fn json<T: serde::de::DeserializeOwned>(dir: &Dir, name: &str) -> Result<T, ImageError> {
    let mut bytes = Vec::new();
    read_file(dir, name)
        .map_err(|e| {
            if missing(&e) {
                ImageError::RetainedImage
            } else {
                e
            }
        })?
        .take(MAX_BINDING + 1)
        .read_to_end(&mut bytes)
        .map_err(storage)?;
    if bytes.len() as u64 > MAX_BINDING {
        return Err(ImageError::RetainedImage);
    }
    let Unique(value) = serde_json::from_slice(&bytes).map_err(|_| ImageError::RetainedImage)?;
    serde_json::from_value(value).map_err(|_| ImageError::RetainedImage)
}
fn missing(error: &ImageError) -> bool {
    matches!(error, ImageError::Storage(std::io::ErrorKind::NotFound))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn owner_path_identity(root: &Dir) -> Result<FileIdentity, ImageError> {
    #[cfg(windows)]
    {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
        use cap_std::fs::{OpenOptions, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
        // The supplied capability already holds the original deny-sharing lock.
        // Query the anchored namespace identity, never reopen that lock for data.
        let mut options = OpenOptions::new();
        options
            .read(true)
            .access_mode(FILE_READ_ATTRIBUTES)
            .follow(FollowSymlinks::No);
        let file = root
            .open_with("owner.lock", &options)
            .map_err(storage)?
            .into_std();
        image_cache::not_reparse(&file)?;
        if !file.metadata().map_err(storage)?.is_file() {
            return Err(ImageError::UnsafePath);
        }
        single_link(&file)?;
        fid(&file)
    }
    #[cfg(not(windows))]
    {
        fid(&read_file(root, "owner.lock")?)
    }
}
fn validate_target(root: &Dir, owner: &File, id: Uuid) -> Result<(), ImageError> {
    if id.is_nil() || owner_path_identity(root)? != fid(owner)? {
        return Err(ImageError::RetainedImage);
    }
    image_cache::not_reparse(&root.try_clone().map_err(storage)?.into_std_file())
}
fn valid_facts(binding: &Binding) -> bool {
    let id = |s: &str| super::manifest::name(s, 63, false);
    let source = &binding.identity.build_source_commit;
    let boot = &binding.boot;
    let provenance = &boot.provenance;
    let hashes = [
        &provenance.disk_sha256,
        &provenance.boot_receipt_sha256,
        &provenance.build_receipt_sha256,
        &provenance.input_sha256,
        &provenance.kernel_config_sha256,
        &provenance.modules_sha256,
        &provenance.endpoint_sha256,
        &provenance.early_hook_sha256,
        &provenance.build_hook_sha256,
        &provenance.extractor_sha256,
        &provenance.inspector_sha256,
    ];
    let drivers: std::collections::BTreeSet<_> = boot
        .required_drivers
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    let tools: std::collections::BTreeSet<_> = provenance
        .extraction_tools
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    let suffix = if binding.target == "macos-arm64" {
        "-arm64"
    } else {
        "-amd64"
    };
    id(&binding.identity.image_id)
        && !binding.identity.image_id.contains("..")
        && id(&binding.identity.state_compatibility_id)
        && source.len() == 40
        && source
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && hashes.into_iter().all(|s| valid_hash(s))
        && boot.kernel_release.strip_suffix(suffix).is_some_and(|s| {
            !s.is_empty()
                && s.len() <= 96
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        })
        && boot.required_drivers.len() == 6
        && drivers
            == [
                "ext4",
                "virtio_blk",
                "virtio_console",
                "virtio_net",
                "virtio_pci",
                "virtio_rng",
            ]
            .into_iter()
            .collect()
        && provenance.extraction_tools.len() == 4
        && tools
            == ["dpkg-query", "guestfish", "readelf", "unmkinitramfs"]
                .into_iter()
                .collect()
        && provenance
            .extraction_tools
            .iter()
            .all(|t| valid_hash(&t.executable_sha256) && super::manifest::tool_version(&t.version))
}
fn validate_binding(
    binding: &Binding,
    target: &InstallationImageTarget<'_>,
) -> Result<(), ImageError> {
    if binding.version != 1
        || binding.image_generation != 1
        || binding.installation_id != target.id
        || binding.target != target_name(target.target)
        || binding.installation_directory != dir_id(target.root)?
        || !valid_hash(&binding.identity.manifest_sha256)
        || binding.identity.image_id.is_empty()
        || !valid_facts(binding)
        || binding
            .os
            .initial_bytes
            .checked_add(binding.kernel.initial_bytes)
            .and_then(|n| n.checked_add(binding.initrd.initial_bytes))
            .is_none()
    {
        return Err(ImageError::RetainedImage);
    }
    let arm = target.target == ImageTarget::MacosArm64;
    if binding.launch != if arm { "uefi" } else { "linux-direct" }
        || binding.os_format != if arm { "raw" } else { "qcow2" }
        || binding.boot.kernel.format
            != if arm {
                "linux-arm64-image"
            } else {
                "linux-bzimage"
            }
        || binding.boot.initrd.format != "initramfs"
        || !matches!(
            binding.boot.initrd.compression.as_str(),
            "gzip" | "xz" | "zstd" | "cpio-prefix"
        )
        || binding.boot.root_selector.kind != "filesystem-uuid"
        || Uuid::parse_str(&binding.boot.root_selector.value).is_err()
        || binding.boot.command_line
            != format!(
                "root=UUID={} ro console={} noresume",
                binding.boot.root_selector.value,
                if arm { "hvc0" } else { "ttyS0,115200n8" }
            )
    {
        return Err(ImageError::RetainedImage);
    }
    for (file, name) in [
        (&binding.os, FILES[0]),
        (&binding.kernel, FILES[1]),
        (&binding.initrd, FILES[2]),
    ] {
        if file.path != format!("image/1/{name}")
            || file.initial_bytes == 0
            || !valid_hash(&file.initial_sha256)
        {
            return Err(ImageError::RetainedImage);
        }
    }
    if binding.kernel.initial_sha256 != binding.boot.kernel.sha256
        || binding.kernel.initial_bytes != binding.boot.kernel.bytes
        || binding.initrd.initial_sha256 != binding.boot.initrd.sha256
        || binding.initrd.initial_bytes != binding.boot.initrd.bytes
        || binding.os.initial_sha256 != binding.boot.provenance.disk_sha256
    {
        return Err(ImageError::RetainedImage);
    }
    Ok(())
}
fn digest(
    file: &mut File,
    bytes: u64,
    sha: &str,
    stop: impl Fn() -> Result<(), ImageError>,
) -> Result<(), ImageError> {
    let mut actual = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        stop()?;
        let n = file.read(&mut buffer).map_err(storage)?;
        if n == 0 {
            break;
        }
        actual = actual
            .checked_add(n as u64)
            .filter(|v| *v <= bytes)
            .ok_or(ImageError::Integrity)?;
        hash.update(&buffer[..n]);
    }
    if actual != bytes || format!("{:x}", hash.finalize()) != sha {
        return Err(ImageError::Integrity);
    }
    Ok(())
}
fn verify_files(
    root: &Dir,
    binding: &Binding,
    initial_os: bool,
    stop: impl Fn() -> Result<(), ImageError>,
) -> Result<(), ImageError> {
    let images = read_directory(root, "image")?.ok_or(ImageError::RetainedImage)?;
    let generation = read_directory(&images, "1")?.ok_or(ImageError::RetainedImage)?;
    if dir_id(&generation)? != binding.image_directory {
        return Err(ImageError::Integrity);
    }
    for (record, name) in [
        (&binding.os, FILES[0]),
        (&binding.kernel, FILES[1]),
        (&binding.initrd, FILES[2]),
    ] {
        let mut file = read_file(&generation, name)?;
        if fid(&file)? != record.identity {
            return Err(ImageError::Integrity);
        }
        if initial_os || name != FILES[0] {
            digest(
                &mut file,
                record.initial_bytes,
                &record.initial_sha256,
                &stop,
            )?;
        }
    }
    Ok(())
}
/// Reopens only a committed binding; initial mutable OS bytes are retained as provenance.
pub fn open_installed_image(
    target: &InstallationImageTarget<'_>,
) -> Result<InstalledImageSet, ImageError> {
    let image = read_installed_image(target)?.ok_or(ImageError::RetainedImage)?;
    if image.identity().state_compatibility_id != target.compatibility {
        return Err(ImageError::RetainedImage);
    }
    Ok(image)
}
/// Owned structural inspection is not permission to boot a different image.
#[cfg_attr(test, allow(dead_code))] // Some actual-source harnesses omit the native caller.
pub(crate) fn inspect_installed_image(
    target: &InstallationImageTarget<'_>,
) -> Result<InstalledImageSet, ImageError> {
    // Only absence can mean an unfinished new install. A committed malformed
    // binding must not fall through to acquisition as RetainedImage does.
    read_installed_image(target)
        .map_err(|error| match error {
            ImageError::RetainedImage => ImageError::Integrity,
            other => other,
        })?
        .ok_or(ImageError::RetainedImage)
}
fn read_installed_image(
    target: &InstallationImageTarget<'_>,
) -> Result<Option<InstalledImageSet>, ImageError> {
    validate_target(target.root, target.owner, target.id)?;
    match target.root.symlink_metadata("image-binding.json") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage(error)),
        Ok(_) => {}
    }
    let binding: Binding = json(target.root, "image-binding.json")?;
    validate_binding(&binding, target)?;
    verify_files(target.root, &binding, false, || Ok(()))?;
    sync_namespace(target, &binding)?;
    Ok(Some(installed(binding)))
}
fn sync_namespace(
    target: &InstallationImageTarget<'_>,
    binding: &Binding,
) -> Result<(), ImageError> {
    #[cfg(test)]
    if let Some(point) = &target.point {
        point("namespace-flush")?;
    }
    let images = read_directory(target.root, "image")?.ok_or(ImageError::RetainedImage)?;
    let generation = read_directory(&images, "1")?.ok_or(ImageError::RetainedImage)?;
    if dir_id(&generation)? != binding.image_directory {
        return Err(ImageError::Integrity);
    }
    image_cache::fs::sync(&generation)?;
    image_cache::fs::sync(&images)?;
    image_cache::fs::sync(target.root)
}
fn installed(binding: Binding) -> InstalledImageSet {
    let identity = ImageIdentity {
        manifest_sha256: binding.identity.manifest_sha256.clone(),
        image_id: binding.identity.image_id.clone(),
        state_compatibility_id: binding.identity.state_compatibility_id.clone(),
        build_source_commit: binding.identity.build_source_commit.clone(),
    };
    InstalledImageSet { binding, identity }
}
/// Authenticated factory read files, prepared before installation ownership.
pub struct PreparedImage {
    identity: ImageIdentity,
    target: ImageTarget,
    disk_bytes: u64,
    disk_sha256: String,
    boot: Boot,
    files: [File; 3],
}
impl PreparedImage {
    #[cfg(debug_assertions)]
    pub(crate) fn from_development(
        image: crate::dev_image::VerifiedDevImage,
    ) -> Result<Self, ImageError> {
        use crate::dev_image::DevImagePlatform;
        use std::io::Seek;
        let (receipt_bytes, receipt_sha) = image.declared(0);
        let mut file = image.manifest().try_clone().map_err(storage)?.into_std();
        file.rewind().map_err(storage)?;
        let mut bytes = Vec::new();
        file.take((receipt_bytes + 1).min(MAX_BINDING + 1))
            .read_to_end(&mut bytes)
            .map_err(storage)?;
        if bytes.len() as u64 != receipt_bytes
            || format!("{:x}", Sha256::digest(&bytes)) != receipt_sha
        {
            return Err(ImageError::Integrity);
        }
        // Project authenticated engineering facts, never fabricate a release manifest.
        let receipt: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| ImageError::Integrity)?;
        let mut direct = serde_json::Map::new();
        for key in [
            "kernel",
            "initrd",
            "rootSelector",
            "kernelRelease",
            "commandLine",
            "requiredDrivers",
        ] {
            direct.insert(key.into(), receipt[key].clone());
        }
        let mut provenance = serde_json::Map::new();
        for key in [
            "diskSha256",
            "buildReceiptSha256",
            "inputSha256",
            "kernelConfigSha256",
            "modulesSha256",
            "endpointSha256",
            "earlyHookSha256",
            "buildHookSha256",
            "extractorSha256",
            "inspectorSha256",
            "extractionTools",
        ] {
            provenance.insert(key.into(), receipt[key].clone());
        }
        provenance.insert("bootReceiptSha256".into(), receipt_sha.clone().into());
        direct.insert("provenance".into(), provenance.into());
        let boot: Boot =
            serde_json::from_value(direct.into()).map_err(|_| ImageError::Integrity)?;
        for (i, bytes, sha) in [
            (1, boot.kernel.bytes, &boot.kernel.sha256),
            (2, boot.initrd.bytes, &boot.initrd.sha256),
        ] {
            if image.declared(i) != (bytes, sha.clone()) {
                return Err(ImageError::Integrity);
            }
        }
        let (disk_bytes, disk_sha256) = image.declared(3);
        if boot.provenance.disk_sha256 != disk_sha256 {
            return Err(ImageError::Integrity);
        }
        let target = match image.platform() {
            DevImagePlatform::MacOsArm64 => ImageTarget::MacosArm64,
            DevImagePlatform::WindowsAmd64 => ImageTarget::WindowsAmd64,
        };
        Ok(Self {
            identity: ImageIdentity {
                // In engineering sets this common identity slot hashes the raw boot receipt.
                manifest_sha256: receipt_sha,
                image_id: format!("engineering-{}", target_name(target)),
                state_compatibility_id: image.platform().state_compatibility_id().into(),
                build_source_commit: image.platform().source_commit().into(),
            },
            target,
            disk_bytes,
            disk_sha256,
            boot,
            files: [
                image.disk().try_clone().map_err(storage)?.into_std(),
                image.kernel().try_clone().map_err(storage)?.into_std(),
                image.initrd().try_clone().map_err(storage)?.into_std(),
            ],
        })
    }
    pub(crate) fn from_verified(image: VerifiedImageSet) -> Result<Self, ImageError> {
        let descriptor = image.image();
        let raw = descriptor.manifest_bytes();
        if format!("{:x}", Sha256::digest(raw)) != descriptor.identity().manifest_sha256 {
            return Err(ImageError::Integrity);
        }
        let raw: serde_json::Value =
            serde_json::from_slice(raw).map_err(|_| ImageError::Integrity)?;
        let boot =
            serde_json::from_value(raw["directBoot"].clone()).map_err(|_| ImageError::Integrity)?;
        Ok(Self {
            identity: descriptor.identity().clone(),
            target: descriptor.target(),
            disk_bytes: descriptor.disk_bytes(),
            disk_sha256: descriptor.disk_sha256().into(),
            boot,
            files: [
                image.open_disk()?,
                image.open_boot(ImageAssetRole::Kernel)?,
                image.open_boot(ImageAssetRole::Initrd)?,
            ],
        })
    }
    pub fn identity(&self) -> &ImageIdentity {
        &self.identity
    }
    pub fn target(&self) -> ImageTarget {
        self.target
    }
    #[allow(dead_code)] // Portable image-only harnesses omit the native upgrade owner.
    pub(crate) fn required_install_bytes(&self) -> Result<u64, ImageError> {
        self.disk_bytes
            .checked_add(self.boot.kernel.bytes)
            .and_then(|n| n.checked_add(self.boot.initrd.bytes))
            .and_then(|n| n.checked_add(bridge_runtime_protocol::STATE_BYTES))
            .ok_or(ImageError::Integrity)
    }
    pub async fn materialize(
        self,
        target: &InstallationImageTarget<'_>,
        closed: watch::Receiver<bool>,
        progress: impl Fn(ImageProgress) + Send + Sync,
    ) -> Result<InstalledImageSet, ImageError> {
        materialize_with(move || Ok(self), target, closed, progress).await
    }
    fn open(&self, index: usize) -> Result<File, ImageError> {
        use std::io::Seek;
        let mut file = self.files[index].try_clone().map_err(storage)?;
        file.rewind().map_err(storage)?;
        Ok(file)
    }
}

struct Context {
    root: Dir,
    owner: Arc<File>,
    id: Uuid,
    target: ImageTarget,
    compatibility: String,
    creation_allowed: bool,
    closed: watch::Receiver<bool>,
    events: mpsc::Sender<ImageProgress>,
    #[cfg(test)]
    point: Option<Point>,
    #[cfg(test)]
    available: Option<u64>,
}
impl Context {
    fn stop(&self) -> Result<(), ImageError> {
        super::image_cache::verification::cancelled(&self.closed)?;
        if self.events.is_closed() {
            return Err(ImageError::Cancelled);
        }
        Ok(())
    }
    fn point(&self, name: &str) -> Result<(), ImageError> {
        self.stop()?;
        #[cfg(test)]
        if let Some(point) = &self.point {
            point(name)?;
        }
        let _ = name;
        self.stop()
    }
    fn space(&self, bytes: u64) -> Result<(), ImageError> {
        let available = available_space(&self.root)?;
        #[cfg(test)]
        let available = self.available.unwrap_or(available);
        if available < bytes {
            return Err(ImageError::NoSpace);
        }
        Ok(())
    }
    fn target(&self) -> InstallationImageTarget<'_> {
        let target = InstallationImageTarget::from_owned_installation(
            &self.root,
            &self.owner,
            self.id,
            self.target,
            &self.compatibility,
            self.creation_allowed,
        );
        #[cfg(test)]
        let target = InstallationImageTarget {
            point: self.point.clone(),
            ..target
        };
        target
    }
}
struct WorkOwner(File, u32);
impl Drop for WorkOwner {
    fn drop(&mut self) {
        if self.1 == std::process::id() {
            let _ = self.0.unlock();
        }
    }
}
fn write_json<T: Serialize>(root: &Dir, name: &str, value: &T) -> Result<(), ImageError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ImageError::Integrity)?;
    if bytes.len() as u64 > MAX_BINDING {
        return Err(ImageError::LimitExceeded);
    }
    let mut file = regular(root, name, true, true)?;
    file.set_len(0).map_err(storage)?;
    file.write_all(&bytes).map_err(storage)?;
    file.sync_all().map_err(storage)
}
#[cfg(unix)]
pub(crate) fn allocation(file: &File) -> Result<u64, ImageError> {
    use std::os::unix::fs::MetadataExt;
    file.metadata()
        .map_err(storage)?
        .blocks()
        .checked_mul(512)
        .ok_or(ImageError::NoSpace)
}
#[cfg(windows)]
pub(crate) fn allocation(file: &File) -> Result<u64, ImageError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
    };
    let mut info = FILE_STANDARD_INFO::default();
    if unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    } == 0
    {
        return Err(storage(std::io::Error::last_os_error()));
    }
    u64::try_from(info.AllocationSize).map_err(|_| ImageError::NoSpace)
}
fn resume_prefix(
    context: &Context,
    output: &mut File,
    source: &mut File,
    bytes: u64,
) -> Result<(u64, Sha256), ImageError> {
    let len = output.metadata().map_err(storage)?.len();
    let mut count = 0;
    let mut hashed = Sha256::new();
    let mut retained = [0; 65536];
    let mut original = [0; 65536];
    if len <= bytes {
        while count < len {
            context.stop()?;
            let n = (len - count).min(retained.len() as u64) as usize;
            output.read_exact(&mut retained[..n]).map_err(storage)?;
            source.read_exact(&mut original[..n]).map_err(storage)?;
            if retained[..n] != original[..n] {
                break;
            }
            hashed.update(&retained[..n]);
            count += n as u64;
        }
        if count == len {
            return Ok((count, hashed));
        }
    }
    context.stop()?;
    output.set_len(0).map_err(storage)?;
    output.rewind().map_err(storage)?;
    source.rewind().map_err(storage)?;
    Ok((0, Sha256::new()))
}
/// On APFS the private copy shares the verified cache file's blocks until the guest writes,
/// so it costs no space or copying. Any refusal (another volume, no clone support) copies instead.
#[cfg(target_os = "macos")]
fn clone(context: &Context, stage: &Dir, name: &str, source: &File) -> Result<bool, ImageError> {
    use std::os::fd::AsRawFd;
    context.stop()?;
    // Tests refuse the clone to exercise the copy that other volumes get.
    #[cfg(test)]
    if context.point.as_ref().is_some_and(|p| p("clone").is_err()) {
        return Ok(false);
    }
    let path = std::ffi::CString::new(name).map_err(|_| ImageError::UnsafePath)?;
    // fclonefileat creates the name only if absent and reads its arguments only during the call.
    if unsafe { libc::fclonefileat(source.as_raw_fd(), stage.as_raw_fd(), path.as_ptr(), 0) } != 0 {
        return Ok(false);
    }
    // A clone carries the cache file's mode and flags; give it what a fresh copy would have,
    // or remove it so a retry never trusts a clone by its bytes alone.
    let normalized = regular(stage, name, false, false).is_ok_and(|file| {
        // Both calls act on this open descriptor only.
        let (flags, mode) = unsafe {
            (
                libc::fchflags(file.as_raw_fd(), 0),
                libc::fchmod(file.as_raw_fd(), 0o644),
            )
        };
        flags == 0 && mode == 0
    });
    if !normalized {
        stage.remove_file(name).map_err(storage)?;
    }
    Ok(normalized)
}
#[cfg(not(target_os = "macos"))]
fn clone(_: &Context, _: &Dir, _: &str, _: &File) -> Result<bool, ImageError> {
    Ok(false)
}
fn copy(
    context: &Context,
    stage: &Dir,
    name: &str,
    mut source: File,
    expected: (u64, &str),
    done: &mut u64,
    total: u64,
) -> Result<Payload, ImageError> {
    let (bytes, sha) = expected;
    let (reusable, absent) = match read_file(stage, name) {
        Ok(mut file) => match digest(&mut file, bytes, sha, || context.stop()) {
            Ok(()) => (true, false),
            Err(ImageError::Integrity) => (false, false),
            Err(error) => return Err(error),
        },
        Err(e) if missing(&e) => (false, true),
        Err(e) => return Err(e),
    };
    if !reusable && absent && clone(context, stage, name, &source)? {
        // A clone is written whole; interruption tests stop here as they would mid-copy.
        context.point("copy-chunk")?;
    } else if !reusable {
        let mut output = regular(stage, name, true, true)?;
        let (mut count, mut hashed) = resume_prefix(context, &mut output, &mut source, bytes)?;
        context.space(bytes - count)?;
        context
            .events
            .blocking_send(ImageProgress::Retained {
                bytes: *done + count,
                total,
            })
            .map_err(|_| ImageError::Cancelled)?;
        let mut buffer = [0; 65536];
        loop {
            context.stop()?;
            let n = source.read(&mut buffer).map_err(storage)?;
            if n == 0 {
                break;
            }
            count = count
                .checked_add(n as u64)
                .filter(|n| *n <= bytes)
                .ok_or(ImageError::Integrity)?;
            context.space(n as u64)?;
            output.write_all(&buffer[..n]).map_err(storage)?;
            hashed.update(&buffer[..n]);
            context
                .events
                .blocking_send(ImageProgress::Materializing {
                    bytes: *done + count,
                    total,
                })
                .map_err(|_| ImageError::Cancelled)?;
            context.point("copy-chunk")?;
        }
        if count != bytes || format!("{:x}", hashed.finalize()) != sha {
            return Err(ImageError::Integrity);
        }
        output.sync_all().map_err(storage)?;
        drop(output);
    } else {
        context
            .events
            .blocking_send(ImageProgress::Retained {
                bytes: *done + bytes,
                total,
            })
            .map_err(|_| ImageError::Cancelled)?;
    }
    context.point(&format!("written-{name}"))?;
    let mut actual = regular(stage, name, true, false)?;
    digest(&mut actual, bytes, sha, || context.stop())?;
    context.point("payload-flush")?;
    actual.sync_all().map_err(storage)?;
    *done = done.checked_add(bytes).ok_or(ImageError::NoSpace)?;
    context
        .events
        .blocking_send(ImageProgress::Materializing {
            bytes: *done,
            total,
        })
        .map_err(|_| ImageError::Cancelled)?;
    Ok(Payload {
        path: format!("image/1/{name}"),
        identity: fid(&actual)?,
        initial_bytes: bytes,
        initial_sha256: sha.into(),
        allocated_bytes: allocation(&actual)?,
    })
}
fn run(image: PreparedImage, context: Context) -> Result<InstalledImageSet, ImageError> {
    context.stop()?;
    validate_target(&context.root, &context.owner, context.id)?;
    if !context.creation_allowed {
        return Err(ImageError::RetainedImage);
    }
    if image.target() != context.target {
        return Err(ImageError::Incompatible(Incompatibility::Target));
    }
    if image.identity().state_compatibility_id != context.compatibility {
        return Err(ImageError::RetainedImage);
    }
    let owner = regular(&context.root, "image-owner.lock", true, true)?;
    owner.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => ImageError::Busy,
        std::fs::TryLockError::Error(e) => storage(e),
    })?;
    let _work = WorkOwner(owner, std::process::id());
    match open_installed_image(&context.target()) {
        Ok(set) => {
            if set.identity.manifest_sha256 != image.identity().manifest_sha256 {
                return Err(ImageError::RetainedImage);
            }
            return Ok(set);
        }
        Err(ImageError::RetainedImage) => {}
        Err(e) => return Err(e),
    }
    // A present malformed selector is never overwritten or treated as incomplete owned work.
    if regular(&context.root, "image-binding.json", false, false).is_ok() {
        return Err(ImageError::RetainedImage);
    }
    let intent = Intent {
        version: 1,
        installation_id: context.id,
        image_generation: 1,
        identity: Identity {
            manifest_sha256: image.identity().manifest_sha256.clone(),
            image_id: image.identity().image_id.clone(),
            state_compatibility_id: context.compatibility.clone(),
            build_source_commit: image.identity().build_source_commit.clone(),
        },
        target: target_name(context.target).into(),
    };
    let boot = &image.boot;
    let images = directory(&context.root, std::ffi::OsStr::new("image"))?;
    let binding = if let Some(generation) = read_directory(&images, "1")? {
        let existing: Intent = json(&generation, "intent.json")?;
        if existing != intent {
            return Err(ImageError::RetainedImage);
        }
        json::<Binding>(&generation, "binding.json")?
    } else {
        let fresh = match images.create_dir(".pending-1") {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(storage(e)),
        };
        let stage = read_directory(&images, ".pending-1")?.ok_or(ImageError::RetainedImage)?;
        if fresh {
            write_json(&stage, "intent.json", &intent)?;
        } else if json::<Intent>(&stage, "intent.json")? != intent {
            return Err(ImageError::RetainedImage);
        }
        image_cache::fs::sync(&stage)?;
        image_cache::fs::sync(&images)?;
        image_cache::fs::sync(&context.root)?;
        let total = image
            .disk_bytes
            .checked_add(boot.kernel.bytes)
            .and_then(|n| n.checked_add(boot.initrd.bytes))
            .ok_or(ImageError::NoSpace)?;
        let mut needed = 0u64;
        for (name, bytes, sha) in [
            (FILES[0], image.disk_bytes, image.disk_sha256.as_str()),
            (FILES[1], boot.kernel.bytes, boot.kernel.sha256.as_str()),
            (FILES[2], boot.initrd.bytes, boot.initrd.sha256.as_str()),
        ] {
            context.stop()?;
            let remaining = match read_file(&stage, name) {
                Ok(mut file) => {
                    let len = file.metadata().map_err(storage)?.len();
                    if len < bytes {
                        // Keep the prefix; copy compares it to the source before appending.
                        bytes - len
                    } else {
                        match digest(&mut file, bytes, sha, || context.stop()) {
                            Ok(()) => 0,
                            Err(ImageError::Integrity) => {
                                context.stop()?;
                                drop(file);
                                regular(&stage, name, true, false)?
                                    .set_len(0)
                                    .map_err(storage)?;
                                bytes
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(e) if missing(&e) => bytes,
                Err(e) => return Err(e),
            };
            needed = needed.checked_add(remaining).ok_or(ImageError::NoSpace)?;
        }
        context.space(needed)?;
        let mut done = 0;
        let os = copy(
            &context,
            &stage,
            FILES[0],
            image.open(0)?,
            (image.disk_bytes, image.disk_sha256.as_str()),
            &mut done,
            total,
        )?;
        let kernel = copy(
            &context,
            &stage,
            FILES[1],
            image.open(1)?,
            (boot.kernel.bytes, &boot.kernel.sha256),
            &mut done,
            total,
        )?;
        let initrd = copy(
            &context,
            &stage,
            FILES[2],
            image.open(2)?,
            (boot.initrd.bytes, &boot.initrd.sha256),
            &mut done,
            total,
        )?;
        let binding = Binding {
            version: 1,
            image_generation: 1,
            installation_id: context.id,
            installation_directory: dir_id(&context.root)?,
            image_directory: dir_id(&stage)?,
            identity: intent.identity.clone(),
            target: intent.target.clone(),
            launch: if context.target == ImageTarget::MacosArm64 {
                "uefi"
            } else {
                "linux-direct"
            }
            .into(),
            os_format: if context.target == ImageTarget::MacosArm64 {
                "raw"
            } else {
                "qcow2"
            }
            .into(),
            boot: boot.clone(),
            os,
            kernel,
            initrd,
        };
        write_json(&stage, "binding.json", &binding)?;
        image_cache::fs::sync(&stage)?;
        context.point("stage-flushed")?;
        drop(stage);
        image_cache::fs::publish_named(&images, ".pending-1", "1")?;
        image_cache::fs::sync(&images)?;
        image_cache::fs::sync(&context.root)?;
        context.point("generation-published")?;
        binding
    };
    if binding.identity != intent.identity
        || &binding.boot != boot
        || binding.os.initial_bytes != image.disk_bytes
        || binding.os.initial_sha256 != image.disk_sha256.as_str()
    {
        return Err(ImageError::RetainedImage);
    }
    validate_binding(&binding, &context.target())?;
    verify_files(&context.root, &binding, true, || context.stop())?;
    context.point("generation-verified")?;
    sync_namespace(&context.target(), &binding)?;
    write_json(&context.root, "image-binding.pending", &binding)?;
    context.point("binding-flushed")?;
    image_cache::fs::publish_named(&context.root, "image-binding.pending", "image-binding.json")?;
    image_cache::fs::sync(&context.root)?;
    #[cfg(test)]
    if let Some(point) = &context.point {
        point("binding-published")?;
    }
    open_installed_image(&context.target())
}
/// Copies only under borrowed current ownership. The actual job retains temporary ownership
/// if its future is dropped; the installed result retains metadata only.
pub async fn materialize_image(
    image: &VerifiedImageSet,
    target: &InstallationImageTarget<'_>,
    closed: watch::Receiver<bool>,
    progress: impl Fn(ImageProgress) + Send + Sync,
) -> Result<InstalledImageSet, ImageError> {
    let image = image.clone();
    materialize_with(
        move || PreparedImage::from_verified(image),
        target,
        closed,
        progress,
    )
    .await
}
async fn materialize_with(
    prepare: impl FnOnce() -> Result<PreparedImage, ImageError> + Send + 'static,
    target: &InstallationImageTarget<'_>,
    closed: watch::Receiver<bool>,
    progress: impl Fn(ImageProgress) + Send + Sync,
) -> Result<InstalledImageSet, ImageError> {
    let (events, mut receive) = mpsc::channel(1);
    let context = Context {
        root: target.root.try_clone().map_err(storage)?,
        owner: Arc::clone(target.owner),
        id: target.id,
        target: target.target,
        compatibility: target.compatibility.into(),
        creation_allowed: target.creation_allowed,
        closed,
        events,
        #[cfg(test)]
        point: target.point.clone(),
        #[cfg(test)]
        available: target.available,
    };
    let mut job = tokio::task::spawn_blocking(move || {
        context.stop()?;
        run(prepare()?, context)
    });
    loop {
        tokio::select! {
            result = &mut job => {
                while let Ok(event) = receive.try_recv() { progress(event); }
                let result = result.map_err(|_| ImageError::Storage(std::io::ErrorKind::Other))?;
                if let Ok(set) = &result {
                    let total = set.binding.os.initial_bytes + set.binding.kernel.initial_bytes + set.binding.initrd.initial_bytes;
                    progress(ImageProgress::Materializing { bytes: total, total });
                    progress(ImageProgress::Verified);
                }
                return result;
            }
            Some(event) = receive.recv() => progress(event)
        }
    }
}
#[cfg(test)]
#[path = "image_install_tests.rs"]
mod tests;
