use super::super::{image_cache::ImageCache, manifest::resolve_image};
use super::*;
use sha2::{Digest, Sha256};
use std::io::Write;

#[cfg(windows)]
#[test]
fn original_deny_sharing_owner_is_validated_without_reopening_its_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("install");
    let journal = crate::runtime::journal::Journal::create(
        &root,
        crate::runtime::types::BackendKind::Qemu,
        vec![1],
    )
    .unwrap();
    let target = journal.image_target("fixture-owner").unwrap();
    validate_target(target.root, target.owner, target.id).unwrap();
    assert_eq!(
        File::open(root.join("owner.lock"))
            .unwrap_err()
            .raw_os_error(),
        Some(32),
        "identity validation must not weaken the original deny-sharing guard"
    );
}
fn hash(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}
fn factory(path: &std::path::Path) -> VerifiedImageSet {
    factory_with_disk(path, b"private synthetic disk")
}
fn factory_with_disk(path: &std::path::Path, disk: &[u8]) -> VerifiedImageSet {
    factory_for(path, disk, ImageTarget::WindowsAmd64)
}
fn factory_for(path: &std::path::Path, disk: &[u8], target: ImageTarget) -> VerifiedImageSet {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(disk).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut kernel = vec![0; 4096];
    kernel[0x1fe..0x200].copy_from_slice(b"\x55\xaa");
    kernel[0x202..0x206].copy_from_slice(b"HdrS");
    kernel[0x38..0x3c].copy_from_slice(b"ARM\x64");
    let initrd = b"070701fixture".to_vec();
    let raw: &[u8] = if target == ImageTarget::MacosArm64 {
        include_bytes!("../../../../images/testdata/v2/producer-macos.json")
    } else {
        include_bytes!("../../../../images/testdata/v2/producer-windows.json")
    };
    let mut v: serde_json::Value = serde_json::from_slice(raw).unwrap();
    v["artifact"]["uncompressedBytes"] = disk.len().into();
    v["artifact"]["uncompressedSha256"] = hash(disk).into();
    v["artifact"]["compressedBytes"] = compressed.len().into();
    v["artifact"]["compressedSha256"] = hash(&compressed).into();
    v["artifact"]["chunks"].as_array_mut().unwrap().truncate(1);
    v["artifact"]["chunks"][0]["bytes"] = compressed.len().into();
    v["artifact"]["chunks"][0]["sha256"] = hash(&compressed).into();
    v["directBoot"]["provenance"]["diskSha256"] = hash(disk).into();
    for (name, data) in [("kernel", &kernel), ("initrd", &initrd)] {
        v["directBoot"][name]["bytes"] = data.len().into();
        v["directBoot"][name]["sha256"] = hash(data).into();
    }
    v["directBoot"]["initrd"]["compression"] = "cpio-prefix".into();
    let raw = serde_json::to_vec(&v).unwrap();
    let pin = ImagePin {
        release_id: "synthetic".into(),
        manifest_url: "https://fixture.invalid/m".into(),
        manifest_sha256: hash(&raw),
        accepted_schema_sha256: hash(include_bytes!("../../../../images/manifest.schema.json")),
        target,
        asset_base: "https://fixture.invalid/".into(),
        redirect_origins: vec![],
        limits: TransferLimits {
            compressed_bytes: 8192,
            disk_bytes: 1024 * 1024,
            kernel_bytes: 8192,
            initrd_bytes: 8192,
        },
    };
    let image = resolve_image(
        &pin,
        &raw,
        &HostTarget {
            target: pin.target,
            host_version: "13.0".into(),
            app_version: "1.0.0".into(),
        },
    )
    .unwrap();
    let cache = ImageCache::open(&path.join("cache")).unwrap();
    let tx = cache.begin(&image).unwrap();
    for (i, data) in [compressed, kernel, initrd].iter().enumerate() {
        let mut f = tx.start_asset(i).unwrap().unwrap();
        f.write_all(data).unwrap();
        drop(f);
        tx.finish_asset(i).unwrap();
    }
    let (_sender, closed) = tokio::sync::watch::channel(false);
    tx.commit(&closed).unwrap()
}
fn owner(path: &std::path::Path) -> (Dir, Arc<File>) {
    std::fs::create_dir(path).unwrap();
    let root = Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap();
    let f = File::options()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path.join("owner.lock"))
        .unwrap();
    f.try_lock().unwrap();
    (root, Arc::new(f))
}
#[tokio::test]
async fn materialization_copies_private_bytes_and_reopens_a_committed_binding() {
    let temp = tempfile::tempdir().unwrap();
    let image = factory(temp.path());
    let path = temp.path().join("install");
    let (root, lock) = owner(&path);
    let id = Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap();
    let target = InstallationImageTarget::from_owned_installation(
        &root,
        &lock,
        id,
        ImageTarget::WindowsAmd64,
        &image.image().identity().state_compatibility_id,
        true,
    );
    let (_sender, closed) = tokio::sync::watch::channel(false);
    let installed = materialize_image(&image, &target, closed, |_| {})
        .await
        .expect("verified private copy must commit the installed binding");
    assert_eq!(installed.installation_id(), id);
    assert_eq!(
        std::fs::read(path.join(installed.disk_path())).unwrap(),
        b"private synthetic disk"
    );
    assert!(path.join("image-binding.json").is_file());
    open_installed_image(&target).unwrap();
    std::fs::write(path.join(installed.disk_path()), b"mutable guest data").unwrap();
    open_installed_image(&target)
        .expect("initial OS hash is provenance after boot, not immutable content");
}
#[cfg(target_os = "macos")]
fn device_offset(file: &File) -> i64 {
    use std::os::fd::AsRawFd;
    let mut place = libc::log2phys {
        l2p_flags: 0,
        l2p_contigbytes: 4096,
        l2p_devoffset: 0,
    };
    assert_ne!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_LOG2PHYS_EXT, &mut place) },
        -1
    );
    place.l2p_devoffset
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn mac_private_disk_shares_the_cache_blocks_until_the_guest_writes() {
    use std::{io::Read, os::unix::fs::FileExt};
    let temp = tempfile::tempdir().unwrap();
    let disk: Vec<u8> = (0..256 * 1024u32).map(|i| (i % 251) as u8 + 1).collect();
    let image = factory_for(temp.path(), &disk, ImageTarget::MacosArm64);
    // A read-only cache file must not make the clone read-only.
    let cache = image.open_disk().unwrap();
    let mut mode = cache.metadata().unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o444);
    cache.set_permissions(mode).unwrap();
    let path = temp.path().join("install");
    let (root, lock) = owner(&path);
    let target = InstallationImageTarget::from_owned_installation(
        &root,
        &lock,
        Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
        ImageTarget::MacosArm64,
        &image.image().identity().state_compatibility_id,
        true,
    );
    let (_sender, closed) = tokio::sync::watch::channel(false);
    let installed = materialize_image(&image, &target, closed, |_| {})
        .await
        .unwrap();
    let private = File::options()
        .read(true)
        .write(true)
        .open(path.join(installed.disk_path()))
        .unwrap();
    assert_eq!(device_offset(&private), device_offset(&cache));
    private.write_all_at(b"guest", 0).unwrap();
    private.sync_all().unwrap();
    assert_ne!(device_offset(&private), device_offset(&cache));
    let mut cached = Vec::new();
    image.open_disk().unwrap().read_to_end(&mut cached).unwrap();
    assert_eq!(cached, disk);
    open_installed_image(&target).unwrap();
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn mac_interrupted_clone_is_rehashed_and_reused_on_retry() {
    let temp = tempfile::tempdir().unwrap();
    let disk: Vec<u8> = (0..256 * 1024u32).map(|i| (i % 241) as u8 + 1).collect();
    let image = factory_for(temp.path(), &disk, ImageTarget::MacosArm64);
    let path = temp.path().join("install");
    let (root, lock) = owner(&path);
    let mut target = InstallationImageTarget::from_owned_installation(
        &root,
        &lock,
        Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
        ImageTarget::MacosArm64,
        &image.image().identity().state_compatibility_id,
        true,
    );
    target.point = Some(Arc::new(|p| {
        if p == "copy-chunk" {
            Err(ImageError::Cancelled)
        } else {
            Ok(())
        }
    }));
    let (_sender, closed) = tokio::sync::watch::channel(false);
    assert_eq!(
        materialize_image(&image, &target, closed.clone(), |_| {})
            .await
            .err(),
        Some(ImageError::Cancelled)
    );
    let pending = path.join("image/.pending-1/os.disk");
    assert_eq!(std::fs::read(&pending).unwrap(), disk);
    target.point = None;
    let installed = materialize_image(&image, &target, closed, |_| {})
        .await
        .unwrap();
    let private = File::open(path.join(installed.disk_path())).unwrap();
    assert_eq!(
        device_offset(&private),
        device_offset(&image.open_disk().unwrap())
    );
}
#[path = "image_install_boundary_tests.rs"]
mod boundary;
#[path = "image_install_native_tests.rs"]
mod native;
#[path = "image_setup_tests.rs"]
mod setup;
