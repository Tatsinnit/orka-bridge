use super::*;
use crate::runtime::{
    image_install::InstallationImageTarget,
    image_types::{ImageTarget, LaunchMode},
    setup_adapter::prepare_development_image,
};
use std::sync::Arc;
use tokio::sync::watch;

fn boot_fixture(platform: DevImagePlatform) -> Fixture {
    let mut f = Fixture::new(platform);
    f.data[1] = vec![0; 4096];
    f.data[1][0x1fe..0x200].copy_from_slice(b"\x55\xaa");
    f.data[1][0x202..0x206].copy_from_slice(b"HdrS");
    f.data[1][0x38..0x3c].copy_from_slice(b"ARM\x64");
    f.data[2] = b"070701fixture".to_vec();
    let raw: &[u8] = match platform {
        DevImagePlatform::MacOsArm64 => include_bytes!("../../../images/testdata/v2/producer-macos.json"),
        DevImagePlatform::WindowsAmd64 => include_bytes!("../../../images/testdata/v2/producer-windows.json"),
    };
    let raw: serde_json::Value = serde_json::from_slice(raw).unwrap();
    let mut receipt = raw["directBoot"].clone();
    for (name, i) in [("kernel", 1), ("initrd", 2)] {
        receipt[name]["name"] = f.pins.files[i].name.into();
        receipt[name]["bytes"] = f.data[i].len().into();
        receipt[name]["sha256"] = format!("{:x}", Sha256::digest(&f.data[i])).into();
    }
    receipt["initrd"]["compression"] = "cpio-prefix".into();
    let mut provenance = receipt.as_object_mut().unwrap().remove("provenance").unwrap();
    provenance.as_object_mut().unwrap().remove("bootReceiptSha256");
    for (key, value) in provenance.as_object().unwrap() {
        receipt[key] = value.clone();
    }
    receipt["diskSha256"] = format!("{:x}", Sha256::digest(&f.data[3])).into();
    receipt["schemaVersion"] = "bridge-guest-boot-assets/v1".into();
    receipt["architecture"] = if platform == DevImagePlatform::MacOsArm64 { "arm64" } else { "amd64" }.into();
    receipt["publicationReady"] = false.into();
    receipt["bootVerified"] = false.into();
    receipt["initrdTools"] = serde_json::json!([]);
    f.data[0] = serde_json::to_vec(&receipt).unwrap();
    for i in 0..4 {
        fs::write(f.path(i), &f.data[i]).unwrap();
        f.pins.files[i].bytes = f.data[i].len() as u64;
        f.pins.files[i].sha256 = Sha256::digest(&f.data[i]).into();
    }
    f
}

// Returning only verified source handles, or projecting the wrong boot facts, cannot pass.
#[tokio::test]
async fn engineering_sources_commit_matching_private_sets_for_both_backends() {
    for platform in PLATFORMS {
        let f = boot_fixture(platform);
        let source = verify(&f.root(), &f.pins).unwrap();
        let (_sender, closed) = watch::channel(false);
        let prepared = prepare_development_image(&source, closed.clone()).await
            .expect("engineering source must prepare the existing installed-image handoff");
        assert_eq!(prepared.identity().state_compatibility_id, platform.state_compatibility_id());
        assert_eq!(prepared.identity().build_source_commit, platform.source_commit());
        assert!(prepared.identity().image_id.starts_with("engineering-"));
        assert_eq!(prepared.identity().manifest_sha256, format!("{:x}", Sha256::digest(&f.data[0])));
        let target_platform = if platform == DevImagePlatform::MacOsArm64 { ImageTarget::MacosArm64 } else { ImageTarget::WindowsAmd64 };
        assert_eq!(prepared.target(), target_platform);
        let path = f.temp.path().join("installation");
        fs::create_dir(&path).unwrap();
        let root = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
        let owner = Arc::new(std::fs::File::options().create_new(true).read(true).write(true).open(path.join("owner.lock")).unwrap());
        owner.try_lock().unwrap();
        let target = InstallationImageTarget::from_owned_installation(
            &root, &owner, bridge_runtime_protocol::Uuid::new_v4(), target_platform, platform.state_compatibility_id(), true,
        );
        let installed = prepared.materialize(&target, closed, |_| {}).await.unwrap();
        installed.revalidate(&target).unwrap();
        assert_eq!(installed.launch_mode(), if platform == DevImagePlatform::MacOsArm64 { LaunchMode::Uefi } else { LaunchMode::LinuxDirect });
        assert_eq!(fs::read(path.join(installed.disk_path())).unwrap(), f.data[3]);
        assert_eq!(fs::read(path.join(installed.kernel_path())).unwrap(), f.data[1]);
        assert_eq!(fs::read(path.join(installed.initrd_path())).unwrap(), f.data[2]);
        fs::write(path.join(installed.disk_path()), b"guest writes its private disk").unwrap();
        assert_eq!(fs::read(f.path(3)).unwrap(), f.data[3]);
    }
}

