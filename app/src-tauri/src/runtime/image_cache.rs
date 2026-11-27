use super::image_types::{
    ImageAssetRole, ImageError, ImageTarget, ResolvedImage, VerifiedImageSet,
};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, DirBuilder, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

#[path = "image_cache_fs.rs"]
pub(crate) mod fs;
#[path = "image_cache_verify.rs"]
pub(crate) mod verification;
use verification::{cancelled, marker, publish_marker, record, verify_set};
#[derive(Clone)]
pub struct ImageCache {
    root: Arc<Dir>,
    ancestors: Arc<Vec<Dir>>,
    #[cfg(test)]
    available: Option<u64>,
}
struct OwnerLock(File, u32);
impl Drop for OwnerLock {
    fn drop(&mut self) {
        // A forked child may retain the file description until exec; it is not an owner.
        if std::process::id() == self.1 {
            let _ = self.0.unlock();
        }
    }
}
pub struct CacheTransaction {
    directory: Dir,
    stage: Dir,
    image: ResolvedImage,
    _lock: OwnerLock,
    published: bool,
    decoded: AtomicBool,
    closed: Option<tokio::sync::watch::Receiver<bool>>,
    #[cfg(test)]
    point: Option<tests::recovery::Point>,
    ancestors: Arc<Vec<Dir>>,
    #[cfg(test)]
    available: Option<u64>,
}
#[cfg(unix)]
pub(crate) fn available_space(dir: &Dir) -> Result<u64, ImageError> {
    use std::os::fd::AsRawFd;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // fstatvfs fills this buffer on success and quotes the held directory's filesystem.
    if unsafe { libc::fstatvfs(dir.as_raw_fd(), info.as_mut_ptr()) } != 0 {
        return Err(storage(io::Error::last_os_error()));
    }
    let info = unsafe { info.assume_init() };
    u64::try_from((info.f_bavail as u128) * (info.f_frsize as u128))
        .map_err(|_| ImageError::NoSpace)
}
#[cfg(windows)]
pub(crate) fn available_space(dir: &Dir) -> Result<u64, ImageError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::{
        Wdk::{
            Storage::FileSystem::{FileFsFullSizeInformation, NtQueryVolumeInformationFile},
            System::SystemServices::FILE_FS_FULL_SIZE_INFORMATION,
        },
        Win32::{Foundation::RtlNtStatusToDosError, System::IO::IO_STATUS_BLOCK},
    };
    let mut info = FILE_FS_FULL_SIZE_INFORMATION::default();
    let mut status = IO_STATUS_BLOCK::default();
    // cap-std's ordinary directory opens are synchronous; query by handle, never by a reconstructed pathname.
    let result = unsafe {
        NtQueryVolumeInformationFile(
            dir.as_raw_handle(),
            &mut status,
            (&mut info as *mut FILE_FS_FULL_SIZE_INFORMATION).cast(),
            std::mem::size_of_val(&info) as u32,
            FileFsFullSizeInformation,
        )
    };
    if result != 0 {
        return Err(storage(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        )));
    }
    let units =
        u128::try_from(info.CallerAvailableAllocationUnits).map_err(|_| ImageError::NoSpace)?;
    let bytes = units
        .checked_mul(info.SectorsPerAllocationUnit.into())
        .and_then(|n| n.checked_mul(info.BytesPerSector.into()))
        .ok_or(ImageError::NoSpace)?;
    u64::try_from(bytes).map_err(|_| ImageError::NoSpace)
}
fn require_space(image: &ResolvedImage, available: u64) -> Result<(), ImageError> {
    let bytes = image
        .assets
        .iter()
        .filter(|a| !matches!(a.role, ImageAssetRole::Chunk(_)))
        .try_fold(
            image
                .compressed_bytes
                .checked_add(image.disk_bytes)
                .ok_or(ImageError::NoSpace)?,
            |n, a| n.checked_add(a.bytes).ok_or(ImageError::NoSpace),
        )?;
    available
        .checked_sub(bytes)
        .map(|_| ())
        .ok_or(ImageError::NoSpace)
}
pub(crate) fn storage(error: io::Error) -> ImageError {
    if error.kind() == io::ErrorKind::StorageFull {
        ImageError::NoSpace
    } else {
        ImageError::Storage(error.kind())
    }
}
pub(crate) fn not_reparse(file: &File) -> Result<(), ImageError> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if file.metadata().map_err(storage)?.file_attributes() & 0x400 != 0 {
            return Err(ImageError::UnsafePath);
        }
    }
    let _ = file;
    Ok(())
}
pub(crate) fn read_directory(
    parent: &Dir,
    name: impl AsRef<Path>,
) -> Result<Option<Dir>, ImageError> {
    let dir = match parent.open_dir_nofollow(name) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(storage(e)),
    };
    not_reparse(&dir.try_clone().map_err(storage)?.into_std_file())?;
    Ok(Some(dir))
}
pub(crate) fn directory(parent: &Dir, name: &std::ffi::OsStr) -> Result<Dir, ImageError> {
    let builder = DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use cap_std::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    if let Err(e) = parent.create_dir_with(name, &builder) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(storage(e));
        }
    }
    let dir = parent
        .open_dir_nofollow(name)
        .map_err(|_| ImageError::UnsafePath)?;
    not_reparse(&dir.try_clone().map_err(storage)?.into_std_file())?;
    Ok(dir)
}
pub(crate) fn regular(
    parent: &Dir,
    name: &str,
    write: bool,
    create: bool,
) -> Result<File, ImageError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(write)
        .create(create)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = parent
        .open_with(name, &options)
        .map_err(storage)?
        .into_std();
    not_reparse(&file)?;
    if !file.metadata().map_err(storage)?.is_file() {
        return Err(ImageError::UnsafePath);
    }
    if write {
        single_link(&file)?;
    }
    Ok(file)
}
pub(crate) fn single_link(file: &File) -> Result<(), ImageError> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(storage(io::Error::last_os_error()));
        }
        if info.nNumberOfLinks != 1 {
            return Err(ImageError::UnsafePath);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata().map_err(storage)?.nlink() != 1 {
            return Err(ImageError::UnsafePath);
        }
    }
    Ok(())
}
impl ImageCache {
    pub fn manifest(
        &self,
        pin: &super::image_types::ImagePin,
    ) -> Result<Option<Vec<u8>>, ImageError> {
        if pin.manifest_sha256.len() != 64
            || !pin
                .manifest_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ImageError::Invalid("cache pin"));
        }
        let Some(entry) = read_directory(&self.root, &pin.manifest_sha256)? else {
            return Ok(None);
        };
        let stage = match read_directory(&entry, "generation-1")? {
            Some(d) => Some(d),
            None => read_directory(&entry, "staging")?,
        };
        let Some(stage) = stage else { return Ok(None) };
        let file = match regular(&stage, "manifest.json", false, false) {
            Ok(f) => f,
            Err(ImageError::Storage(io::ErrorKind::NotFound)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(storage)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(ImageError::LimitExceeded);
        }
        if format!("{:x}", Sha256::digest(&bytes)) != pin.manifest_sha256 {
            return Err(ImageError::Integrity);
        }
        Ok(Some(bytes))
    }
    pub fn verified(&self, image: &ResolvedImage) -> Result<Option<VerifiedImageSet>, ImageError> {
        self.verified_checked(image, None)
    }
    pub(crate) fn verified_checked(
        &self,
        image: &ResolvedImage,
        closed: Option<&tokio::sync::watch::Receiver<bool>>,
    ) -> Result<Option<VerifiedImageSet>, ImageError> {
        if let Some(closed) = closed {
            cancelled(closed)?;
        }
        let Some(dir) = read_directory(&self.root, &image.identity.manifest_sha256)? else {
            return Ok(None);
        };
        if !marker(&dir, "ready.json", &record(image, "ready"))? {
            return Ok(None);
        }
        let stage = dir.open_dir_nofollow("generation-1").map_err(storage)?;
        not_reparse(&stage.try_clone().map_err(storage)?.into_std_file())?;
        if !marker(&stage, "generation.json", &record(image, "generation"))? {
            return Err(ImageError::Integrity);
        }
        verify_set(&stage, image, closed, true)?;
        Ok(Some(VerifiedImageSet {
            directory: Arc::new(stage),
            image: image.clone(),
        }))
    }
    /// A native app-local-data location, never a renderer-supplied path.
    pub fn open(path: &Path) -> Result<Self, ImageError> {
        let parent = Dir::open_ambient_dir(
            path.parent().ok_or(ImageError::UnsafePath)?,
            cap_std::ambient_authority(),
        )
        .map_err(storage)?;
        let root = directory(&parent, path.file_name().ok_or(ImageError::UnsafePath)?)?;
        let images = directory(&root, std::ffi::OsStr::new("images"))?;
        let versioned = directory(&images, std::ffi::OsStr::new("v2"))?;
        let ancestors = Arc::new(vec![
            versioned.try_clone().map_err(storage)?,
            images,
            root,
            parent,
        ]);
        Ok(Self {
            root: Arc::new(versioned),
            ancestors,
            #[cfg(test)]
            available: None,
        })
    }
    pub fn begin(&self, image: &ResolvedImage) -> Result<CacheTransaction, ImageError> {
        let available = available_space(&self.root)?;
        #[cfg(test)]
        let available = self.available.unwrap_or(available);
        let directory = directory(
            &self.root,
            std::ffi::OsStr::new(&image.identity().manifest_sha256),
        )?;
        // Never unlink/recreate this filename: all processes must lock the same object.
        let lock = regular(&directory, "owner.lock", true, true)?;
        lock.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => ImageError::Busy,
            std::fs::TryLockError::Error(e) => storage(e),
        })?;
        let lock = OwnerLock(lock, std::process::id());
        if marker(&directory, "ready.json", &record(image, "ready"))? {
            return Err(ImageError::RetainedImage);
        }
        let published = match directory.symlink_metadata("generation-1") {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(storage(e)),
        };
        let stage = if published {
            directory
                .open_dir_nofollow("generation-1")
                .map_err(storage)?
        } else {
            match read_directory(&directory, "staging")? {
                Some(stage) => stage,
                None => {
                    require_space(image, available)?;
                    self::directory(&directory, std::ffi::OsStr::new("staging"))?
                }
            }
        };
        not_reparse(&stage.try_clone().map_err(storage)?.into_std_file())?;
        if published && !marker(&stage, "generation.json", &record(image, "generation"))? {
            return Err(ImageError::Integrity);
        }
        publish_marker(&stage, "manifest.json", image.manifest_bytes())?;
        Ok(CacheTransaction {
            directory,
            stage,
            image: image.clone(),
            _lock: lock,
            published,
            decoded: AtomicBool::new(false),
            closed: None,
            #[cfg(test)]
            point: None,
            ancestors: self.ancestors.clone(),
            #[cfg(test)]
            available: self.available,
        })
    }
}
fn boot_header(
    target: ImageTarget,
    role: ImageAssetRole,
    compression: &str,
    header: &[u8],
) -> bool {
    match role {
        ImageAssetRole::Kernel => match target {
            ImageTarget::WindowsAmd64 => {
                header.get(0x1fe..0x200) == Some(b"\x55\xaa")
                    && header.get(0x202..0x206) == Some(b"HdrS")
            }
            ImageTarget::MacosArm64 => header.get(0x38..0x3c) == Some(b"ARM\x64"),
        },
        ImageAssetRole::Initrd => match compression {
            "gzip" => header.starts_with(b"\x1f\x8b"),
            "zstd" => header.starts_with(b"\x28\xb5\x2f\xfd"),
            "xz" => header.starts_with(b"\xfd7zXZ\0"),
            "cpio-prefix" => header.starts_with(b"070701") || header.starts_with(b"070702"),
            _ => false,
        },
        ImageAssetRole::Chunk(_) => false,
    }
}
struct Chunks<'a> {
    stage: &'a Dir,
    image: &'a ResolvedImage,
    next: usize,
    file: Option<File>,
    expected: Option<(u64, &'a str)>,
    bytes: u64,
    hash: Sha256,
}
impl Read for Chunks<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(file) = &mut self.file {
                let n = file.read(buffer)?;
                if n > 0 {
                    self.bytes = self
                        .bytes
                        .checked_add(n as u64)
                        .filter(|n| *n <= self.expected.unwrap().0)
                        .ok_or(io::ErrorKind::InvalidData)?;
                    self.hash.update(&buffer[..n]);
                    return Ok(n);
                }
                let (length, hash) = self.expected.take().unwrap();
                if self.bytes != length || format!("{:x}", self.hash.clone().finalize()) != hash {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                self.file = None;
            }
            let Some((index, asset)) = self
                .image
                .assets
                .iter()
                .enumerate()
                .skip(self.next)
                .find(|(_, a)| matches!(a.role, ImageAssetRole::Chunk(_)))
            else {
                return Ok(0);
            };
            self.next = index + 1;
            self.bytes = 0;
            self.hash = Sha256::new();
            self.expected = Some((asset.bytes, &asset.sha256));
            self.file = Some(
                regular(self.stage, &format!("asset-{index}"), false, false).map_err(|error| {
                    match error {
                        ImageError::Storage(kind) => io::Error::from(kind),
                        _ => io::ErrorKind::InvalidData.into(),
                    }
                })?,
            );
        }
    }
}
fn chunk_stream<'a>(stage: &'a Dir, image: &'a ResolvedImage) -> Chunks<'a> {
    Chunks {
        stage,
        image,
        next: 0,
        file: None,
        expected: None,
        bytes: 0,
        hash: Sha256::new(),
    }
}
/// Counts and hashes the whole compressed stream, refusing more than its pinned size.
struct Compressed<'a, R> {
    source: R,
    image: &'a ResolvedImage,
    total: u64,
    whole: Sha256,
}
impl<'a, R: Read> Compressed<'a, R> {
    fn new(image: &'a ResolvedImage, source: R) -> Self {
        let whole = Sha256::new();
        Self {
            source,
            image,
            total: 0,
            whole,
        }
    }
    fn matches(&self) -> bool {
        self.total == self.image.compressed_bytes
            && format!("{:x}", self.whole.clone().finalize()) == self.image.compressed_sha256
    }
}
impl<R: Read> Read for Compressed<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let n = self.source.read(buffer)?;
        let limit = self
            .image
            .compressed_bytes
            .min(self.image.limits.compressed_bytes);
        self.total = self
            .total
            .checked_add(n as u64)
            .filter(|n| *n <= limit)
            .ok_or(io::ErrorKind::InvalidData)?;
        self.whole.update(&buffer[..n]);
        Ok(n)
    }
}
/// Leaves all-zero blocks of a fresh, empty file unwritten, so filesystems with holes store them free.
struct SparseOutput {
    file: File,
    length: u64,
    hole: u64,
}
const SPARSE_BLOCK: u64 = 4096;
fn zero(block: &[u8]) -> bool {
    let (words, tail) = block.as_chunks::<8>();
    words.iter().fold(0, |a, w| a | u64::from_ne_bytes(*w)) == 0 && tail.iter().all(|b| *b == 0)
}
/// macOS can zero-fill a skipped range when a later write lands past it; punching returns those
/// blocks. A volume without holes refuses, and its zeros read the same, so failure is ignored.
#[cfg(target_os = "macos")]
fn punch(file: &File, start: u64, length: u64) {
    use std::os::fd::AsRawFd;
    let from = start.next_multiple_of(SPARSE_BLOCK);
    let to = (start + length) / SPARSE_BLOCK * SPARSE_BLOCK;
    let length = to.saturating_sub(from);
    if let (true, Ok(fp_offset), Ok(fp_length)) =
        (length > 0, i64::try_from(from), i64::try_from(length))
    {
        let mut hole = libc::fpunchhole_t {
            fp_flags: 0,
            reserved: 0,
            fp_offset,
            fp_length,
        };
        // fcntl reads this region descriptor for the open file; no pointer outlives the call.
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PUNCHHOLE, &mut hole) };
    }
}
#[cfg(not(target_os = "macos"))]
fn punch(_: &File, _: u64, _: u64) {}
impl SparseOutput {
    fn write(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            // Blocks align to the logical offset; a run of non-zero blocks is one write.
            let mut run = 0;
            let mut zeroes = None;
            while run < data.len() {
                let step = ((SPARSE_BLOCK - (self.length + run as u64) % SPARSE_BLOCK) as usize)
                    .min(data.len() - run);
                let empty = zero(&data[run..run + step]);
                if *zeroes.get_or_insert(empty) != empty {
                    break;
                }
                run += step;
            }
            if zeroes == Some(true) {
                self.hole += run as u64;
            } else {
                let start = self.length - self.hole;
                if self.hole > 0 {
                    let hole = i64::try_from(self.hole).map_err(|_| io::ErrorKind::InvalidData)?;
                    self.file.seek(SeekFrom::Current(hole))?;
                }
                self.file.write_all(&data[..run])?;
                punch(&self.file, start, std::mem::take(&mut self.hole));
            }
            self.length += run as u64;
            data = &data[run..];
        }
        Ok(())
    }
    fn finish(self) -> io::Result<File> {
        self.file.set_len(self.length)?;
        Ok(self.file)
    }
}
fn decode_gzip(
    image: &ResolvedImage,
    source: impl Read,
    output: File,
    closed: &tokio::sync::watch::Receiver<bool>,
) -> Result<(), ImageError> {
    use std::io::{BufRead, BufReader};
    let mut source = BufReader::with_capacity(65536, Compressed::new(image, source));
    let mut output = SparseOutput {
        file: output,
        length: 0,
        hole: 0,
    };
    let mut bytes = 0u64;
    let mut digest = Sha256::new();
    let mut first = true;
    loop {
        if *closed.borrow() || closed.has_changed().is_err() {
            return Err(ImageError::Cancelled);
        }
        if !first {
            loop {
                let buffer = source.fill_buf().map_err(|_| ImageError::Integrity)?;
                let zeroes = buffer.iter().take_while(|b| **b == 0).count();
                if zeroes == 0 {
                    break;
                }
                source.consume(zeroes);
                if *closed.borrow() {
                    return Err(ImageError::Cancelled);
                }
            }
            if source
                .fill_buf()
                .map_err(|_| ImageError::Integrity)?
                .is_empty()
            {
                break;
            }
        }
        // A single-member decoder preserves the buffered remainder, including Python's NUL padding.
        let mut member = flate2::bufread::GzDecoder::new(&mut source);
        let mut buffer = [0; 65536];
        loop {
            if *closed.borrow() || closed.has_changed().is_err() {
                return Err(ImageError::Cancelled);
            }
            let n = member
                .read(&mut buffer)
                .map_err(|_| ImageError::Integrity)?;
            if n == 0 {
                break;
            }
            bytes = bytes
                .checked_add(n as u64)
                .filter(|n| *n <= image.disk_bytes.min(image.limits.disk_bytes))
                .ok_or(ImageError::Integrity)?;
            output.write(&buffer[..n]).map_err(storage)?;
            digest.update(&buffer[..n]);
        }
        first = false;
    }
    if bytes != image.disk_bytes
        || format!("{:x}", digest.finalize()) != image.disk_sha256
        || !source.get_ref().matches()
    {
        return Err(ImageError::Integrity);
    }
    output
        .finish()
        .map_err(storage)?
        .sync_all()
        .map_err(storage)?;
    Ok(())
}
fn verify_checked(
    file: &mut File,
    bytes: u64,
    hash: &str,
    closed: Option<&tokio::sync::watch::Receiver<bool>>,
) -> Result<(), ImageError> {
    let mut buffer = [0; 65536];
    let mut actual = 0u64;
    let mut digest = Sha256::new();
    loop {
        if let Some(closed) = closed {
            cancelled(closed)?;
        }
        let n = file.read(&mut buffer).map_err(storage)?;
        if n == 0 {
            break;
        }
        actual = actual
            .checked_add(n as u64)
            .filter(|n| *n <= bytes)
            .ok_or(ImageError::Integrity)?;
        digest.update(&buffer[..n]);
    }
    if actual != bytes || format!("{:x}", digest.finalize()) != hash {
        return Err(ImageError::Integrity);
    }
    Ok(())
}
impl CacheTransaction {
    pub(crate) fn with_cancellation(mut self, closed: tokio::sync::watch::Receiver<bool>) -> Self {
        self.closed = Some(closed);
        self
    }
    fn sync_namespace(&self) -> Result<(), ImageError> {
        fs::sync(&self.stage)?;
        fs::sync(&self.directory)?;
        for dir in self.ancestors.iter() {
            fs::sync(dir)?;
        }
        Ok(())
    }
    fn space(&self, bytes: u64) -> Result<(), ImageError> {
        #[cfg(test)]
        if let Some(available) = self.available {
            return if bytes <= available {
                Ok(())
            } else {
                Err(ImageError::NoSpace)
            };
        }
        available_space(&self.directory)?
            .checked_sub(bytes)
            .map(|_| ())
            .ok_or(ImageError::NoSpace)
    }
    pub fn commit(
        self,
        closed: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<VerifiedImageSet, ImageError> {
        #[cfg(test)]
        let point = |name: &str| self.point.as_ref().map_or(Ok(()), |p| p(name));
        cancelled(closed)?;
        if self.needs_disk()? {
            self.decode(chunk_stream(&self.stage, &self.image), closed)?;
        }
        // A disk this owner decoded was hashed as it was written; an older one is reread.
        let reread = !self.decoded.load(Ordering::Acquire);
        verify_set(&self.stage, &self.image, Some(closed), reread)?;
        if !self.published {
            publish_marker(
                &self.stage,
                "generation.json",
                &record(&self.image, "generation"),
            )?;
            #[cfg(test)]
            point("generation-record")?;
            fs::sync(&self.stage)?;
            cancelled(closed)?;
        }
        // cap-std Windows directory handles do not share DELETE access.
        drop(self.stage);
        if !self.published {
            fs::publish(&self.directory)?;
        }
        #[cfg(test)]
        point("generation-published")?;
        fs::sync(&self.directory)?;
        for dir in self.ancestors.iter() {
            fs::sync(dir)?;
        }
        cancelled(closed)?;
        let committed = self
            .directory
            .open_dir_nofollow("generation-1")
            .map_err(storage)?;
        not_reparse(&committed.try_clone().map_err(storage)?.into_std_file())?;
        if !marker(
            &committed,
            "generation.json",
            &record(&self.image, "generation"),
        )? {
            return Err(ImageError::Integrity);
        }
        // The rename kept the files just checked; installs reread the disk they copy.
        verify_set(&committed, &self.image, Some(closed), false)?;
        cancelled(closed)?;
        #[cfg(test)]
        point("before-selector")?;
        publish_marker(&self.directory, "ready.json", &record(&self.image, "ready"))?;
        #[cfg(test)]
        point("selector-published")?;
        fs::sync(&self.directory)?;
        Ok(VerifiedImageSet {
            directory: Arc::new(committed),
            image: self.image.clone(),
        })
    }
    /// Whether the OS disk is still to be decoded; acquisition then streams chunks into `decode`.
    pub(crate) fn needs_disk(&self) -> Result<bool, ImageError> {
        if self.published {
            return Ok(false);
        }
        match regular(&self.stage, "os.disk", false, false) {
            Ok(_) => Ok(false),
            Err(ImageError::Storage(io::ErrorKind::NotFound)) => Ok(true),
            Err(e) => Err(e),
        }
    }
    /// Names the disk `os.disk` only after both the compressed and raw hashes match.
    pub(crate) fn decode(
        &self,
        source: impl Read,
        closed: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), ImageError> {
        let file = regular(&self.stage, "disk.pending", true, true)?;
        file.set_len(0).map_err(storage)?;
        self.space(self.image.disk_bytes)?;
        decode_gzip(&self.image, source, file, closed)?;
        fs::link(&self.stage, "disk.pending", "os.disk")?;
        self.decoded.store(true, Ordering::Release);
        Ok(())
    }
    /// Hands a chunk kept from an earlier run to a decoder that starts from the first byte.
    #[allow(dead_code)] // Image-only harnesses omit the acquisition caller.
    pub(crate) fn replay(
        &self,
        index: usize,
        decoder: &std::sync::mpsc::SyncSender<Vec<u8>>,
    ) -> Result<(), ImageError> {
        let mut file = regular(&self.stage, &format!("asset-{index}"), false, false)?;
        loop {
            self.closed.as_ref().map_or(Ok(()), cancelled)?;
            let mut bytes = vec![0; 65536];
            let n = file.read(&mut bytes).map_err(storage)?;
            if n == 0 {
                return Ok(());
            }
            bytes.truncate(n);
            decoder
                .send(bytes)
                .map_err(|_| ImageError::Storage(io::ErrorKind::BrokenPipe))?;
        }
    }
    pub(crate) fn start_asset(&self, index: usize) -> Result<Option<File>, ImageError> {
        self.closed.as_ref().map_or(Ok(()), cancelled)?;
        let asset = self
            .image
            .assets
            .get(index)
            .ok_or(ImageError::Invalid("asset index"))?;
        let name = format!("asset-{index}");
        match regular(&self.stage, &name, false, false) {
            Ok(mut file) => {
                verify_checked(&mut file, asset.bytes, &asset.sha256, self.closed.as_ref())?;
                self.sync_namespace()?;
                return Ok(None);
            }
            Err(ImageError::Storage(io::ErrorKind::NotFound)) => {}
            Err(e) => return Err(e),
        }
        if self.published {
            return Err(ImageError::Integrity);
        }
        let file = regular(&self.stage, &format!("pending-{index}"), true, true)?;
        file.set_len(0).map_err(storage)?;
        self.space(asset.bytes)?;
        Ok(Some(file))
    }
    pub(crate) fn finish_asset(&self, index: usize) -> Result<(), ImageError> {
        self.closed.as_ref().map_or(Ok(()), cancelled)?;
        let asset = self
            .image
            .assets
            .get(index)
            .ok_or(ImageError::Invalid("asset index"))?;
        let pending = format!("pending-{index}");
        let mut file = regular(&self.stage, &pending, true, false)?;
        verify_checked(&mut file, asset.bytes, &asset.sha256, self.closed.as_ref())?;
        file.sync_all().map_err(storage)?;
        drop(file);
        fs::link(&self.stage, &pending, &format!("asset-{index}"))?;
        self.sync_namespace()
    }
}
#[cfg(test)]
#[path = "image_cache_tests.rs"]
pub(crate) mod tests;
