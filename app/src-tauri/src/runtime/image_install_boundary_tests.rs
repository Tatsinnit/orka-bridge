use super::*;

pub(super) struct Scene {
    temp: tempfile::TempDir,
    pub(super) image: VerifiedImageSet,
    root: Dir,
    lock: Arc<File>,
    id: Uuid,
}
impl Scene {
    pub(super) fn new() -> Self {
        Self::with_disk(b"private synthetic disk")
    }
    fn with_disk(disk: &[u8]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let image = factory_with_disk(temp.path(), disk);
        let (root, lock) = owner(&temp.path().join("install"));
        Self {
            temp,
            image,
            root,
            lock,
            id: Uuid::parse_str("12345678-1234-4234-8234-123456789abc").unwrap(),
        }
    }
    fn path(&self) -> std::path::PathBuf {
        self.temp.path().join("install")
    }
    pub(super) fn target(&self) -> InstallationImageTarget<'_> {
        InstallationImageTarget::from_owned_installation(
            &self.root,
            &self.lock,
            self.id,
            ImageTarget::WindowsAmd64,
            &self.image.image().identity().state_compatibility_id,
            true,
        )
    }
    pub(super) async fn install(&self) -> InstalledImageSet {
        let (_sender, closed) = watch::channel(false);
        materialize_image(&self.image, &self.target(), closed, |_| {})
            .await
            .unwrap()
    }
}
#[tokio::test]
async fn source_mutation_after_cache_verification_cannot_be_bound() {
    for name in ["os.disk", "asset-1", "asset-2"] {
        let scene = Scene::new();
        scene
            .image
            .directory
            .write(name, b"changed after cache verification")
            .unwrap();
        let (_sender, closed) = watch::channel(false);
        assert_eq!(
            materialize_image(&scene.image, &scene.target(), closed, |_| {})
                .await
                .err(),
            Some(ImageError::Integrity),
            "{name}"
        );
        assert!(!scene.path().join("image-binding.json").exists());
    }
}
#[tokio::test]
async fn actual_destination_bytes_are_checked_after_the_copy() {
    for name in FILES {
        let scene = Scene::new();
        let path = scene.path();
        let mut target = scene.target();
        let event = format!("written-{name}");
        let leaf = name.to_string();
        target.point = Some(Arc::new(move |point| {
            if point == event {
                std::fs::write(
                    path.join("image/.pending-1").join(&leaf),
                    b"successful copy, corrupted destination",
                )
                .unwrap();
            }
            Ok(())
        }));
        let (_sender, closed) = watch::channel(false);
        assert_eq!(
            materialize_image(&scene.image, &target, closed, |_| {})
                .await
                .err(),
            Some(ImageError::Integrity)
        );
        assert!(!scene.path().join("image-binding.json").exists());
    }
}
#[tokio::test]
async fn every_publication_interruption_has_a_safe_exact_owned_retry() {
    for point in [
        "stage-flushed",
        "generation-published",
        "generation-verified",
        "binding-flushed",
        "binding-published",
    ] {
        let scene = Scene::new();
        let mut target = scene.target();
        target.point = Some(Arc::new(move |p| {
            if p == point {
                Err(ImageError::Storage(std::io::ErrorKind::Interrupted))
            } else {
                Ok(())
            }
        }));
        let (_sender, closed) = watch::channel(false);
        assert_eq!(
            materialize_image(&scene.image, &target, closed.clone(), |_| {})
                .await
                .err(),
            Some(ImageError::Storage(std::io::ErrorKind::Interrupted)),
            "{point}"
        );
        assert_eq!(
            open_installed_image(&scene.target()).is_ok(),
            point == "binding-published",
            "{point}"
        );
        target.point = None;
        materialize_image(&scene.image, &target, closed, |_| {})
            .await
            .unwrap();
        assert!(scene.path().join("image-binding.json").exists());
    }
}
#[tokio::test]
async fn binding_publication_observes_cancellation_at_the_last_boundary() {
    let scene = Scene::new();
    let mut target = scene.target();
    let (sender, closed) = watch::channel(false);
    target.point = Some(Arc::new(move |p| {
        if p == "binding-flushed" {
            sender.send(true).unwrap();
        }
        Ok(())
    }));
    assert_eq!(
        materialize_image(&scene.image, &target, closed, |_| {})
            .await
            .err(),
        Some(ImageError::Cancelled)
    );
    assert!(
        !scene.path().join("image-binding.json").exists(),
        "cancellation before selector rename must not publish"
    );
}
#[tokio::test]
async fn retained_generation_metadata_must_match_the_requested_image() {
    let scene = Scene::new();
    let mut target = scene.target();
    target.point = Some(Arc::new(|p| {
        if p == "generation-published" {
            Err(ImageError::Storage(std::io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    }));
    let (_sender, closed) = watch::channel(false);
    materialize_image(&scene.image, &target, closed.clone(), |_| {})
        .await
        .unwrap_err();
    let path = scene.path().join("image/1/binding.json");
    let mut binding: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    binding["identity"]["manifestSha256"] = "b".repeat(64).into();
    std::fs::write(path, serde_json::to_vec(&binding).unwrap()).unwrap();
    assert_eq!(
        materialize_image(&scene.image, &scene.target(), closed, |_| {})
            .await
            .err(),
        Some(ImageError::RetainedImage)
    );
    assert!(!scene.path().join("image-binding.json").exists());
}
#[test]
fn a_cancelled_reuse_hash_does_not_truncate_a_complete_private_copy() {
    let scene = Scene::new();
    let stage = directory(&scene.root, std::ffi::OsStr::new("stage")).unwrap();
    stage.write("os.disk", b"keep completed bytes").unwrap();
    let (_sender, closed) = watch::channel(true);
    let (events, _receive) = mpsc::channel(1);
    let context = Context {
        root: scene.root.try_clone().unwrap(),
        owner: Arc::clone(&scene.lock),
        id: scene.id,
        target: ImageTarget::WindowsAmd64,
        compatibility: scene
            .image
            .image()
            .identity()
            .state_compatibility_id
            .clone(),
        creation_allowed: true,
        closed,
        events,
        point: None,
        available: None,
    };
    let data = b"keep completed bytes";
    let result = copy(
        &context,
        &stage,
        "os.disk",
        scene.image.open_disk().unwrap(),
        (data.len() as u64, &hash(data)),
        &mut 0,
        data.len() as u64,
    );
    assert_eq!(result.err(), Some(ImageError::Cancelled));
    assert_eq!(stage.read("os.disk").unwrap(), data);
}
#[tokio::test]
async fn partial_writes_retain_owned_work_without_restoring_creation_authority() {
    let data = vec![42; 131072];
    let scene = Scene::with_disk(&data);
    let mut target = scene.target();
    target.point = Some(Arc::new(|p| {
        if p == "copy-chunk" || p == "clone" {
            Err(ImageError::Storage(std::io::ErrorKind::Other))
        } else {
            Ok(())
        }
    }));
    let (_sender, closed) = watch::channel(false);
    assert!(
        materialize_image(&scene.image, &target, closed.clone(), |_| {})
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::metadata(scene.path().join("image/.pending-1/os.disk"))
            .unwrap()
            .len(),
        65536
    );
    target.point = None;
    target.creation_allowed = false;
    assert_eq!(
        materialize_image(&scene.image, &target, closed.clone(), |_| {})
            .await
            .err(),
        Some(ImageError::RetainedImage)
    );
    assert!(open_installed_image(&target).is_err());
    target.creation_allowed = true;
    let installed = materialize_image(&scene.image, &target, closed, |_| {})
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(scene.path().join(installed.disk_path())).unwrap(),
        data
    );
}
#[tokio::test]
async fn cancelled_prefix_is_compared_and_resume_does_not_recopy_it() {
    for corrupt in [false, true] {
        let data = vec![42; 196608];
        let scene = Scene::with_disk(&data);
        let mut target = scene.target();
        target.point = Some(Arc::new(|p| {
            if p == "copy-chunk" || p == "clone" {
                Err(ImageError::Cancelled)
            } else {
                Ok(())
            }
        }));
        let (_sender, closed) = watch::channel(false);
        assert_eq!(
            materialize_image(&scene.image, &target, closed.clone(), |_| {})
                .await
                .err(),
            Some(ImageError::Cancelled)
        );
        let path = scene.path().join("image/.pending-1/os.disk");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 65536);
        if corrupt {
            std::fs::write(&path, vec![99; 65536]).unwrap();
        }
        let chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = chunks.clone();
        target.point = Some(Arc::new(move |p| {
            if p == "copy-chunk" {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        }));
        let installed = materialize_image(&scene.image, &target, closed, |_| {})
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(scene.path().join(installed.disk_path())).unwrap(),
            data
        );
        // Two disk chunks remain for a valid prefix, plus one each for boot files.
        assert_eq!(
            chunks.load(std::sync::atomic::Ordering::SeqCst),
            if corrupt { 5 } else { 4 }
        );
    }
}
#[tokio::test]
async fn immutable_boot_corruption_and_same_bytes_replacement_are_rejected() {
    for name in ["os.kernel", "os.initrd"] {
        let scene = Scene::new();
        let installed = scene.install().await;
        let path = scene.path().join("image/1").join(name);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"bad boot bytes").unwrap();
        assert_eq!(
            open_installed_image(&scene.target()).err().unwrap(),
            ImageError::Integrity
        );
        std::fs::write(&path, &bytes).unwrap();
        let keep = File::open(&path).unwrap();
        let replacement = path.with_extension("replacement");
        std::fs::write(&replacement, bytes).unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(
            installed.revalidate(&scene.target()),
            Err(ImageError::Integrity)
        );
        drop(keep);
    }
}
#[tokio::test]
async fn target_uuid_platform_and_state_compatibility_are_not_interchangeable() {
    let scene = Scene::new();
    scene.install().await;
    let mut target = scene.target();
    target.id = Uuid::new_v4();
    assert!(open_installed_image(&target).is_err());
    target = scene.target();
    target.target = ImageTarget::MacosArm64;
    assert!(open_installed_image(&target).is_err());
    target = scene.target();
    target.compatibility = "other-state";
    assert!(open_installed_image(&target).is_err());
    target = scene.target();
    target.id = Uuid::nil();
    assert!(open_installed_image(&target).is_err());
}
#[tokio::test]
async fn malformed_or_legacy_bindings_are_not_migrated() {
    let scene = Scene::new();
    assert_eq!(
        open_installed_image(&scene.target()).err().unwrap(),
        ImageError::RetainedImage
    );
    let installed = scene.install().await;
    let path = scene.path().join("image-binding.json");
    let bytes = std::fs::read(&path).unwrap();
    for bad in [
        b"{".to_vec(),
        b"{\"version\":1,\"version\":1}".to_vec(),
        vec![b' '; MAX_BINDING as usize + 1],
    ] {
        std::fs::write(&path, bad).unwrap();
        assert!(open_installed_image(&scene.target()).is_err());
    }
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for (field, bad) in [
        ("version", 2.into()),
        ("imageGeneration", 99.into()),
        ("extra", true.into()),
    ] {
        let mut v = value.clone();
        v[field] = bad;
        std::fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(open_installed_image(&scene.target()).is_err());
    }
    value["boot"]["kernel"]["extra"] = true.into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(installed.revalidate(&scene.target()).is_err());
    std::fs::write(path, bytes).unwrap();
}
#[tokio::test]
async fn space_quotes_are_checked_before_payload_and_overflow_is_refused() {
    let scene = Scene::new();
    let mut target = scene.target();
    target.available = Some(0);
    let (_sender, closed) = watch::channel(false);
    assert_eq!(
        materialize_image(&scene.image, &target, closed, |_| {})
            .await
            .err(),
        Some(ImageError::NoSpace)
    );
    assert!(
        std::fs::metadata(scene.path().join("image/.pending-1/os.disk"))
            .map_or(true, |m| m.len() == 0)
    );
    let mut image = scene.image.clone();
    image.image.disk_bytes = u64::MAX;
    let (_sender, closed) = watch::channel(false);
    assert_eq!(
        materialize_image(&image, &scene.target(), closed, |_| {})
            .await
            .err(),
        Some(ImageError::NoSpace)
    );
}
#[tokio::test]
async fn cloned_handoff_metadata_does_not_own_the_installation_lock_or_factory_bytes() {
    let scene = Scene::new();
    let bytes = b"record with independent wire generation 99";
    scene.root.write("record.json", bytes).unwrap();
    let installed = scene.install().await;
    let retained = installed.clone();
    assert_ne!(
        *installed.os_identity(),
        fid(&scene.image.open_disk().unwrap()).unwrap()
    );
    std::fs::write(
        scene.path().join(installed.disk_path()),
        b"mutable private OS",
    )
    .unwrap();
    let mut source = Vec::new();
    scene
        .image
        .open_disk()
        .unwrap()
        .read_to_end(&mut source)
        .unwrap();
    assert_eq!(source, b"private synthetic disk");
    assert_eq!(scene.root.read("record.json").unwrap(), bytes);
    assert_eq!(retained.image_generation(), 1);
    let Scene {
        temp,
        image,
        root,
        lock,
        id: _,
    } = scene;
    drop(root);
    drop(lock);
    drop(image);
    let owner = File::options()
        .read(true)
        .write(true)
        .open(temp.path().join("install/owner.lock"))
        .unwrap();
    owner
        .try_lock()
        .expect("metadata clones must not retain installation ownership");
    drop(retained);
}
#[tokio::test]
async fn fresh_copy_quotes_all_missing_files_before_payload_work() {
    let scene = Scene::new();
    let total = scene.image.image.disk_bytes()
        + scene
            .image
            .image
            .assets()
            .iter()
            .filter(|a| !matches!(a.role(), ImageAssetRole::Chunk(_)))
            .map(|a| a.bytes())
            .sum::<u64>();
    let mut target = scene.target();
    target.available = Some(total - 1);
    let events = std::sync::Mutex::new(Vec::new());
    let (_sender, closed) = watch::channel(false);
    let result = materialize_image(&scene.image, &target, closed, |e| {
        events.lock().unwrap().push(e)
    })
    .await;
    assert_eq!(result.err(), Some(ImageError::NoSpace));
    assert!(!events
        .into_inner()
        .unwrap()
        .iter()
        .any(|e| matches!(e,ImageProgress::Materializing{bytes,..} if *bytes>0)));
}
#[tokio::test]
async fn complete_retained_private_copies_do_not_need_the_fresh_copy_budget() {
    let scene = Scene::new();
    let mut target = scene.target();
    target.point = Some(Arc::new(|p| {
        if p == "stage-flushed" {
            Err(ImageError::Storage(std::io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    }));
    let (_sender, closed) = watch::channel(false);
    materialize_image(&scene.image, &target, closed.clone(), |_| {})
        .await
        .unwrap_err();
    target.point = None;
    target.available = Some(0);
    materialize_image(&scene.image, &target, closed, |_| {})
        .await
        .expect("all private bytes already exist and are rehashed");
}
#[path = "image_install_review_tests.rs"]
mod review;
#[path = "image_install_safety_tests.rs"]
mod safety;
fn rewrite(path: &std::path::Path) {
    use std::io::Write;
    // In place, so the file keeps the identity the binding records.
    let length = std::fs::metadata(path).unwrap().len() as usize;
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.write_all(&vec![b'z'; length]).unwrap();
}
#[tokio::test]
async fn the_reread_after_writing_refuses_a_changed_private_disk() {
    let scene = Scene::new();
    let mut target = scene.target();
    let disk = scene.path().join("image/.pending-1/os.disk");
    target.point = Some(Arc::new(move |p| {
        if p == "written-os.disk" {
            rewrite(&disk);
        }
        Ok(())
    }));
    let (_sender, closed) = watch::channel(false);
    let result = materialize_image(&scene.image, &target, closed, |_| {}).await;
    assert_eq!(result.err(), Some(ImageError::Integrity));
    assert!(!scene.path().join("image-binding.json").exists());
}
#[tokio::test]
async fn a_retry_rereads_a_private_disk_an_earlier_run_published() {
    let scene = Scene::new();
    let mut target = scene.target();
    target.point = Some(Arc::new(|p| match p {
        "generation-published" => Err(ImageError::Storage(std::io::ErrorKind::Interrupted)),
        _ => Ok(()),
    }));
    let (_sender, closed) = watch::channel(false);
    materialize_image(&scene.image, &target, closed.clone(), |_| {})
        .await
        .unwrap_err();
    rewrite(&scene.path().join("image/1/os.disk"));
    let result = materialize_image(&scene.image, &scene.target(), closed, |_| {}).await;
    assert_eq!(result.err(), Some(ImageError::Integrity));
    assert!(!scene.path().join("image-binding.json").exists());
}
