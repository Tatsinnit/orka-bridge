use super::*;

pub(crate) fn cancelled(closed: &tokio::sync::watch::Receiver<bool>) -> Result<(), ImageError> {
    if *closed.borrow() || closed.has_changed().is_err() {
        Err(ImageError::Cancelled)
    } else {
        Ok(())
    }
}
pub(super) fn record(image: &ResolvedImage, kind: &str) -> Vec<u8> {
    serde_json::to_vec(
        &serde_json::json!({"schemaVersion":"bridge-image-cache/v1", "kind":kind,
        "manifestSha256":image.identity.manifest_sha256, "generation":1}),
    )
    .expect("fixed cache record")
}
pub(super) fn marker(dir: &Dir, name: &str, expected: &[u8]) -> Result<bool, ImageError> {
    let file = match regular(dir, name, false, false) {
        Ok(file) => file,
        Err(ImageError::Storage(io::ErrorKind::NotFound)) => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(expected.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(storage)?;
    if bytes != expected {
        return Err(ImageError::Integrity);
    }
    Ok(true)
}
pub(super) fn publish_marker(dir: &Dir, name: &str, bytes: &[u8]) -> Result<(), ImageError> {
    if marker(dir, name, bytes)? {
        return Ok(());
    }
    let pending = format!("{name}.pending");
    let mut file = regular(dir, &pending, true, true)?;
    file.set_len(0).map_err(storage)?;
    file.write_all(bytes).map_err(storage)?;
    file.sync_all().map_err(storage)?;
    drop(file);
    fs::link(dir, &pending, name)
}
pub(super) fn verify_set(
    stage: &Dir,
    image: &ResolvedImage,
    closed: Option<&tokio::sync::watch::Receiver<bool>>,
    disk: bool,
) -> Result<(), ImageError> {
    use std::io::{Seek, SeekFrom};
    if !marker(stage, "manifest.json", image.manifest_bytes())? {
        return Err(ImageError::Integrity);
    }
    for (index, asset) in image.assets.iter().enumerate() {
        if matches!(asset.role, ImageAssetRole::Chunk(_)) {
            continue;
        }
        let mut file = regular(stage, &format!("asset-{index}"), false, false)?;
        verify_checked(&mut file, asset.bytes, &asset.sha256, closed)?;
        file.seek(SeekFrom::Start(0)).map_err(storage)?;
        let mut header = Vec::new();
        file.take(4096).read_to_end(&mut header).map_err(storage)?;
        if !boot_header(image.target, asset.role, &image.initrd_compression, &header) {
            return Err(ImageError::Integrity);
        }
    }
    let mut file = regular(stage, "os.disk", false, false)?;
    if !disk {
        // The caller already hashed these bytes; only the size is rechecked.
        if file.metadata().map_err(storage)?.len() != image.disk_bytes {
            return Err(ImageError::Integrity);
        }
        return Ok(());
    }
    let mut chunks = Compressed::new(image, chunk_stream(stage, image));
    let mut buffer = [0; 65536];
    loop {
        if let Some(closed) = closed {
            cancelled(closed)?;
        }
        if chunks
            .read(&mut buffer)
            .map_err(|_| ImageError::Integrity)?
            == 0
        {
            break;
        }
    }
    if !chunks.matches() {
        return Err(ImageError::Integrity);
    }
    verify_checked(&mut file, image.disk_bytes, &image.disk_sha256, closed)
}
impl VerifiedImageSet {
    pub fn open_disk(&self) -> Result<File, ImageError> {
        regular(&self.directory, "os.disk", false, false)
    }
    pub fn open_boot(&self, role: ImageAssetRole) -> Result<File, ImageError> {
        if matches!(role, ImageAssetRole::Chunk(_)) {
            return Err(ImageError::Invalid("boot role"));
        }
        let index = self
            .image
            .assets
            .iter()
            .position(|a| a.role == role)
            .ok_or(ImageError::Integrity)?;
        regular(&self.directory, &format!("asset-{index}"), false, false)
    }
}
