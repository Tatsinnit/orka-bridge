use super::*;
use crate::local_vm::native::image::{ImageSource, Predecessor, ReleaseImage};
use crate::runtime as fixture_runtime;
use crate::runtime::image_types::*;
use crate::runtime::types::BackendKind;
use sha2::{Digest, Sha256};
mod fixture {
    include!("../tests/support/runtime_image_fixture.rs");
}
#[path = "workspace_upgrade_crash_tests.rs"]
mod process;
#[path = "workspace_upgrade_restore_tests.rs"]
mod restore;
struct Scene {
    _temp: tempfile::TempDir,
    data: PathBuf,
    local: PathBuf,
    source: ImageSource,
    target: Option<PreparedImage>,
    source_id: Uuid,
}
impl Scene {
    fn new(target: ImageTarget) -> Self {
        let temp = match std::env::var_os("BRIDGE_UPGRADE_CRASH_HOME") {
            Some(path) => tempfile::tempdir_in(path).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        let data = temp.path().join("roaming");
        let local = temp.path().join("local");
        fs::create_dir_all(data.join("setup-v1")).unwrap();
        fs::create_dir(&local).unwrap();
        let old_dir = temp.path().join("old-cache");
        let new_dir = temp.path().join("new-cache");
        fs::create_dir(&old_dir).unwrap();
        fs::create_dir(&new_dir).unwrap();
        let old =
            fixture::verified_with_identity(&old_dir, target, Some("old-state"), Some("old-image"));
        let new =
            fixture::verified_with_identity(&new_dir, target, Some("new-state"), Some("new-image"));
        fn profile(image: &VerifiedImageSet) -> ReleaseImage {
            let resolved = image.image();
            ReleaseImage {
                pin: ImagePin {
                    release_id: "fixture".into(),
                    manifest_url: "https://fixture.invalid/manifest.json".into(),
                    manifest_sha256: resolved.identity().manifest_sha256.clone(),
                    accepted_schema_sha256: format!(
                        "{:x}",
                        Sha256::digest(include_bytes!("../../../images/manifest.schema.json"))
                    ),
                    target: resolved.target(),
                    asset_base: "https://fixture.invalid/".into(),
                    redirect_origins: vec![],
                    limits: TransferLimits {
                        compressed_bytes: 8192,
                        disk_bytes: 1024 * 1024,
                        kernel_bytes: 8192,
                        initrd_bytes: 8192,
                    },
                },
                manifest: resolved.manifest_bytes().to_vec(),
                image_id: resolved.identity().image_id.clone(),
                state_compatibility_id: resolved.identity().state_compatibility_id.clone(),
                predecessors: vec![],
            }
        }
        let mut next = profile(&new);
        next.predecessors.push(Predecessor {
            source: Box::new(profile(&old)),
            app_releases: vec!["v0.1.2".into()],
            target_manifests: vec![next.pin.manifest_sha256.clone()],
        });
        let backend = match target {
            ImageTarget::MacosArm64 => BackendKind::Apple,
            ImageTarget::WindowsAmd64 => BackendKind::Qemu,
        };
        let journal = Journal::create(&local.join("local-vm-v1"), backend, vec![1]).unwrap();
        let source_id = journal.record().installation_id;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (_send, closed) = watch::channel(false);
        runtime
            .block_on(PreparedImage::from_verified(old).unwrap().materialize(
                &journal.image_target("old-state").unwrap(),
                closed,
                |_| {},
            ))
            .unwrap();
        fs::write(local.join("local-vm-v1/efi-stand-in"), b"firmware-state").unwrap();
        let slot = Uuid::new_v4();
        fs::write(data.join("setup-v1/model-v1"), serde_json::to_vec(&serde_json::json!({"slot":slot,"installation":source_id,"phase":"stored","nonce":null,"connection":null})).unwrap()).unwrap();
        fs::write(
            data.join("setup-v1/model-account-v1"),
            serde_json::to_vec(&serde_json::json!({"slot":slot,"login":"stand-in"})).unwrap(),
        )
        .unwrap();
        drop(journal);
        Self {
            _temp: temp,
            data,
            local,
            source: ImageSource::Pinned {
                profile: Box::new(next),
                host: HostTarget {
                    target,
                    host_version: "99.0".into(),
                    app_version: "1.0.0".into(),
                },
            },
            target: Some(PreparedImage::from_verified(new).unwrap()),
            source_id,
        }
    }
    fn replace(&mut self, stop_at: Option<&str>) -> Result<UpgradeStatus, RuntimeError> {
        let mut owner = WorkspaceUpgrade::open(&self.data, &self.local)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (_send, closed) = watch::channel(false);
        owner.replace_with(
            &self.source,
            vec![2],
            &closed,
            runtime.handle(),
            || self.target.take().ok_or(RuntimeError::Unavailable),
            |point| {
                process::crash_at(&self.data, &self.local, self.source_id, point);
                if Some(point) == stop_at {
                    Err(RuntimeError::Closed)
                } else {
                    Ok(())
                }
            },
            available_space,
        )
    }
}
#[test]
fn every_switch_boundary_retains_exact_source_and_never_grants_reopen_authority() {
    for target in [ImageTarget::MacosArm64, ImageTarget::WindowsAmd64] {
        for point in [
            "acquired",
            "intent",
            "model-archived",
            "account-archived",
            "source-retained",
            "destination-created",
            "destination-recorded",
            "image-prepared",
        ] {
            let mut s = Scene::new(target);
            let root = s.local.join("local-vm-v1");
            let before = directory_identity(&root).unwrap();
            assert!(s.replace(Some(point)).is_err(), "{point}");
            let owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
            let status = owner.recover().unwrap_or(UpgradeStatus::Conflict);
            if point == "acquired" {
                assert_eq!(status, UpgradeStatus::None);
                assert_eq!(directory_identity(&root).unwrap(), before);
            } else {
                assert_ne!(status, UpgradeStatus::None);
                assert!(admit(&root).is_err());
                let intent = owner.read_intent().unwrap().unwrap();
                let retained = s.local.join(&intent.retained);
                let a = if retained.exists() {
                    retained
                } else {
                    root.clone()
                };
                assert_eq!(directory_identity(&a).unwrap(), before);
                assert_eq!(fs::read(a.join("efi-stand-in")).unwrap(), b"firmware-state");
                assert_eq!(read_record(&a).unwrap().installation_id, s.source_id);
                if let Some(b) = intent.destination {
                    assert_ne!(b.installation, s.source_id);
                    let journal = Journal::open(&root).unwrap();
                    assert!(journal.require_fresh_creation().is_err());
                }
            }
        }
    }
}
#[test]
fn unexpected_canonical_after_rename_is_preserved_and_refused() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    assert!(s.replace(Some("source-retained")).is_err());
    let root = s.local.join("local-vm-v1");
    let foreign = Journal::create(&root, BackendKind::Qemu, vec![8]).unwrap();
    let id = foreign.record().installation_id;
    drop(foreign);
    let owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    assert!(owner.recover().is_err());
    assert_eq!(read_record(&root).unwrap().installation_id, id);
}
#[test]
fn malformed_metadata_and_offline_acquisition_leave_source_untouched() {
    for malformed in [false, true] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        if malformed {
            fs::write(s.data.join("setup-v1/model-v1"), b"{\"futureSchema\":2}").unwrap();
        } else {
            s.target = None;
        }
        let before = fs::read(s.data.join("setup-v1/model-v1")).unwrap();
        let root_id = directory_identity(&s.local.join("local-vm-v1")).unwrap();
        assert!(s.replace(None).is_err());
        assert_eq!(fs::read(s.data.join("setup-v1/model-v1")).unwrap(), before);
        assert_eq!(
            directory_identity(&s.local.join("local-vm-v1")).unwrap(),
            root_id
        );
        assert_eq!(
            WorkspaceUpgrade::open(&s.data, &s.local)
                .unwrap()
                .inspect()
                .unwrap(),
            UpgradeStatus::None
        );
    }
}
#[tokio::test]
async fn recovery_blocked_vm_rejects_direct_start_reset_and_connect_without_touching_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let setup = std::sync::Arc::new(crate::setup::SetupStore::new(Some(temp.path().into())));
    let vm = crate::local_vm::LocalVm::recovery_blocked();
    vm.start();
    tokio::task::yield_now().await;
    assert_eq!(vm.status(), crate::local_vm::LocalVmStatus::Failed);
    assert!(vm.reset_model(setup.clone()).await.is_err());
    assert!(vm.connect_model(setup.clone()).await.is_err());
    assert!(vm.remove_model(setup).await.is_err());
    assert!(!temp.path().join("setup-v1").exists());
}
#[test]
fn cancellation_and_no_space_leave_source_and_both_metadata_records_unchanged() {
    for no_space in [false, true] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        let root = s.local.join("local-vm-v1");
        let before = installation(&root).unwrap();
        let model = fs::read(s.data.join("setup-v1/model-v1")).unwrap();
        let account = fs::read(s.data.join("setup-v1/model-account-v1")).unwrap();
        let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (sender, closed) = watch::channel(false);
        let result = owner.replace_with(
            &s.source,
            vec![2],
            &closed,
            runtime.handle(),
            || {
                if !no_space {
                    sender.send_replace(true);
                }
                Ok(s.target.take().unwrap())
            },
            |_| Ok(()),
            |_| Ok(0),
        );
        assert!(if no_space {
            matches!(result, Err(RuntimeError::Image(ImageError::NoSpace)))
        } else {
            matches!(result, Err(RuntimeError::Closed))
        });
        assert_eq!(installation(&root).unwrap(), before);
        assert_eq!(fs::read(s.data.join("setup-v1/model-v1")).unwrap(), model);
        assert_eq!(
            fs::read(s.data.join("setup-v1/model-account-v1")).unwrap(),
            account
        );
        assert_eq!(owner.inspect().unwrap(), UpgradeStatus::None);
    }
}
#[cfg(windows)]
#[test]
fn replacement_retains_a_real_stale_windows_control_socket_without_reading_it() {
    use std::{
        mem::{size_of, zeroed},
        os::windows::{
            fs::OpenOptionsExt,
            io::{FromRawSocket, OwnedSocket},
        },
    };
    use windows_sys::Win32::{
        Networking::WinSock::*,
        Storage::FileSystem::{FILE_READ_ATTRIBUTES, READ_CONTROL},
    };
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let root = s.local.join("local-vm-v1");
    let endpoint = crate::runtime::windows::control::ControlEndpoint::create(&root).unwrap();
    let path = endpoint.path().to_owned();
    let relative = path.strip_prefix(&root).unwrap().to_owned();
    let mut data = unsafe { zeroed() };
    assert_eq!(unsafe { WSAStartup(0x0202, &mut data) }, 0);
    let raw = unsafe {
        WSASocketW(
            AF_UNIX as i32,
            SOCK_STREAM,
            0,
            std::ptr::null(),
            0,
            WSA_FLAG_NO_HANDLE_INHERIT,
        )
    };
    assert_ne!(raw, INVALID_SOCKET);
    let socket = unsafe { OwnedSocket::from_raw_socket(raw as _) };
    let mut address: SOCKADDR_UN = unsafe { zeroed() };
    address.sun_family = AF_UNIX;
    for (out, byte) in address
        .sun_path
        .iter_mut()
        .zip(path.to_str().unwrap().bytes())
    {
        *out = byte as i8;
    }
    assert_eq!(
        unsafe {
            bind(
                raw,
                (&address as *const SOCKADDR_UN).cast(),
                size_of::<SOCKADDR_UN>() as i32,
            )
        },
        0
    );
    drop(socket);
    unsafe {
        WSACleanup();
    }
    drop(endpoint);
    let metadata = |path: &Path| {
        OpenOptions::new()
            .access_mode(READ_CONTROL | FILE_READ_ATTRIBUTES)
            .share_mode(3)
            .custom_flags(0x02200000)
            .open(path)
            .unwrap()
    };
    let before = file_identity(&metadata(&path)).unwrap();
    assert_eq!(s.replace(None).unwrap(), UpgradeStatus::Fresh);
    let owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    let retained = s.local.join(intent.retained).join(relative);
    assert_eq!(file_identity(&metadata(&retained)).unwrap(), before);
}
#[cfg(windows)]
#[test]
fn ordinary_files_in_native_socket_slots_are_refused_before_archival() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let root = s.local.join("local-vm-v1");
    let endpoint = crate::runtime::windows::control::ControlEndpoint::create(&root).unwrap();
    fs::write(endpoint.path(), b"not an AF_UNIX endpoint").unwrap();
    drop(endpoint);
    let before = fs::read(s.data.join("setup-v1/model-v1")).unwrap();
    assert!(matches!(s.replace(None), Err(RuntimeError::Io(_))));
    assert_eq!(fs::read(s.data.join("setup-v1/model-v1")).unwrap(), before);
    assert_eq!(
        WorkspaceUpgrade::open(&s.data, &s.local)
            .unwrap()
            .inspect()
            .unwrap(),
        UpgradeStatus::None
    );
}
#[test]
fn failed_upgrade_parent_flush_refuses_before_archiving_source_metadata() {
    let s = Scene::new(ImageTarget::WindowsAmd64);
    let model = fs::read(s.data.join("setup-v1/model-v1")).unwrap();
    let result = WorkspaceUpgrade::open_with_sync(&s.data, &s.local, |_| Err(ImageError::NoSpace));
    assert!(matches!(
        result,
        Err(RuntimeError::Image(ImageError::NoSpace))
    ));
    assert_eq!(fs::read(s.data.join("setup-v1/model-v1")).unwrap(), model);
    assert!(s.local.join("local-vm-v1").exists());
    assert!(!s.local.join(STORE).join("intent.json").exists());
}
#[test]
fn native_replace_uses_verified_cache_and_keeps_only_live_creation_authority() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    drop(s.target.take());
    fs::rename(
        s._temp.path().join("new-cache/cache"),
        s.local.join("image-cache-v1"),
    )
    .unwrap();
    let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (_sender, closed) = watch::channel(false);
    assert_eq!(
        owner
            .replace(&s.source, vec![2], &closed, runtime.handle())
            .unwrap(),
        UpgradeStatus::Fresh
    );
    let journal = owner.take_destination().unwrap();
    assert!(journal.require_fresh_creation().is_ok());
    assert_ne!(journal.record().installation_id, s.source_id);
    drop(journal);
    drop(owner);
    assert_eq!(
        WorkspaceUpgrade::open(&s.data, &s.local)
            .unwrap()
            .recover()
            .unwrap(),
        UpgradeStatus::Incomplete
    );
}
#[test]
fn recovery_refuses_changed_retained_binding_instead_of_trusting_directory_alone() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    s.replace(None).unwrap();
    let owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    fs::write(
        s.local.join(intent.retained).join("image-binding.json"),
        b"corrupt binding",
    )
    .unwrap();
    assert!(matches!(
        owner.recover(),
        Err(RuntimeError::Image(_)) | Err(RuntimeError::RetainedState)
    ));
}
#[test]
fn owner_excludes_second_switch_and_never_overwrites_previous_retention() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    let owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    assert!(matches!(
        WorkspaceUpgrade::open(&s.data, &s.local),
        Err(RuntimeError::Busy)
    ));
    drop(owner);
    assert!(s.replace(Some("source-retained")).is_err());
    let owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    assert_eq!(
        read_record(&s.local.join(intent.retained))
            .unwrap()
            .installation_id,
        s.source_id
    );
    assert!(s.replace(None).is_err());
}
