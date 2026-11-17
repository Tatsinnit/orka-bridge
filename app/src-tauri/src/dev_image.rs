//! Debug-only evidence for an explicitly selected engineering image folder.
//!
//! The raw engineering receipt is hash-bound, not parsed as a release manifest.
//! These read-only handles are not an installed image or permission to boot.
//! Another same-user writer can still mutate their backing files. A future
//! consumer must materialize/reverify owned runtime copies, never boot the
//! factory writable. This verifier is not an OS sandbox.
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fmt,
    io::{self, Read, Seek},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevImagePlatform {
    MacOsArm64,
    WindowsAmd64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevImageDiskFormat {
    Raw,
    Qcow2,
}

impl DevImagePlatform {
    pub const fn state_compatibility_id(self) -> &'static str {
        match self {
            Self::MacOsArm64 => "engineering-43e73491",
            Self::WindowsAmd64 => "engineering-43e73491",
        }
    }
    pub const fn source_commit(self) -> &'static str {
        match self {
            Self::MacOsArm64 => "43e7349115b4c7e9b1865a4d5f0989a02726724c",
            Self::WindowsAmd64 => "43e7349115b4c7e9b1865a4d5f0989a02726724c",
        }
    }
}
pub const STATE_COMPATIBILITY_ID: &str = if cfg!(windows) {
    DevImagePlatform::WindowsAmd64.state_compatibility_id()
} else {
    DevImagePlatform::MacOsArm64.state_compatibility_id()
};

pub struct VerifiedDevImage {
    platform: DevImagePlatform,
    pins: [FilePin; 4],
    files: [File; 4],
}
impl VerifiedDevImage {
    pub(crate) fn try_clone(&self) -> Result<Self, DevImageError> {
        let copy = || {
            Ok::<_, io::Error>(Self {
                platform: self.platform,
                pins: self.pins,
                files: [
                    self.files[0].try_clone()?,
                    self.files[1].try_clone()?,
                    self.files[2].try_clone()?,
                    self.files[3].try_clone()?,
                ],
            })
        };
        copy().map_err(|_| DevImageError)
    }
    pub(crate) fn declared(&self, slot: usize) -> (u64, String) {
        let pin = &self.pins[slot];
        (
            pin.bytes,
            pin.sha256.iter().map(|b| format!("{b:02x}")).collect(),
        )
    }
    pub fn platform(&self) -> DevImagePlatform {
        self.platform
    }
    pub fn disk_format(&self) -> DevImageDiskFormat {
        match self.platform {
            DevImagePlatform::MacOsArm64 => DevImageDiskFormat::Raw,
            DevImagePlatform::WindowsAmd64 => DevImageDiskFormat::Qcow2,
        }
    }
    pub fn manifest(&self) -> &File {
        &self.files[0]
    }
    pub fn kernel(&self) -> &File {
        &self.files[1]
    }
    pub fn initrd(&self) -> &File {
        &self.files[2]
    }
    pub fn disk(&self) -> &File {
        &self.files[3]
    }
}
pub struct DevImageError;
impl fmt::Display for DevImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("debug local image unavailable")
    }
}
impl fmt::Debug for DevImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for DevImageError {}

#[derive(Clone, Copy)]
struct FilePin {
    name: &'static str,
    bytes: u64,
    sha256: [u8; 32],
}
struct Pins {
    platform: DevImagePlatform,
    files: [FilePin; 4],
}

