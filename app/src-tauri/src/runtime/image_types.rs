//! Rust-owned inputs and opaque validated descriptors. None are renderer-deserializable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageTarget {
    MacosArm64,
    WindowsAmd64,
}
#[derive(Clone, Debug)]
pub struct HostTarget {
    pub target: ImageTarget,
    pub host_version: String,
    pub app_version: String,
}
#[derive(Clone, Copy, Debug)]
pub struct TransferLimits {
    pub compressed_bytes: u64,
    pub disk_bytes: u64,
    pub kernel_bytes: u64,
    pub initrd_bytes: u64,
}
/// Supplied only by trusted Rust selection, never renderer input.
#[derive(Clone, Debug)]
pub struct ImagePin {
    pub release_id: String,
    pub manifest_url: String,
    pub manifest_sha256: String,
    pub accepted_schema_sha256: String,
    pub target: ImageTarget,
    pub asset_base: String,
    pub redirect_origins: Vec<String>,
    pub limits: TransferLimits,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Incompatibility {
    Target,
    HostVersion,
    AppVersion,
    UnsupportedVersionPolicy,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    Invalid(&'static str),
    Unsupported,
    Incompatible(Incompatibility),
    Integrity,
    LimitExceeded,
    Cancelled,
    Network(&'static str),
    Storage(std::io::ErrorKind),
    NoSpace,
    UnsafePath,
    Busy,
    RetainedImage,
    WorkspaceUpdateRequired,
    WorkspaceVersionUnsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Uefi,
    LinuxDirect,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageIdentity {
    pub manifest_sha256: String,
    pub image_id: String,
    pub state_compatibility_id: String,
    pub build_source_commit: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageProgress {
    Download {
        bytes: u64,
        total: u64,
    },
    Verifying,
    Verified,
    Materializing {
        bytes: u64,
        total: u64,
    },
    /// Retained bytes are a baseline, not fresh movement for transfer-rate estimates.
    Retained {
        bytes: u64,
        total: u64,
    },
}

/// A revalidated factory read capability; private installed copies still require verification.
#[derive(Clone)]
pub struct VerifiedImageSet {
    pub(crate) directory: std::sync::Arc<cap_std::fs::Dir>,
    pub(crate) image: ResolvedImage,
}
impl VerifiedImageSet {
    pub fn image(&self) -> &ResolvedImage {
        &self.image
    }
}

/// Base and validated leaf stay separate; the future HTTP adapter owns URL parsing/policy.
#[derive(Clone, Debug)]
pub struct AssetLocation {
    pub(crate) base: String,
    pub(crate) leaf: String,
}
impl AssetLocation {
    pub fn base(&self) -> &str {
        &self.base
    }
    pub fn leaf(&self) -> &str {
        &self.leaf
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageAssetRole {
    Chunk(u32),
    Kernel,
    Initrd,
}
#[derive(Clone, Debug)]
pub struct ImageAsset {
    pub(crate) location: AssetLocation,
    pub(crate) role: ImageAssetRole,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}
impl ImageAsset {
    pub fn location(&self) -> &AssetLocation {
        &self.location
    }
    pub fn role(&self) -> ImageAssetRole {
        self.role
    }
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}
/// Only resolve_image can construct this; asset names and launch policy are validated.
#[derive(Clone, Debug)]
pub struct ResolvedImage {
    pub(crate) identity: ImageIdentity,
    pub(crate) target: ImageTarget,
    pub(crate) launch_mode: LaunchMode,
    pub(crate) command_line: String,
    pub(crate) initrd_compression: String,
    pub(crate) assets: Vec<ImageAsset>,
    pub(crate) limits: TransferLimits,
    pub(crate) compressed_sha256: String,
    pub(crate) compressed_bytes: u64,
    pub(crate) disk_sha256: String,
    pub(crate) disk_bytes: u64,
    pub(crate) manifest_bytes: std::sync::Arc<[u8]>,
}
impl ResolvedImage {
    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest_bytes
    }
    pub fn identity(&self) -> &ImageIdentity {
        &self.identity
    }
    pub fn target(&self) -> ImageTarget {
        self.target
    }
    pub fn launch_mode(&self) -> LaunchMode {
        self.launch_mode
    }
    pub fn command_line(&self) -> &str {
        &self.command_line
    }
    pub fn initrd_compression(&self) -> &str {
        &self.initrd_compression
    }
    pub fn assets(&self) -> &[ImageAsset] {
        &self.assets
    }
    pub fn limits(&self) -> TransferLimits {
        self.limits
    }
    pub fn compressed_sha256(&self) -> &str {
        &self.compressed_sha256
    }
    pub fn compressed_bytes(&self) -> u64 {
        self.compressed_bytes
    }
    pub fn disk_sha256(&self) -> &str {
        &self.disk_sha256
    }
    pub fn disk_bytes(&self) -> u64 {
        self.disk_bytes
    }
}
