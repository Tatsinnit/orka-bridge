//! Native-owned source selection. A catalog entry is a reviewed release input,
//! never a renderer URL or a development environment override.
use crate::runtime::image_types::*;

#[derive(Clone)]
pub(super) struct ReleaseImage {
    pub pin: ImagePin,
    // Keep the reviewed manifest in the app profile so cache-free reopen can
    // recheck authenticated host/app policy without network or new disk state.
    pub manifest: Vec<u8>,
    pub image_id: String,
    pub state_compatibility_id: String,
    pub predecessors: Vec<Predecessor>,
}
#[derive(Clone)]
pub(super) struct Predecessor {
    pub source: Box<ReleaseImage>,
    pub app_releases: Vec<String>,
    pub target_manifests: Vec<String>,
}
impl ReleaseImage {
    fn validate_installed(
        &self,
        image: &crate::runtime::image_install::InstalledImageSet,
        host: &HostTarget,
    ) -> Result<(), ImageError> {
        let current = self.validate_host(host)?;
        if image.identity().manifest_sha256 == self.pin.manifest_sha256 {
            return image.validate_pinned(&current);
        }
        for predecessor in &self.predecessors {
            if image.identity().manifest_sha256 != predecessor.source.pin.manifest_sha256 {
                continue;
            }
            let resolved = predecessor
                .source
                .validate_host(host)
                .map_err(|error| match error {
                    ImageError::Unsupported | ImageError::Incompatible(_) => {
                        ImageError::WorkspaceVersionUnsupported
                    }
                    other => other,
                })?;
            image.validate_pinned(&resolved)?;
            return Err(
                if !predecessor.app_releases.is_empty()
                    && predecessor
                        .target_manifests
                        .contains(&self.pin.manifest_sha256)
                {
                    ImageError::WorkspaceUpdateRequired
                } else {
                    ImageError::WorkspaceVersionUnsupported
                },
            );
        }
        Err(ImageError::WorkspaceVersionUnsupported)
    }
    fn validate_host(&self, host: &HostTarget) -> Result<ResolvedImage, ImageError> {
        let resolved = crate::runtime::manifest::resolve_image(&self.pin, &self.manifest, host)?;
        self.validate(resolved.identity(), resolved.target())?;
        Ok(resolved)
    }
    pub fn validate(
        &self,
        identity: &ImageIdentity,
        target: ImageTarget,
    ) -> Result<(), ImageError> {
        if identity.manifest_sha256 != self.pin.manifest_sha256
            || identity.image_id != self.image_id
            || identity.state_compatibility_id != self.state_compatibility_id
            || target != self.pin.target
        {
            return Err(ImageError::Integrity);
        }
        Ok(())
    }
}

/// The URLs stay fixed across the draft walks and app publication. Missing
/// public images are a download error, never a request for operator credentials.
pub(super) fn shipping_image(target: ImageTarget) -> Option<ReleaseImage> {
    let (manifest, digest) = match target {
        ImageTarget::MacosArm64 => (
            include_bytes!("../../../images/release/macos-arm64.json").as_slice(),
            "f8c2d2b1423aa63b9f9b626eb428c944e5283277303872e47df655ee6064709e",
        ),
        ImageTarget::WindowsAmd64 => (
            include_bytes!("../../../images/release/windows-amd64.json").as_slice(),
            "be18ce4298a22b28bec2de5776288726bc5bd020c2dc9a62c360fc256a9718e9",
        ),
    };
    let mut image = release_image(
        target,
        manifest,
        digest,
        "bridge-images-preview-43e73491-r2",
    )?;
    let source = match target {
        ImageTarget::MacosArm64 => {
            include_bytes!("../../../images/predecessors/v0.1.2/macos-arm64.json").as_slice()
        }
        ImageTarget::WindowsAmd64 => {
            include_bytes!("../../../images/predecessors/v0.1.2/windows-amd64.json").as_slice()
        }
    };
    let source_digest = match target {
        ImageTarget::MacosArm64 => {
            "f8c2d2b1423aa63b9f9b626eb428c944e5283277303872e47df655ee6064709e"
        }
        ImageTarget::WindowsAmd64 => {
            "be18ce4298a22b28bec2de5776288726bc5bd020c2dc9a62c360fc256a9718e9"
        }
    };
    image.predecessors.push(Predecessor {
        source: Box::new(release_image(
            target,
            source,
            source_digest,
            "bridge-images-preview-43e73491-r2",
        )?),
        app_releases: vec!["v0.1.2".into()],
        // The image-release owner registers exact qualified destination hashes.
        // Keeping source evidence is not permission to move to an arbitrary pin.
        target_manifests: vec![],
    });
    Some(image)
}
fn release_image(
    target: ImageTarget,
    manifest: &[u8],
    digest: &str,
    release: &str,
) -> Option<ReleaseImage> {
    let value: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    let platform = match target {
        ImageTarget::MacosArm64 => "macos-arm64",
        ImageTarget::WindowsAmd64 => "windows-amd64",
    };
    let base = format!("https://github.com/orka-agents/orka-bridge/releases/download/{release}/");
    Some(ReleaseImage {
        pin: ImagePin {
            release_id: release.into(),
            manifest_url: format!("{base}manifest-{platform}.json"),
            manifest_sha256: digest.into(),
            accepted_schema_sha256:
                "80a867ef73ef29685f3d7e7d4d72365ca823cc9a56ce6e0ea9e1ee3e39b92d21".into(),
            target,
            asset_base: base,
            redirect_origins: vec!["https://release-assets.githubusercontent.com".into()],
            limits: TransferLimits {
                compressed_bytes: value["artifact"]["compressedBytes"].as_u64()?,
                disk_bytes: value["artifact"]["uncompressedBytes"].as_u64()?,
                kernel_bytes: value["directBoot"]["kernel"]["bytes"].as_u64()?,
                initrd_bytes: value["directBoot"]["initrd"]["bytes"].as_u64()?,
            },
        },
        manifest: manifest.to_vec(),
        image_id: value["imageId"].as_str()?.into(),
        state_compatibility_id: value["stateCompatibilityId"].as_str()?.into(),
        predecessors: vec![],
    })
}