// Pins authenticate the delivered engineering bytes, not a release policy.
// Kept separate from release image installation and runtime readiness.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
const MAC_PINS: Pins = Pins {
    platform: DevImagePlatform::MacOsArm64,
    files: [
        FilePin {
            name: "boot-assets.json",
            bytes: 12546,
            sha256: digest(b"2aec683295a0358a9a5b41b45e45db4ef2776ed9526d75bc7bc0d9d1b7c00c13"),
        },
        FilePin {
            name: "os.kernel",
            bytes: 44158912,
            sha256: digest(b"27f833e233753a0a601f3a6792dcd73e2f2c7c8bd30e00b837315c71c5996dcb"),
        },
        FilePin {
            name: "os.initrd",
            bytes: 35865741,
            sha256: digest(b"375394b72b6aeea1e6cc70e00cfba99d6269d0936ece451657449da1899dad93"),
        },
        FilePin {
            name: "os.raw",
            bytes: 17179869184,
            sha256: digest(b"3d4379c1002febfb31b081b92805ffd542eca3ae5e9b65f007e20f1d8656b160"),
        },
    ],
};
#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
const WINDOWS_PINS: Pins = Pins {
    platform: DevImagePlatform::WindowsAmd64,
    files: [
        FilePin {
            name: "boot-assets.json",
            bytes: 12504,
            sha256: digest(b"9d25627b1b5e6bd7952f18f18e8850f531794c8474e7f76aff29affd9a0d8e20"),
        },
        FilePin {
            name: "os.kernel",
            bytes: 14170048,
            sha256: digest(b"4a44487bb3cfdc2be1d7697b8c71c97733d6ab2db62e6e6e8f1c6af9431ffd2e"),
        },
        FilePin {
            name: "os.initrd",
            bytes: 17000871,
            sha256: digest(b"2a05265fb7e7407e389fc7518ded27f876545cd50d2e14d994b2c9e9cf3fd414"),
        },
        FilePin {
            name: "os.qcow2",
            bytes: 3765960704,
            sha256: digest(b"226cfa6e3f7ffc3cde467a3c8e9498aeb1db2aeac68e60c3f27e8f086528850c"),
        },
    ],
};
#[cfg(any(
    test,
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "windows", target_arch = "x86_64")
))]
const fn digest(hex: &[u8; 64]) -> [u8; 32] {
    const fn nibble(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("invalid engineering digest"),
        }
    }
    let mut result = [0; 32];
    let mut i = 0;
    while i < result.len() {
        result[i] = nibble(hex[i * 2]) * 16 + nibble(hex[i * 2 + 1]);
        i += 1;
    }
    result
}

