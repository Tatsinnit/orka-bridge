#[path = "image_cache_publish_tests.rs"]
pub(crate) mod publication;
#[path = "image_cache_recovery_tests.rs"]
pub(crate) mod recovery;
use super::super::image_types::*;
use super::*;
use sha2::{Digest, Sha256};
fn image() -> ResolvedImage {
    let raw = b"tiny nonbootable native-owned descriptor fixture";
    ResolvedImage {
        identity: ImageIdentity {
            manifest_sha256: format!("{:x}", Sha256::digest(raw)),
            image_id: "test-only".into(),
            state_compatibility_id: "fixture-v1".into(),
            build_source_commit: "a".repeat(40),
        },
        target: ImageTarget::WindowsAmd64,
        launch_mode: LaunchMode::LinuxDirect,
        command_line: "fixture".into(),
        initrd_compression: "cpio-prefix".into(),
        assets: vec![],
        limits: TransferLimits {
            compressed_bytes: 4096,
            disk_bytes: 4096,
            kernel_bytes: 4096,
            initrd_bytes: 4096,
        },
        compressed_sha256: "a".repeat(64),
        compressed_bytes: 32,
        disk_sha256: "b".repeat(64),
        disk_bytes: 64,
        manifest_bytes: raw.as_slice().into(),
    }
}
fn chunk_image() -> (ResolvedImage, Vec<u8>) {
    let mut image = image();
    let data = b"complete pinned chunk bytes".to_vec();
    image.assets.push(ImageAsset {
        location: AssetLocation {
            base: "https://fixtures.invalid/".into(),
            leaf: "part-0000".into(),
        },
        role: ImageAssetRole::Chunk(0),
        bytes: data.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&data)),
    });
    (image, data)
}
#[test]
fn only_complete_rehashed_chunks_resume_for_the_same_manifest() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, data) = chunk_image();
    {
        let tx = cache.begin(&image).unwrap();
        let mut file = tx.start_asset(0).unwrap().unwrap();
        file.write_all(&data[..3]).unwrap();
    }
    {
        let tx = cache.begin(&image).unwrap();
        let mut file = tx.start_asset(0).unwrap().unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        file.write_all(&data).unwrap();
        file.sync_all().unwrap();
        drop(file);
        tx.finish_asset(0).unwrap();
        assert!(tx.start_asset(0).unwrap().is_none());
    }
    let tx = cache.begin(&image).unwrap();
    assert!(tx.start_asset(0).unwrap().is_none());
}
#[test]
fn changed_complete_chunk_is_integrity_failure_not_a_resume_marker() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (image, data) = chunk_image();
    let tx = cache.begin(&image).unwrap();
    let mut file = tx.start_asset(0).unwrap().unwrap();
    file.write_all(&data).unwrap();
    drop(file);
    tx.finish_asset(0).unwrap();
    let path = temp
        .path()
        .join("cache/images/v2")
        .join(&image.identity.manifest_sha256)
        .join("staging/asset-0");
    std::fs::write(path, vec![0; data.len()]).unwrap();
    assert!(matches!(tx.start_asset(0), Err(ImageError::Integrity)));
}
#[test]
fn gzip_uses_the_shared_python_member_padding_and_refusal_vectors() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../images/testdata/v2/gzip-future.json"
    ))
    .unwrap();
    for row in rows {
        let hex = row["hex"].as_str().unwrap();
        let compressed: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let valid = row["valid"].as_bool().unwrap();
        let payload = if valid {
            row["payload"].as_str().unwrap().as_bytes()
        } else {
            b"a"
        };
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
        let mut image = image();
        image.assets.clear();
        for (index, byte) in compressed.iter().enumerate() {
            std::fs::write(temp.path().join(format!("asset-{index}")), [*byte]).unwrap();
            image.assets.push(ImageAsset {
                location: AssetLocation {
                    base: "https://fixtures.invalid/".into(),
                    leaf: format!("part-{index:04}"),
                },
                role: ImageAssetRole::Chunk(index as u32),
                bytes: 1,
                sha256: format!("{:x}", Sha256::digest([*byte])),
            });
        }
        image.compressed_bytes = compressed.len() as u64;
        image.compressed_sha256 = format!("{:x}", Sha256::digest(&compressed));
        image.disk_bytes = payload.len() as u64;
        image.disk_sha256 = format!("{:x}", Sha256::digest(payload));
        let output = std::fs::File::create(temp.path().join("output")).unwrap();
        let (_tx, closed) = tokio::sync::watch::channel(false);
        assert_eq!(
            decode_gzip(&image, &dir, output, &closed).is_ok(),
            valid,
            "{}",
            row["invariant"]
        );
        if valid {
            assert_eq!(std::fs::read(temp.path().join("output")).unwrap(), payload);
        }
    }
}
#[test]
fn boot_headers_match_python_without_executing_or_decompressing_guest_files() {
    let mut bz = vec![0; 4096];
    bz[0x1fe..0x200].copy_from_slice(&[0x55, 0xaa]);
    bz[0x202..0x206].copy_from_slice(b"HdrS");
    assert!(boot_header(
        ImageTarget::WindowsAmd64,
        ImageAssetRole::Kernel,
        "gzip",
        &bz
    ));
    assert!(!boot_header(
        ImageTarget::MacosArm64,
        ImageAssetRole::Kernel,
        "gzip",
        &bz
    ));
    let mut arm = vec![0; 64];
    arm[0x38..0x3c].copy_from_slice(b"ARM\x64");
    assert!(boot_header(
        ImageTarget::MacosArm64,
        ImageAssetRole::Kernel,
        "gzip",
        &arm
    ));
    assert!(!boot_header(
        ImageTarget::WindowsAmd64,
        ImageAssetRole::Kernel,
        "gzip",
        &arm
    ));
    for (prefix, compression) in [
        (b"\x1f\x8b".as_slice(), "gzip"),
        (b"\x28\xb5\x2f\xfd", "zstd"),
        (b"\xfd7zXZ\0", "xz"),
        (b"070701", "cpio-prefix"),
        (b"070702", "cpio-prefix"),
    ] {
        assert!(boot_header(
            ImageTarget::MacosArm64,
            ImageAssetRole::Initrd,
            compression,
            prefix
        ));
        assert!(!boot_header(
            ImageTarget::MacosArm64,
            ImageAssetRole::Initrd,
            compression,
            &prefix[..prefix.len() - 1]
        ));
    }
    assert!(!boot_header(
        ImageTarget::MacosArm64,
        ImageAssetRole::Initrd,
        "xz",
        b"\x1f\x8b"
    ));
}
#[test]
fn empty_read_does_not_consume_or_validate_a_chunk_prematurely() {
    let temp = tempfile::tempdir().unwrap();
    let (image, data) = chunk_image();
    std::fs::write(temp.path().join("asset-0"), data).unwrap();
    let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
    let mut chunks = Chunks {
        stage: &dir,
        image: &image,
        next: 0,
        file: None,
        expected: None,
        bytes: 0,
        hash: Sha256::new(),
        total: 0,
        whole: Sha256::new(),
    };
    assert_eq!(chunks.read(&mut []).unwrap(), 0);
    assert_eq!(chunks.next, 0);
}
#[test]
fn insufficient_space_and_overflow_refuse_before_payload_files() {
    let temp = tempfile::tempdir().unwrap();
    let mut cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let (mut image, _) = chunk_image();
    cache.available = Some(0);
    assert!(matches!(cache.begin(&image), Err(ImageError::NoSpace)));
    assert!(!temp
        .path()
        .join("cache/images/v2")
        .join(&image.identity.manifest_sha256)
        .join("staging/asset-0")
        .exists());
    image.disk_bytes = u64::MAX;
    cache.available = Some(u64::MAX);
    assert!(matches!(cache.begin(&image), Err(ImageError::NoSpace)));
}
#[test]
fn per_pin_lock_is_busy_and_release_does_not_unlink_lock_authority() {
    let temp = tempfile::tempdir().unwrap();
    let cache = ImageCache::open(&temp.path().join("cache")).unwrap();
    let image = image();
    let tx = cache.begin(&image).unwrap();
    assert!(matches!(cache.begin(&image), Err(ImageError::Busy)));
    drop(tx);
    assert!(cache.begin(&image).is_ok());
}
#[cfg(unix)]
#[test]
fn cache_root_symlink_is_not_adopted() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), temp.path().join("cache")).unwrap();
    assert!(matches!(
        ImageCache::open(&temp.path().join("cache")),
        Err(ImageError::UnsafePath)
    ));
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}
fn sparse_disk() -> Vec<u8> {
    // Zero runs on and off block boundaries, single non-zero bytes at both block edges, a zero tail.
    let mut disk = vec![7u8; 5000];
    disk.resize(disk.len() + (1 << 20), 0);
    let edge = disk.len() + (4096 - disk.len() % 4096);
    disk.resize(edge + 4096, 0);
    disk[edge] = 1;
    disk[edge + 4095] = 2;
    disk.resize(disk.len() + 8192, 0);
    disk.push(3);
    disk.resize(disk.len() + (1 << 20) + 123, 0);
    disk
}
#[test]
fn zero_runs_decode_to_the_same_hash_into_a_sparse_file() {
    use std::io::Write;
    let disk = sparse_disk();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&disk).unwrap();
    let compressed = encoder.finish().unwrap();
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("asset-0"), &compressed).unwrap();
    let dir = Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority()).unwrap();
    let mut image = image();
    image.assets = vec![ImageAsset {
        location: AssetLocation {
            base: "https://fixtures.invalid/".into(),
            leaf: "part-0000".into(),
        },
        role: ImageAssetRole::Chunk(0),
        bytes: compressed.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&compressed)),
    }];
    image.limits.compressed_bytes = compressed.len() as u64;
    image.limits.disk_bytes = disk.len() as u64;
    image.compressed_bytes = compressed.len() as u64;
    image.compressed_sha256 = format!("{:x}", Sha256::digest(&compressed));
    image.disk_bytes = disk.len() as u64;
    image.disk_sha256 = format!("{:x}", Sha256::digest(&disk));
    let output = std::fs::File::create(temp.path().join("output")).unwrap();
    let (_tx, closed) = tokio::sync::watch::channel(false);
    decode_gzip(&image, &dir, output, &closed).unwrap();
    let written = std::fs::read(temp.path().join("output")).unwrap();
    assert_eq!(written.len(), disk.len());
    assert_eq!(format!("{:x}", Sha256::digest(&written)), image.disk_sha256);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let allocated = std::fs::metadata(temp.path().join("output"))
            .unwrap()
            .blocks()
            * 512;
        assert!(allocated < 256 * 1024, "{allocated} bytes allocated");
    }
}
#[test]
fn sparse_output_keeps_every_non_zero_byte_across_unaligned_writes() {
    let disk = sparse_disk();
    for split in [1, 7, 4095, 4096, 4097, 65536] {
        let temp = tempfile::tempdir().unwrap();
        let mut output = SparseOutput {
            file: std::fs::File::create(temp.path().join("output")).unwrap(),
            length: 0,
            hole: 0,
        };
        for piece in disk.chunks(split) {
            output.write(piece).unwrap();
        }
        output.finish().unwrap();
        assert_eq!(
            std::fs::read(temp.path().join("output")).unwrap(),
            disk,
            "split {split}"
        );
    }
}
