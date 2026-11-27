use super::*;

#[test]
fn substituted_staging_entry_cannot_commit_a_selector_for_another_directory() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, files) = complete_image();
    let (_sender, closed) = tokio::sync::watch::channel(false);
    let tx = cache.begin(&image).unwrap();
    fill(&tx, &files);
    let root = temp
        .path()
        .join("cache/images/v2")
        .join(&image.identity().manifest_sha256);
    let moved = std::fs::rename(root.join("staging"), root.join("held-stage"));
    #[cfg(windows)]
    {
        assert_eq!(moved.unwrap_err().raw_os_error(), Some(32));
        tx.commit(&closed).unwrap();
    }
    #[cfg(not(windows))]
    {
        moved.unwrap();
        std::fs::create_dir(root.join("staging")).unwrap();
        assert!(tx.commit(&closed).is_err());
        assert!(!root.join("ready.json").exists());
    }
}

pub(crate) fn complete_image() -> (ResolvedImage, Vec<Vec<u8>>) {
    let mut image = image();
    let disk = b"tiny nonbootable disk bytes";
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(disk).unwrap();
    let compressed = encoder.finish().unwrap();
    let mut kernel = vec![0; 4096];
    kernel[0x1fe..0x200].copy_from_slice(b"\x55\xaa");
    kernel[0x202..0x206].copy_from_slice(b"HdrS");
    let files = vec![compressed.clone(), kernel, b"070701fixture".to_vec()];
    image.assets = files
        .iter()
        .zip([
            ImageAssetRole::Chunk(0),
            ImageAssetRole::Kernel,
            ImageAssetRole::Initrd,
        ])
        .enumerate()
        .map(|(i, (data, role))| ImageAsset {
            location: AssetLocation {
                base: "https://fixture.invalid/".into(),
                leaf: format!("fixture-{i}"),
            },
            role,
            bytes: data.len() as u64,
            sha256: format!("{:x}", Sha256::digest(data)),
        })
        .collect();
    image.compressed_bytes = compressed.len() as u64;
    image.compressed_sha256 = format!("{:x}", Sha256::digest(&compressed));
    image.disk_bytes = disk.len() as u64;
    image.disk_sha256 = format!("{:x}", Sha256::digest(disk));
    (image, files)
}
fn fill(tx: &CacheTransaction, files: &[Vec<u8>]) {
    for (index, bytes) in files.iter().enumerate() {
        let mut file = tx.start_asset(index).unwrap().unwrap();
        file.write_all(bytes).unwrap();
        drop(file);
        tx.finish_asset(index).unwrap();
    }
}
#[test]
fn complete_generation_is_committed_and_revalidated_not_trusted_by_marker() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, files) = complete_image();
    let (_sender, closed) = tokio::sync::watch::channel(false);
    assert!(cache.verified(&image).unwrap().is_none());
    let tx = cache.begin(&image).unwrap();
    fill(&tx, &files);
    let set = tx.commit(&closed).unwrap();
    assert_eq!(
        set.image().identity().manifest_sha256,
        image.identity().manifest_sha256
    );
    assert!(cache.verified(&image).unwrap().is_some());
    let path = temp
        .path()
        .join("cache/images/v2")
        .join(&image.identity().manifest_sha256)
        .join("generation-1/os.disk");
    std::fs::write(path, b"corrupt").unwrap();
    assert!(matches!(cache.verified(&image), Err(ImageError::Integrity)));
}
#[test]
fn cancellation_never_publishes_and_keeps_complete_chunks() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, files) = complete_image();
    let (_sender, closed) = tokio::sync::watch::channel(true);
    let tx = cache.begin(&image).unwrap();
    fill(&tx, &files);
    assert!(matches!(tx.commit(&closed), Err(ImageError::Cancelled)));
    assert!(cache.verified(&image).unwrap().is_none());
    assert!(cache
        .begin(&image)
        .unwrap()
        .start_asset(0)
        .unwrap()
        .is_none());
}
#[test]
fn complete_unselected_generation_recovers_without_rewriting_assets() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, files) = complete_image();
    let (_sender, closed) = tokio::sync::watch::channel(false);
    let tx = cache.begin(&image).unwrap();
    fill(&tx, &files);
    tx.commit(&closed).unwrap();
    let root = temp
        .path()
        .join("cache/images/v2")
        .join(&image.identity().manifest_sha256);
    std::fs::remove_file(root.join("ready.json")).unwrap();
    assert!(cache.verified(&image).unwrap().is_none());
    let tx = cache.begin(&image).unwrap();
    for index in 0..files.len() {
        assert!(tx.start_asset(index).unwrap().is_none());
    }
    tx.commit(&closed).unwrap();
    assert!(cache.verified(&image).unwrap().is_some());
}
fn rewrite(path: &std::path::Path) {
    // In place, so the file keeps the identity its owner recorded.
    let length = std::fs::metadata(path).unwrap().len() as usize;
    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.write_all(&vec![b'z'; length]).unwrap();
}
#[test]
fn a_streamed_disk_is_named_only_after_both_hashes_match() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, files) = complete_image();
    let (_sender, closed) = tokio::sync::watch::channel(false);
    let tx = cache.begin(&image).unwrap();
    fill(&tx, &files);
    // A changed gzip timestamp decodes to the same disk; only the compressed hash differs.
    let mut stamped = files[0].clone();
    stamped[4] ^= 1;
    let truncated = &files[0][..files[0].len() / 2];
    for source in [&stamped[..], truncated] {
        assert!(matches!(
            tx.decode(source, &closed),
            Err(ImageError::Integrity)
        ));
        assert!(tx.needs_disk().unwrap());
    }
    tx.decode(&files[0][..], &closed).unwrap();
    assert!(!tx.needs_disk().unwrap());
    tx.commit(&closed).unwrap();
    assert!(cache.verified(&image).unwrap().is_some());
}
#[test]
fn a_disk_left_by_an_earlier_owner_is_reread_before_selection() {
    for stage in ["staging", "generation-1"] {
        let temp = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
        let (image, files) = complete_image();
        let (_sender, closed) = tokio::sync::watch::channel(false);
        let mut tx = cache.begin(&image).unwrap();
        fill(&tx, &files);
        if stage == "staging" {
            tx.decode(&files[0][..], &closed).unwrap();
            drop(tx);
        } else {
            tx.point = Some(Arc::new(|p| match p {
                "generation-published" => Err(ImageError::Storage(io::ErrorKind::Interrupted)),
                _ => Ok(()),
            }));
            assert!(tx.commit(&closed).is_err());
        }
        let root = temp.path().join("cache/images/v2");
        rewrite(
            &root
                .join(&image.identity().manifest_sha256)
                .join(stage)
                .join("os.disk"),
        );
        let tx = cache.begin(&image).unwrap();
        assert!(
            matches!(tx.commit(&closed), Err(ImageError::Integrity)),
            "{stage}"
        );
        assert!(cache.verified(&image).unwrap().is_none(), "{stage}");
    }
}