/// Hashes synchronously; callers must use a blocking worker, not a UI thread.
/// Unset is inert; configured invalid or unsupported sources never fall back.
pub fn from_environment() -> Result<Option<VerifiedDevImage>, DevImageError> {
    from_config(
        std::env::var_os("BRIDGE_DEV_IMAGE_DIR"),
        native_pins(),
        |path| Dir::open_ambient_dir(path, cap_std::ambient_authority()),
    )
}
pub fn from_environment_cancellable(
    closed: &tokio::sync::watch::Receiver<bool>,
) -> Result<Option<VerifiedDevImage>, DevImageError> {
    let Some(source) = std::env::var_os("BRIDGE_DEV_IMAGE_DIR") else {
        return Ok(None);
    };
    if source.is_empty() {
        return Err(DevImageError);
    }
    let pins = native_pins().ok_or(DevImageError)?;
    let check = || {
        if *closed.borrow() {
            Err(io::ErrorKind::Interrupted.into())
        } else {
            Ok(())
        }
    };
    check().map_err(|_| DevImageError)?;
    let root = Dir::open_ambient_dir(Path::new(&source), cap_std::ambient_authority())
        .map_err(|_| DevImageError)?;
    verify_checked(&root, pins, &check).map(Some)
}
fn native_pins() -> Option<&'static Pins> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Some(&MAC_PINS)
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Some(&WINDOWS_PINS)
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    {
        None
    }
}
/// Total pinned bytes for this target's engineering image; no I/O or acquisition.
pub(crate) fn pinned_image_bytes() -> Option<u64> {
    image_bytes_for(native_pins()?)
}
fn image_bytes_for(pins: &Pins) -> Option<u64> {
    pins.files
        .iter()
        .try_fold(0u64, |sum, pin| sum.checked_add(pin.bytes))
}
fn from_config(
    source: Option<OsString>,
    pins: Option<&Pins>,
    open: impl FnOnce(&Path) -> io::Result<Dir>,
) -> Result<Option<VerifiedDevImage>, DevImageError> {
    let Some(source) = source else {
        return Ok(None);
    };
    if source.is_empty() {
        return Err(DevImageError);
    }
    let pins = pins.ok_or(DevImageError)?;
    let root = open(Path::new(&source)).map_err(|_| DevImageError)?;
    verify(&root, pins).map(Some)
}
fn verify(root: &Dir, pins: &Pins) -> Result<VerifiedDevImage, DevImageError> {
    verify_checked(root, pins, &|| Ok(()))
}
fn verify_checked(
    root: &Dir,
    pins: &Pins,
    check: &impl Fn() -> io::Result<()>,
) -> Result<VerifiedDevImage, DevImageError> {
    let result = (|| {
        Ok::<_, io::Error>(VerifiedDevImage {
            platform: pins.platform,
            pins: pins.files,
            files: [
                verified_file_checked(root, &pins.files[0], check)?,
                verified_file_checked(root, &pins.files[1], check)?,
                verified_file_checked(root, &pins.files[2], check)?,
                verified_file_checked(root, &pins.files[3], check)?,
            ],
        })
    })();
    result.map_err(|_| DevImageError)
}
#[cfg(test)]
fn verified_file(root: &Dir, pin: &FilePin) -> io::Result<File> {
    verified_file_checked(root, pin, &|| Ok(()))
}
fn verified_file_checked(
    root: &Dir,
    pin: &FilePin,
    check: &impl Fn() -> io::Result<()>,
) -> io::Result<File> {
    check()?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        // Pipes must not block opening; devices must not acquire a terminal.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    // All leaves are fixed private pins, resolved relative to this held Dir.
    // cap-fs-ext enforces no-follow behavior on Windows as well as Unix.
    let mut file = root.open_with(pin.name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != pin.bytes {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = vec![0; 1024 * 1024];
    // One extra byte detects growth while bounding reads even with a writer.
    let mut limited = file.by_ref().take(pin.bytes + 1);
    loop {
        check()?;
        let n = limited.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        hash.update(&buffer[..n]);
    }
    check()?;
    let actual: [u8; 32] = hash.finalize().into();
    if bytes != pin.bytes || actual != pin.sha256 {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    file.rewind()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    #[cfg(debug_assertions)]
    mod installation {
        include!("dev_image_install_tests.rs");
    }
    use super::*;
    use cap_std::{ambient_authority, fs::Dir};
    use sha2::{Digest, Sha256};
    use std::{
        ffi::OsString,
        fs,
        io::{Read, Write},
        path::Path,
    };

    const PLATFORMS: [DevImagePlatform; 2] =
        [DevImagePlatform::MacOsArm64, DevImagePlatform::WindowsAmd64];

    struct Fixture {
        temp: tempfile::TempDir,
        pins: Pins,
        data: [Vec<u8>; 4],
    }

    impl Fixture {
        fn new(platform: DevImagePlatform) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let data = match platform {
                DevImagePlatform::MacOsArm64 => [
                    b"arm engineering receipt\n".to_vec(),
                    b"arm kernel".to_vec(),
                    b"arm initrd".to_vec(),
                    b"arm raw factory".to_vec(),
                ],
                DevImagePlatform::WindowsAmd64 => [
                    b"amd engineering receipt\n".to_vec(),
                    b"amd kernel".to_vec(),
                    b"amd initrd".to_vec(),
                    b"amd qcow2 factory".to_vec(),
                ],
            };
            let names = [
                "boot-assets.json",
                "os.kernel",
                "os.initrd",
                match platform {
                    DevImagePlatform::MacOsArm64 => "os.raw",
                    DevImagePlatform::WindowsAmd64 => "os.qcow2",
                },
            ];
            let files = std::array::from_fn(|i| {
                fs::write(temp.path().join(names[i]), &data[i]).unwrap();
                FilePin {
                    name: names[i],
                    bytes: data[i].len() as u64,
                    sha256: Sha256::digest(&data[i]).into(),
                }
            });
            Self {
                temp,
                pins: Pins { platform, files },
                data,
            }
        }

        fn root(&self) -> Dir {
            Dir::open_ambient_dir(self.temp.path(), ambient_authority()).unwrap()
        }

        fn path(&self, role: usize) -> std::path::PathBuf {
            self.temp.path().join(self.pins.files[role].name)
        }
    }

    fn assert_contents(image: &VerifiedDevImage, fixture: &Fixture) {
        assert_eq!(image.platform(), fixture.pins.platform);
        assert_eq!(
            image.disk_format(),
            match fixture.pins.platform {
                DevImagePlatform::MacOsArm64 => DevImageDiskFormat::Raw,
                DevImagePlatform::WindowsAmd64 => DevImageDiskFormat::Qcow2,
            }
        );
        for (mut file, expected) in [
            image.manifest(),
            image.kernel(),
            image.initrd(),
            image.disk(),
        ]
        .into_iter()
        .zip(&fixture.data)
        {
            let mut actual = Vec::new();
            file.read_to_end(&mut actual).unwrap();
            assert_eq!(&actual, expected);
        }
    }

    #[test]
    fn verification_checks_cancellation_between_hash_reads() {
        let mut fixture = Fixture::new(DevImagePlatform::WindowsAmd64);
        fixture.data[3] = vec![42; 3 * 1024 * 1024];
        fs::write(fixture.path(3), &fixture.data[3]).unwrap();
        fixture.pins.files[3].bytes = fixture.data[3].len() as u64;
        fixture.pins.files[3].sha256 = Sha256::digest(&fixture.data[3]).into();
        let reads = std::cell::Cell::new(0);
        let result = verified_file_checked(&fixture.root(), &fixture.pins.files[3], &|| {
            reads.set(reads.get() + 1);
            if reads.get() >= 3 {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::Interrupted));
        assert_eq!(reads.get(), 3);
    }
    #[test]
    fn delivered_amd_model_image_and_arm_identity_stay_separate() {
        assert_eq!(
            WINDOWS_PINS.files[0].sha256,
            digest(b"9d25627b1b5e6bd7952f18f18e8850f531794c8474e7f76aff29affd9a0d8e20")
        );
        assert_eq!(WINDOWS_PINS.files[2].bytes, 17000871);
        assert_eq!(
            WINDOWS_PINS.files[2].sha256,
            digest(b"2a05265fb7e7407e389fc7518ded27f876545cd50d2e14d994b2c9e9cf3fd414")
        );
        assert_eq!(WINDOWS_PINS.files[3].bytes, 3765960704);
        assert_eq!(
            WINDOWS_PINS.files[3].sha256,
            digest(b"226cfa6e3f7ffc3cde467a3c8e9498aeb1db2aeac68e60c3f27e8f086528850c")
        );
        assert_eq!(
            DevImagePlatform::WindowsAmd64.source_commit(),
            "43e7349115b4c7e9b1865a4d5f0989a02726724c"
        );
        assert_eq!(
            DevImagePlatform::WindowsAmd64.state_compatibility_id(),
            "engineering-43e73491"
        );
        assert_eq!(
            DevImagePlatform::MacOsArm64.source_commit(),
            "43e7349115b4c7e9b1865a4d5f0989a02726724c"
        );
        assert_eq!(
            DevImagePlatform::MacOsArm64.state_compatibility_id(),
            "engineering-43e73491"
        );
        assert_eq!(
            MAC_PINS.files[0].sha256,
            digest(b"2aec683295a0358a9a5b41b45e45db4ef2776ed9526d75bc7bc0d9d1b7c00c13")
        );
    }

    // Reject-all stubs, incorrect handle rewinding, or identity selection break this.
    #[test]
    fn both_platform_layouts_return_verified_rewound_handles() {
        for platform in PLATFORMS {
            let fixture = Fixture::new(platform);
            let image = verify(&fixture.root(), &fixture.pins);
            assert!(image.is_ok(), "valid fixture must verify");
            assert_contents(&image.unwrap(), &fixture);
        }
    }

    // Removing any role's size/hash/open checks accepts at least one mutation.
    #[test]
    fn every_role_rejects_missing_short_long_and_same_size_corruption() {
        for platform in PLATFORMS {
            for role in 0..4 {
                for mutation in 0..4 {
                    let fixture = Fixture::new(platform);
                    let path = fixture.path(role);
                    let mut data = fixture.data[role].clone();
                    match mutation {
                        0 => fs::remove_file(&path).unwrap(),
                        1 => {
                            data.pop();
                            fs::write(&path, data).unwrap();
                        }
                        2 => {
                            data.push(0);
                            fs::write(&path, data).unwrap();
                        }
                        _ => {
                            data[0] ^= 1;
                            fs::write(&path, data).unwrap();
                        }
                    }
                    assert!(
                        verify(&fixture.root(), &fixture.pins).is_err(),
                        "role {role}, mutation {mutation}"
                    );
                }
            }
        }
    }

    #[test]
    fn manifest_is_bound_as_raw_bytes_not_parsed_json() {
        let fixture = Fixture::new(PLATFORMS[0]);
        fs::write(fixture.path(0), b"arm engineering receipt ").unwrap();
        assert!(verify(&fixture.root(), &fixture.pins).is_err());
    }

    #[test]
    fn mixed_platform_payloads_and_wrong_disk_name_are_rejected() {
        for platform in PLATFORMS {
            for role in 0..4 {
                let fixture = Fixture::new(platform);
                let other = Fixture::new(if platform == PLATFORMS[0] {
                    PLATFORMS[1]
                } else {
                    PLATFORMS[0]
                });
                fs::write(fixture.path(role), &other.data[role]).unwrap();
                assert!(verify(&fixture.root(), &fixture.pins).is_err());
            }
            let fixture = Fixture::new(platform);
            fs::rename(
                fixture.path(3),
                fixture.temp.path().join(if platform == PLATFORMS[0] {
                    "os.qcow2"
                } else {
                    "os.raw"
                }),
            )
            .unwrap();
            assert!(verify(&fixture.root(), &fixture.pins).is_err());
        }
    }

    #[test]
    fn directories_are_rejected_for_every_role() {
        for platform in PLATFORMS {
            for role in 0..4 {
                let fixture = Fixture::new(platform);
                fs::remove_file(fixture.path(role)).unwrap();
                fs::create_dir(fixture.path(role)).unwrap();
                assert!(verify(&fixture.root(), &fixture.pins).is_err());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_leaves_are_rejected_even_when_target_bytes_match() {
        for platform in PLATFORMS {
            for role in 0..4 {
                let fixture = Fixture::new(platform);
                let target = fixture.temp.path().join("target");
                fs::rename(fixture.path(role), &target).unwrap();
                std::os::unix::fs::symlink(&target, fixture.path(role)).unwrap();
                assert!(verify(&fixture.root(), &fixture.pins).is_err());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn fifos_are_rejected_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        for platform in PLATFORMS {
            for role in 0..4 {
                let fixture = Fixture::new(platform);
                fs::remove_file(fixture.path(role)).unwrap();
                let path =
                    std::ffi::CString::new(fixture.path(role).as_os_str().as_bytes()).unwrap();
                // SAFETY: path is a valid NUL-terminated temporary fixture path.
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                assert!(verify(&fixture.root(), &fixture.pins).is_err());
            }
        }
    }

    #[test]
    fn verification_never_changes_source_and_returned_handles_cannot_write() {
        for platform in PLATFORMS {
            let fixture = Fixture::new(platform);
            let image = verify(&fixture.root(), &fixture.pins).unwrap();
            for mut file in [
                image.manifest(),
                image.kernel(),
                image.initrd(),
                image.disk(),
            ] {
                assert!(file.write_all(b"X").is_err());
            }
            assert_eq!(fs::read_dir(fixture.temp.path()).unwrap().count(), 4);
            for role in 0..4 {
                assert_eq!(fs::read(fixture.path(role)).unwrap(), fixture.data[role]);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn retained_handles_survive_root_rename_and_replacement() {
        for platform in PLATFORMS {
            let fixture = Fixture::new(platform);
            let root = fixture.root();
            let image = verify(&root, &fixture.pins).unwrap();
            let parent = tempfile::tempdir().unwrap();
            fs::rename(fixture.temp.path(), parent.path().join("retained")).unwrap();
            fs::create_dir(fixture.temp.path()).unwrap();
            for pin in &fixture.pins.files {
                fs::write(fixture.temp.path().join(pin.name), b"replacement").unwrap();
            }
            assert_contents(&image, &fixture);
        }
    }

    #[test]
    fn unset_configuration_does_not_open_any_source() {
        assert!(from_config(None, None, |_| panic!("unset must not open"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn configured_unsupported_host_and_empty_source_do_not_open() {
        let fixture = Fixture::new(PLATFORMS[0]);
        assert!(
            from_config(Some(OsString::from("private-source")), None, |_| panic!(
                "unsupported must not open"
            ))
            .is_err()
        );
        assert!(
            from_config(Some(OsString::new()), Some(&fixture.pins), |_| panic!(
                "empty must not open"
            ))
            .is_err()
        );
    }

    #[test]
    fn configured_source_uses_verified_handles_and_sanitizes_errors() {
        let fixture = Fixture::new(PLATFORMS[0]);
        let source = fixture.temp.path().as_os_str().to_owned();
        let image = from_config(Some(source.clone()), Some(&fixture.pins), |path| {
            assert_eq!(path, Path::new(&source));
            Dir::open_ambient_dir(path, ambient_authority())
        })
        .unwrap()
        .unwrap();
        assert_contents(&image, &fixture);
        let error = from_config(Some(source), Some(&fixture.pins), |_| {
            Err(std::io::Error::other("sensitive path and details"))
        })
        .err()
        .unwrap();
        assert_eq!(error.to_string(), "debug local image unavailable");
        assert_eq!(format!("{error:?}"), "debug local image unavailable");
    }

    #[test]
    fn streamed_verification_covers_data_past_one_mebibyte() {
        let mut fixture = Fixture::new(PLATFORMS[0]);
        fixture.data[3] = vec![0x5a; 1024 * 1024 + 19];
        fixture.pins.files[3].bytes = fixture.data[3].len() as u64;
        fixture.pins.files[3].sha256 = Sha256::digest(&fixture.data[3]).into();
        fs::write(fixture.path(3), &fixture.data[3]).unwrap();
        assert_contents(&verify(&fixture.root(), &fixture.pins).unwrap(), &fixture);
        let mut corrupt = fixture.data[3].clone();
        *corrupt.last_mut().unwrap() ^= 1;
        fs::write(fixture.path(3), corrupt).unwrap();
        assert!(verify(&fixture.root(), &fixture.pins).is_err());
    }

    #[test]
    fn engineering_pins_do_not_accept_synthetic_fixture_authority() {
        for pins in [&MAC_PINS, &WINDOWS_PINS] {
            let fixture = Fixture::new(pins.platform);
            assert!(verify(&fixture.root(), pins).is_err());
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(
            native_pins().unwrap().platform,
            DevImagePlatform::MacOsArm64
        );
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        assert_eq!(
            native_pins().unwrap().platform,
            DevImagePlatform::WindowsAmd64
        );
    }

    #[test]
    fn pinned_image_bytes_sum_each_platform_pins() {
        assert_eq!(image_bytes_for(&MAC_PINS), Some(17_259_906_383));
        assert_eq!(image_bytes_for(&WINDOWS_PINS), Some(3_797_144_127));
        assert_eq!(
            pinned_image_bytes(),
            native_pins().and_then(image_bytes_for)
        );
    }

    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    #[test]
    fn native_pins_are_unavailable_on_other_targets() {
        assert!(native_pins().is_none());
    }
}