#[derive(Clone)]
pub(super) enum ImageSource {
    #[cfg(debug_assertions)]
    Development,
    Pinned {
        profile: Box<ReleaseImage>,
        host: HostTarget,
    },
}
impl ImageSource {
    pub fn compatibility(&self) -> &str {
        match self {
            #[cfg(debug_assertions)]
            Self::Development => crate::dev_image::STATE_COMPATIBILITY_ID,
            Self::Pinned { profile, .. } => &profile.state_compatibility_id,
        }
    }
    pub fn validate(
        &self,
        identity: &ImageIdentity,
        target: ImageTarget,
    ) -> Result<(), ImageError> {
        match self {
            #[cfg(debug_assertions)]
            Self::Development => Ok(()),
            Self::Pinned { profile, .. } => profile.validate(identity, target),
        }
    }
}

pub(super) fn select_source(
    development: bool,
    override_present: bool,
    installed: bool,
    release: Option<(ReleaseImage, HostTarget)>,
) -> Option<ImageSource> {
    #[cfg(debug_assertions)]
    if development && (override_present || (installed && release.is_none())) {
        return Some(ImageSource::Development);
    }
    let _ = (development, override_present, installed);
    release.map(|(profile, host)| ImageSource::Pinned {
        profile: Box::new(profile),
        host,
    })
}

pub(super) fn open_selected(
    source: &mut ImageSource,
    journal: &crate::runtime::journal::Journal,
) -> Result<
    std::sync::Arc<crate::runtime::image_install::InstalledImageSet>,
    crate::runtime::types::RuntimeError,
> {
    use crate::runtime::image_install::inspect_installed_image;
    #[cfg(debug_assertions)]
    use crate::runtime::image_install::open_installed_image;
    let target = journal.image_target(source.compatibility())?;
    let opened = match source {
        ImageSource::Pinned { .. } => inspect_installed_image(&target),
        #[cfg(debug_assertions)]
        ImageSource::Development => open_installed_image(&target),
    };
    #[cfg(debug_assertions)]
    if matches!(source, ImageSource::Pinned { .. })
        && (matches!(opened, Err(ImageError::RetainedImage))
            || opened.as_ref().is_ok_and(|image| {
                image.identity().state_compatibility_id == crate::dev_image::STATE_COMPATIBILITY_ID
                    && matches!(source, ImageSource::Pinned { profile, .. }
                        if image.identity().manifest_sha256 != profile.pin.manifest_sha256)
            }))
    {
        // Preserve debug reopen through the verified development binding, even
        // when release and development share a state ID but not a manifest hash.
        // This branch is absent from release builds.
        if let Ok(image) =
            open_installed_image(&journal.image_target(crate::dev_image::STATE_COMPATIBILITY_ID)?)
        {
            let engineering = match image.target() {
                ImageTarget::MacosArm64 => "engineering-macos-arm64",
                ImageTarget::WindowsAmd64 => "engineering-windows-amd64",
            };
            if image.identity().image_id == engineering {
                *source = ImageSource::Development;
                return Ok(std::sync::Arc::new(image));
            }
        }
    }
    let image = opened?;
    match source {
        ImageSource::Pinned { profile, host } => profile.validate_installed(&image, host)?,
        #[cfg(debug_assertions)]
        ImageSource::Development => {}
    }
    Ok(std::sync::Arc::new(image))
}

/// This helper never owns a backend. The factory keeps the original Journal
/// across preparation retries and transfers it only at the native launch boundary.
#[allow(clippy::too_many_arguments)] // Keep the source, paths and retained owner explicit.
pub(super) fn prepare_installation(
    source: &ImageSource,
    root: &std::path::Path,
    cache_path: &std::path::Path,
    custody: &mut crate::runtime::journal::JournalCustody,
    closed: tokio::sync::watch::Receiver<bool>,
    progress: impl Fn(crate::local_vm::LocalProgress) + Send + Sync,
    create: impl FnOnce()
        -> Result<crate::runtime::journal::Journal, crate::runtime::types::RuntimeError>,
    runtime: &tokio::runtime::Handle,
) -> Result<
    std::sync::Arc<crate::runtime::image_install::InstalledImageSet>,
    crate::runtime::types::RuntimeError,