#[tokio::test]
async fn changed_engineering_receipt_or_payload_cannot_select_an_image() {
    use crate::runtime::image_types::ImageError;
    for role in 0..4 {
        let f = boot_fixture(DevImagePlatform::WindowsAmd64);
        let source = verify(&f.root(), &f.pins).unwrap();
        let mut bytes = f.data[role].clone();
        if role == 0 {
            let at = bytes.windows(b"6.1.0-fixture-amd64".len()).position(|v| v == b"6.1.0-fixture-amd64").unwrap();
            bytes[at] = b'7'; // Still-valid metadata with unchanged payloads.
        } else {
            let last = bytes.len() - 1; bytes[last] ^= 1;
        }
        fs::write(f.path(role), bytes).unwrap();
        let (_sender, closed) = watch::channel(false);
        let prepared = prepare_development_image(&source, closed.clone()).await;
        let error = if let Ok(prepared) = prepared {
            let path = f.temp.path().join("installation");
            fs::create_dir(&path).unwrap();
            let root = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
            let owner = Arc::new(std::fs::File::options().create_new(true).read(true).write(true).open(path.join("owner.lock")).unwrap());
            owner.try_lock().unwrap();
            let target = InstallationImageTarget::from_owned_installation(
                &root, &owner, bridge_runtime_protocol::Uuid::new_v4(), ImageTarget::WindowsAmd64, DevImagePlatform::WindowsAmd64.state_compatibility_id(), true,
            );
            let result = prepared.materialize(&target, closed, |_| {}).await;
            assert!(!path.join("image-binding.json").exists(), "changed source role {role} selected an image");
            result.err()
        } else { prepared.err() };
        assert_eq!(error, Some(ImageError::Integrity), "changed source role {role}");
    }
}

#[tokio::test]
async fn engineering_binding_does_not_restore_release_compatibility_or_creation() {
    for platform in PLATFORMS {
        check_retained_engineering_binding(platform).await;
    }
}