> {
    use crate::{
        local_vm::{LocalProgress, LocalVmStatus},
        runtime::{acquire::ImageHttpClient, image_cache::ImageCache, types::RuntimeError},
    };
    if *closed.borrow() {
        return Err(RuntimeError::Closed);
    }
    progress(LocalProgress::Image(ImageProgress::Verifying));
    let mut source = source.clone();
    let existing = if let Some(journal) = &custody.0 {
        match open_selected(&mut source, journal) {
            Ok(image) => Some(image),
            Err(RuntimeError::Image(ImageError::RetainedImage)) => None,
            Err(error) => return Err(error),
        }
    } else {
        match super::reopen(root, &mut source)? {
            Some((journal, image)) => {
                custody.0 = Some(journal);
                Some(image)
            }
            None => None,
        }
    };
    let resolved = match &source {
        #[cfg(debug_assertions)]
        ImageSource::Development => None,
        ImageSource::Pinned { profile, host } => Some(profile.validate_host(host)?),
    };
    if let Some(image) = existing {
        source.validate(image.identity(), image.target())?;
        if let Some(resolved) = &resolved {
            image.validate_pinned(resolved)?;
        }
        return Ok(image);
    }
    let prepared = match &source {
        #[cfg(debug_assertions)]
        ImageSource::Development => {
            let image = crate::dev_image::from_environment_cancellable(&closed)
                .map_err(|_| {
                    if *closed.borrow() {
                        RuntimeError::Closed
                    } else {
                        RuntimeError::Unavailable
                    }
                })?
                .ok_or(RuntimeError::Unavailable)?;
            runtime.block_on(crate::runtime::setup_adapter::prepare_development_image(
                &image,
                closed.clone(),
            ))?
        }
        ImageSource::Pinned { profile, host } => {
            std::fs::create_dir_all(cache_path.parent().ok_or(RuntimeError::Unavailable)?)?;
            let cache = ImageCache::open(cache_path)?;
            let client = ImageHttpClient::new(&profile.pin)?;
            runtime.block_on(crate::runtime::setup_adapter::prepare_pinned_image(
                &profile.pin,
                host,
                &cache,
                &client,
                closed.clone(),
                |event| progress(LocalProgress::Image(event)),
            ))?
        }
    };
    source.validate(prepared.identity(), prepared.target())?;
    if *closed.borrow() {
        return Err(RuntimeError::Closed);
    }
    if custody.0.is_none() {
        custody.0 = Some(create()?);
    }
    progress(LocalProgress::Status(LocalVmStatus::Installing));
    let journal = custody.0.as_ref().ok_or(RuntimeError::Unavailable)?;
    let image = runtime.block_on(prepared.materialize(
        &journal.image_target(source.compatibility())?,
        closed,
        |event| progress(LocalProgress::Image(event)),
    ))?;
    if let Some(resolved) = &resolved {
        image.validate_pinned(resolved)?;
    }
    Ok(std::sync::Arc::new(image))
}

pub(super) fn checked_host_version(value: &str) -> Result<String, ImageError> {
    let value = value.strip_suffix('\0').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if !(2..=3).contains(&parts.len())
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || part.parse::<u32>().is_err()
        })
    {
        return Err(ImageError::Unsupported);
    }
    Ok(value.into())
}

pub(super) fn native_host_target(target: ImageTarget) -> Result<HostTarget, ImageError> {
    #[cfg(all(windows, target_arch = "x86_64"))]
    let version = {
        use windows_sys::{
            Wdk::System::SystemServices::RtlGetVersion,
            Win32::System::SystemInformation::OSVERSIONINFOW,
        };
        if target != ImageTarget::WindowsAmd64 {
            return Err(ImageError::Unsupported);
        }
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        // Unlike compatibility-manifest version APIs, this reports the host OS.
        if unsafe { RtlGetVersion(&mut info) } != 0 {
            return Err(ImageError::Unsupported);
        }
        format!(
            "{}.{}.{}",
            info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
        )
    };
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let version = {
        if target != ImageTarget::MacosArm64 {
            return Err(ImageError::Unsupported);
        }
        let mut bytes = [0u8; 64];
        let mut size = bytes.len();
        // The product version, not Darwin's kernel version. Bounded and read-only.
        if unsafe {
            libc::sysctlbyname(
                c"kern.osproductversion".as_ptr(),
                bytes.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            || size == 0
            || size > bytes.len()
            || bytes[size - 1] != 0
        {
            return Err(ImageError::Unsupported);
        }
        std::str::from_utf8(&bytes[..size])
            .map_err(|_| ImageError::Unsupported)?
            .to_owned()
    };
    #[cfg(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    return Ok(HostTarget {
        target,
        host_version: checked_host_version(&version)?,
        app_version: env!("CARGO_PKG_VERSION").into(),
    });
    #[cfg(not(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )))]
    {
        let _ = target;
        Err(ImageError::Unsupported)
    }
}