async fn check_retained_engineering_binding(platform: DevImagePlatform) {
    use crate::runtime::{image_install::open_installed_image, image_types::ImageError};
    let f = boot_fixture(platform);
    let image_target = match platform {
        DevImagePlatform::MacOsArm64 => ImageTarget::MacosArm64,
        DevImagePlatform::WindowsAmd64 => ImageTarget::WindowsAmd64,
    };
    let source = verify(&f.root(), &f.pins).unwrap();
    let (_sender, closed) = watch::channel(false);
    let prepared = prepare_development_image(&source, closed.clone()).await.unwrap();
    let path = f.temp.path().join("installation");
    fs::create_dir(&path).unwrap();
    let root = Dir::open_ambient_dir(&path, ambient_authority()).unwrap();
    let owner = Arc::new(std::fs::File::options().create_new(true).read(true).write(true).open(path.join("owner.lock")).unwrap());
    owner.try_lock().unwrap();
    let id = bridge_runtime_protocol::Uuid::new_v4();
    let target = InstallationImageTarget::from_owned_installation(
        &root, &owner, id, image_target, platform.state_compatibility_id(), true,
    );
    prepared.materialize(&target, closed.clone(), |_| {}).await.unwrap();
    let release = InstallationImageTarget::from_owned_installation(
        &root, &owner, id, image_target, "release-fixture-v1", false,
    );
    assert!(matches!(open_installed_image(&release), Err(ImageError::RetainedImage)));
    let no_creation = InstallationImageTarget::from_owned_installation(
        &root, &owner, id, image_target, platform.state_compatibility_id(), false,
    );
    open_installed_image(&no_creation).unwrap();
    let old = InstallationImageTarget::from_owned_installation(
        &root, &owner, id, image_target, "engineering-d471f5eb", false,
    );
    let binding = fs::read(path.join("image-binding.json")).unwrap();
    assert!(matches!(open_installed_image(&old), Err(ImageError::RetainedImage)));
    assert_eq!(fs::read(path.join("image-binding.json")).unwrap(), binding);
    // Refused old bindings retain their bytes, including their compatibility id.
    let previous_platform_id = match platform {
        DevImagePlatform::MacOsArm64 => "engineering-d86ebac3",
        DevImagePlatform::WindowsAmd64 => "engineering-d86ebac3",
    };
    for compatibility in ["engineering-d471f5eb", "engineering-10d23420", previous_platform_id] {
        let mut previous: serde_json::Value = serde_json::from_slice(&binding).unwrap();
        previous["identity"]["stateCompatibilityId"] = compatibility.into();
        let previous = serde_json::to_vec(&previous).unwrap();
        fs::write(path.join("image-binding.json"), &previous).unwrap();
        assert!(matches!(open_installed_image(&target), Err(ImageError::RetainedImage)));
        assert_eq!(fs::read(path.join("image-binding.json")).unwrap(), previous);
    }
    fs::write(path.join("image-binding.json"), binding).unwrap();
    let prepared = prepare_development_image(&source, closed.clone()).await.unwrap();
    assert!(matches!(prepared.materialize(&no_creation, closed.clone(), |_| {}).await, Err(ImageError::RetainedImage)));
    let (_sender, cancelled) = watch::channel(true);
    assert!(matches!(prepare_development_image(&source, cancelled).await, Err(ImageError::Cancelled)));
}

#[tokio::test]
#[ignore = "explicit frozen source directories required; no VM is started"]
async fn frozen_windows_source_materializes_actual_pinned_bytes() {
    let boot = std::env::var_os("BRIDGE_TEST_BOOT_DIR").expect("test boot directory");
    let disk = std::env::var_os("BRIDGE_TEST_DISK_DIR").expect("test disk directory");
    let destination = std::env::var_os("BRIDGE_TEST_INSTALL_DIR").expect("fresh test installation directory");
    let boot = Dir::open_ambient_dir(boot, ambient_authority()).unwrap();
    let disk = Dir::open_ambient_dir(disk, ambient_authority()).unwrap();
    let source = VerifiedDevImage {
        platform: WINDOWS_PINS.platform, pins: WINDOWS_PINS.files,
        files: [
            verified_file(&boot, &WINDOWS_PINS.files[0]).unwrap(),
            verified_file(&boot, &WINDOWS_PINS.files[1]).unwrap(),
            verified_file(&boot, &WINDOWS_PINS.files[2]).unwrap(),
            verified_file(&disk, &WINDOWS_PINS.files[3]).unwrap(),
        ],
    };
    let (_sender, closed) = watch::channel(false);
    let prepared = prepare_development_image(&source, closed.clone()).await.unwrap();
    let path = std::path::Path::new(&destination);
    fs::create_dir(path).unwrap();
    let root = Dir::open_ambient_dir(path, ambient_authority()).unwrap();
    let owner = Arc::new(std::fs::File::options().create_new(true).read(true).write(true).open(path.join("owner.lock")).unwrap());
    owner.try_lock().unwrap();
    let target = InstallationImageTarget::from_owned_installation(
        &root, &owner, bridge_runtime_protocol::Uuid::new_v4(), ImageTarget::WindowsAmd64, DevImagePlatform::WindowsAmd64.state_compatibility_id(), true,
    );
    let installed = prepared.materialize(&target, closed, |_| {}).await.unwrap();
    installed.revalidate(&target).unwrap();
    assert_eq!(installed.initial_os_bytes(), 3765960704);
    assert_eq!(installed.initial_os_sha256(), "226cfa6e3f7ffc3cde467a3c8e9498aeb1db2aeac68e60c3f27e8f086528850c");
    assert_eq!(installed.kernel_release(), "6.1.0-54-cloud-amd64");
}
