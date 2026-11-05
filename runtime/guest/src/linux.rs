//! Fixed-purpose Linux guest operations; never called by filesystem unit adapters.
use crate::diagnostics::{self, Context};
use crate::early::{invalid, validate, BootSystem, Disk, Identity};
use bridge_runtime_protocol::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const RUN: &str = "/run/bridge";
pub const PORT: &str = "/dev/virtio-ports/ai.orka.bridge.control";
pub const SOCKET: &str = "/run/bridge/kmx.sock";
const BOOT: &str = "/run/bridge/boot.json";
pub struct Linux {
    root: &'static str,
    device: PathBuf,
}
impl Linux {
    pub fn early() -> io::Result<Self> {
        const STAGE: &str = "early";
        private_run(STAGE)?;
        if fs::symlink_metadata(BOOT).is_ok() {
            diagnostics::log(
                Context::new(STAGE, "boot-exists"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
        Ok(Self {
            root: "/root",
            device: PathBuf::new(),
        })
    }
    pub fn serve() -> io::Result<Self> {
        const STAGE: &str = "serve";
        private_run(STAGE)?;
        Ok(Self {
            root: "",
            device: PathBuf::new(),
        })
    }
    fn stage(&self) -> &'static str {
        if self.root == "/root" {
            "early"
        } else {
            "serve"
        }
    }
    fn state(&self) -> PathBuf {
        PathBuf::from(format!("{}/var/lib/bridge", self.root))
    }
    fn target(&self, suffix: &str) -> String {
        format!("{}{suffix}", self.root)
    }
    pub fn accepted() -> io::Result<BootDescriptor> {
        const STAGE: &str = "serve";
        let b: BootDescriptor = match serde_json::from_slice(&read_regular(Path::new(BOOT), 4096)?)
        {
            Ok(b) => b,
            Err(_) => {
                diagnostics::log(
                    Context::new(STAGE, "accepted-decode"),
                    io::ErrorKind::InvalidData,
                    None,
                );
                return Err(invalid());
            }
        };
        if let Err(e) = validate(&b) {
            diagnostics::note(Context::new(STAGE, "accepted-validate"), &e);
            return Err(e);
        }
        Ok(b)
    }
    pub fn set_clock(b: &BootDescriptor) -> io::Result<()> {
        const STAGE: &str = "early";
        let time = libc::timespec {
            tv_sec: match b.host_utc.try_into() {
                Ok(s) => s,
                Err(_) => {
                    diagnostics::log(
                        Context::new(STAGE, "clock-convert"),
                        io::ErrorKind::InvalidInput,
                        None,
                    );
                    return Err(invalid());
                }
            },
            tv_nsec: 0,
        };
        // SAFETY: the initialized timespec lives through the system call.
        if unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &time) } != 0 {
            let e = io::Error::last_os_error();
            diagnostics::note(Context::new(STAGE, "clock-settime"), &e);
            return Err(e);
        }
        Ok(())
    }
    pub fn request_poweroff() -> io::Result<()> {
        checked("serve", "/usr/bin/systemctl", &["--no-block", "poweroff"]).map(|_| ())
    }
}
fn private_run(stage: &'static str) -> io::Result<()> {
    diagnostics::context(
        Context::new(stage, "run-dir"),
        directory(Path::new(RUN), true),
    )?;
    diagnostics::context(
        Context::new(stage, "run-perms"),
        fs::set_permissions(RUN, fs::Permissions::from_mode(0o700)),
    )?;
    mount_is(stage, RUN, "tmpfs", None, false)
}
fn bind_source(expected: &Path, target: &Path) -> io::Result<()> {
    let source = fs::symlink_metadata(expected)?;
    let mounted = fs::symlink_metadata(target)?;
    if source.file_type().is_symlink()
        || mounted.file_type().is_symlink()
        || source.dev() != mounted.dev()
        || source.ino() != mounted.ino()
    {
        return Err(invalid());
    }
    Ok(())
}
fn target_directory(path: &Path) -> io::Result<()> {
    directory(path, false)
}
fn directory(path: &Path, create: bool) -> io::Result<()> {
    if create && !path.exists() {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let m = fs::symlink_metadata(path)?;
    if !m.file_type().is_dir() {
        return Err(invalid());
    }
    Ok(())
}
use std::os::unix::fs::DirBuilderExt;
pub fn read_regular(path: &Path, limit: u64) -> io::Result<Vec<u8>> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)?;
    if !f.metadata()?.file_type().is_file() {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    (&mut f).take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid());
    }
    Ok(bytes)
}
fn create_file(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    File::open(path.parent().ok_or_else(invalid)?)?.sync_all()
}
fn run(stage: &'static str, program: &str, args: &[&str]) -> io::Result<(i32, Vec<u8>)> {
    let mut child = match Command::new(program)
        .args(args)
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            diagnostics::note(Context::new(stage, "command-spawn"), &e);
            return Err(e);
        }
    };
    let output = child.stdout.take().ok_or_else(|| {
        diagnostics::log(
            Context::new(stage, "command-stdout-pipe"),
            io::ErrorKind::Other,
            None,
        );
        invalid()
    })?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        output.take(4097).read_to_end(&mut b).map(|_| b)
    });
    let errors = child.stderr.take().ok_or_else(|| {
        diagnostics::log(
            Context::new(stage, "command-stderr-pipe"),
            io::ErrorKind::Other,
            None,
        );
        invalid()
    })?;
    let error_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        errors.take(4097).read_to_end(&mut b).map(|_| b)
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                diagnostics::log(
                    Context::new(stage, "command-timeout"),
                    io::ErrorKind::TimedOut,
                    None,
                );
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = error_reader.join();
                return Err(invalid());
            }
            Err(e) => {
                diagnostics::note(Context::new(stage, "command-wait"), &e);
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = error_reader.join();
                return Err(invalid());
            }
        }
    };
    let bytes = match reader.join() {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            diagnostics::note(Context::new(stage, "command-stdout-read"), &e);
            return Err(e);
        }
        Err(_) => {
            diagnostics::log(
                Context::new(stage, "command-stdout-read"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
    };
    let errors = match error_reader.join() {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            diagnostics::note(Context::new(stage, "command-stderr-read"), &e);
            return Err(e);
        }
        Err(_) => {
            diagnostics::log(
                Context::new(stage, "command-stderr-read"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
    };
    if bytes.len() > 4096 {
        diagnostics::log(
            Context::new(stage, "command-stdout-limit"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    if !errors.is_empty() {
        diagnostics::log(
            Context::new(stage, "command-stderr"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    let code = match status.code() {
        Some(c) => c,
        None => {
            diagnostics::log(
                Context::new(stage, "command-signal"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
    };
    Ok((code, bytes))
}
fn checked(stage: &'static str, program: &str, args: &[&str]) -> io::Result<Vec<u8>> {
    let (status, b) = run(stage, program, args)?;
    if status != 0 {
        diagnostics::log(
            Context::new(stage, "command-status"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    Ok(b)
}
fn text(b: Vec<u8>) -> io::Result<String> {
    String::from_utf8(b)
        .map(|s| s.trim().to_owned())
        .map_err(|_| invalid())
}
fn mount_is(
    stage: &'static str,
    path: &str,
    kind: &str,
    uuid: Option<Uuid>,
    exact: bool,
) -> io::Result<()> {
    let selector = if exact { "-M" } else { "-T" };
    if text(checked(
        stage,
        "/usr/bin/findmnt",
        &["-rn", selector, path, "-o", "FSTYPE"],
    )?)? != kind
    {
        diagnostics::log(
            Context::new(stage, "mount-fstype-mismatch"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    if let Some(id) = uuid {
        if text(checked(
            stage,
            "/usr/bin/findmnt",
            &["-rn", selector, path, "-o", "UUID"],
        )?)? != id.to_string()
        {
            diagnostics::log(
                Context::new(stage, "mount-uuid-mismatch"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
    }
    Ok(())
}
fn machine_id_mount_target(stage: &'static str, target: &str) -> io::Result<String> {
    diagnostics::context(
        Context::new(stage, "ready-machine-id-findmnt"),
        checked(
            stage,
            "/usr/bin/findmnt",
            &["-rn", "-T", target, "-o", "TARGET"],
        )
        .and_then(text),
    )
}
impl BootSystem for Linux {
    fn probe(&mut self) -> io::Result<Disk> {
        self.device = fs::canonicalize("/dev/disk/by-id/virtio-bridge-state")?;
        if !self.device.starts_with("/dev")
            || !fs::metadata(&self.device)?.file_type().is_block_device()
        {
            return Err(invalid());
        }
        let path = self.device.to_str().ok_or_else(invalid)?;
        let bytes = text(checked(
            self.stage(),
            "/sbin/blockdev",
            &["--getsize64", path],
        )?)?
        .parse::<u64>()
        .map_err(|_| invalid())?;
        // A probe error on an unreadable device must not be interpreted as blank.
        let mut header = [0; 4096];
        File::open(&self.device)?.read_exact(&mut header)?;
        let (status, output) = run(self.stage(), "/sbin/blkid", &["-p", "-o", "export", path])?;
        parse_probe(status, &output, bytes)
    }
    fn format(&mut self, id: Uuid) -> io::Result<()> {
        checked(
            self.stage(),
            "/sbin/mkfs.ext4",
            &[
                "-q",
                "-U",
                &id.to_string(),
                self.device.to_str().ok_or_else(invalid)?,
            ],
        )
        .map(|_| ())
    }
    fn mount_state(&mut self) -> io::Result<()> {
        let state = self.state();
        target_directory(&state)?;
        checked(
            self.stage(),
            "/bin/mount",
            &[
                "-t",
                "ext4",
                "-o",
                "rw",
                self.device.to_str().ok_or_else(invalid)?,
                state.to_str().ok_or_else(invalid)?,
            ],
        )
        .map(|_| ())
    }
    fn identity(&mut self) -> io::Result<Option<Identity>> {
        match read_regular(&self.state().join("identity.json"), 4096) {
            Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|_| invalid()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn initialize(&mut self, id: &Identity) -> io::Result<()> {
        for name in ["docker", "containerd"] {
            directory(&self.state().join(name), true)?;
        }
        create_file(
            &self.state().join("machine-id"),
            format!("{}\n", id.installation_id.simple()).as_bytes(),
            0o444,
        )?;
        create_file(
            &self.state().join("identity.json"),
            &serde_json::to_vec(id).map_err(|_| invalid())?,
            0o600,
        )
    }
    fn bind_persistent(&mut self) -> io::Result<()> {
        let machine = read_regular(&self.state().join("machine-id"), 33)?;
        let id = std::str::from_utf8(&machine).map_err(|_| invalid())?.trim();
        if id.len() != 32
            || !id.bytes().all(|c| c.is_ascii_hexdigit())
            || id.bytes().all(|c| c == b'0')
        {
            return Err(invalid());
        }
        for name in ["machine-id", "docker", "containerd"] {
            let source = self.state().join(name);
            let target = self.target(if name == "machine-id" {
                "/etc/machine-id"
            } else if name == "docker" {
                "/var/lib/docker"
            } else {
                "/var/lib/containerd"
            });
            if name != "machine-id" {
                directory(&source, false)?;
                target_directory(Path::new(&target))?;
            } else if !fs::symlink_metadata(&target)?.file_type().is_file() {
                return Err(invalid());
            }
            checked(
                self.stage(),
                "/bin/mount",
                &["--bind", source.to_str().ok_or_else(invalid)?, &target],
            )?;
        }
        Ok(())
    }
    fn credentials(&mut self) -> io::Result<()> {
        let path = "/run/bridge/credentials";
        directory(Path::new(path), true)?;
        checked(
            self.stage(),
            "/bin/mount",
            &[
                "-t",
                "tmpfs",
                "-o",
                "mode=0700,nosuid,nodev,noexec",
                "tmpfs",
                path,
            ],
        )?;
        mount_is(self.stage(), path, "tmpfs", None, true)
    }
    fn ready(&mut self, b: &BootDescriptor) -> io::Result<()> {
        let stage = self.stage();
        if let Err(e) = validate(b) {
            diagnostics::note(Context::new(stage, "ready-validate"), &e);
            return Err(e);
        }
        diagnostics::context(
            Context::new(stage, "ready-state-mount"),
            mount_is(
                stage,
                self.state().to_str().ok_or_else(invalid)?,
                "ext4",
                Some(b.state_fs_uuid),
                true,
            ),
        )?;
        for name in ["docker", "containerd"] {
            let target = self.target(&format!("/var/lib/{name}"));
            let (mount_phase, bind_phase) = if name == "docker" {
                ("ready-docker-mount", "ready-docker-bind-source")
            } else {
                ("ready-containerd-mount", "ready-containerd-bind-source")
            };
            diagnostics::context(
                Context::new(stage, mount_phase),
                mount_is(stage, &target, "ext4", Some(b.state_fs_uuid), true),
            )?;
            diagnostics::context(
                Context::new(stage, bind_phase),
                bind_source(&self.state().join(name), Path::new(&target)),
            )?;
        }
        let target = self.target("/etc/machine-id");
        if machine_id_mount_target(stage, &target)? != target {
            diagnostics::log(
                Context::new(stage, "ready-machine-id-target"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
        diagnostics::context(
            Context::new(stage, "ready-machine-id-mount"),
            mount_is(stage, &target, "ext4", Some(b.state_fs_uuid), false),
        )?;
        diagnostics::context(
            Context::new(stage, "ready-machine-id-bind-source"),
            bind_source(&self.state().join("machine-id"), Path::new(&target)),
        )?;
        diagnostics::context(
            Context::new(stage, "ready-credentials-mount"),
            mount_is(stage, "/run/bridge/credentials", "tmpfs", None, true),
        )?;
        let identity = match self.identity() {
            Ok(Some(id)) => id,
            Ok(None) => {
                diagnostics::log(
                    Context::new(stage, "ready-identity-missing"),
                    io::ErrorKind::NotFound,
                    None,
                );
                return Err(invalid());
            }
            Err(e) => {
                diagnostics::note(Context::new(stage, "ready-identity"), &e);
                return Err(e);
            }
        };
        if identity.schema_version != "bridge-state/v1"
            || identity.installation_id != b.installation_id
        {
            diagnostics::log(
                Context::new(stage, "ready-identity-mismatch"),
                io::ErrorKind::Other,
                None,
            );
            return Err(invalid());
        }
        Ok(())
    }
    fn shutdown(&mut self) -> io::Result<()> {
        Self::request_poweroff()
    }
    fn save_boot(&mut self, b: &BootDescriptor) -> io::Result<()> {
        create_file(
            Path::new(BOOT),
            &serde_json::to_vec(b).map_err(|_| invalid())?,
            0o600,
        )
    }
}
fn parse_probe(status: i32, output: &[u8], bytes: u64) -> io::Result<Disk> {
    if status == 2 && output.is_empty() {
        return Ok(Disk {
            bytes,
            fs_uuid: None,
            blank: true,
        });
    }
    if status != 0 {
        return Err(invalid());
    }
    let s = std::str::from_utf8(output).map_err(|_| invalid())?;
    let fields: Vec<_> = s.lines().collect();
    if fields.iter().any(|f| f.starts_with("PTTYPE=")) || !fields.contains(&"TYPE=ext4") {
        return Err(invalid());
    }
    let ids: Vec<_> = fields
        .iter()
        .filter_map(|f| f.strip_prefix("UUID="))
        .collect();
    if ids.len() != 1 {
        return Err(invalid());
    }
    Ok(Disk {
        bytes,
        fs_uuid: Some(Uuid::parse_str(ids[0]).map_err(|_| invalid())?),
        blank: false,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifo_identity_is_rejected_without_waiting_for_a_writer() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, sync::mpsc};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("identity.json");
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (tx, rx) = mpsc::channel();
        let input = path.clone();
        let reader = std::thread::spawn(move || {
            let _ = tx.send(read_regular(&input, 4096));
        });
        let answer = rx.recv_timeout(Duration::from_secs(2));
        if answer.is_err() {
            let _ = OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path);
        }
        reader.join().unwrap();
        assert!(answer.expect("nonregular open blocked").is_err());
    }
    #[test]
    fn exact_persistent_bind_sources_are_required() {
        let temp = tempfile::tempdir().unwrap();
        let docker = temp.path().join("docker");
        let containerd = temp.path().join("containerd");
        fs::create_dir(&docker).unwrap();
        fs::create_dir(&containerd).unwrap();
        assert!(bind_source(&docker, &containerd).is_err());
        let source = temp.path().join("machine-id");
        let target = temp.path().join("etc-machine-id");
        fs::write(&source, b"machine").unwrap();
        fs::hard_link(&source, &target).unwrap();
        bind_source(&source, &target).unwrap();
        let wrong = temp.path().join("unrelated");
        fs::write(&wrong, b"machine").unwrap();
        assert!(bind_source(&source, &wrong).is_err());
    }
    #[test]
    fn missing_mount_target_is_not_created() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert!(target_directory(&missing).is_err());
        assert!(!missing.exists());
        target_directory(temp.path()).unwrap();
    }
    #[test]
    fn probe_tool_io_error_is_not_blank() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing");
        assert!(run("serve", "/bin/ls", &[missing.to_str().unwrap()]).is_err());
    }
    #[test]
    fn diagnostic_command_child() {
        if std::env::var_os("BRIDGE_TEST_DIAGNOSTIC_COMMAND").is_none() {
            return;
        }
        let err = run("early", "/nonexistent/definitely-not-a-binary", &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(err.raw_os_error(), Some(2));
        assert!(run(
            "serve",
            "/bin/sh",
            &["-c", "printf synthetic-credential-do-not-log >&2; exit 3"]
        )
        .is_err());
        assert!(checked("serve", "/bin/sh", &["-c", "exit 3"]).is_err());
        assert!(machine_id_mount_target("serve", "/dev/null/not-a-directory").is_err());
    }
    #[test]
    fn command_failure_logs_keep_context_without_content() {
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::tests::diagnostic_command_child",
                "--nocapture",
            ])
            .env("BRIDGE_TEST_DIAGNOSTIC_COMMAND", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(!stderr.contains("synthetic-credential-do-not-log"));
        assert!(!stderr.contains("/nonexistent"));
        assert_eq!(
            stderr.lines().collect::<Vec<_>>(),
            [
                "bridge-guest: stage=early phase=command-spawn kind=NotFound errno=2",
                "bridge-guest: stage=serve phase=command-stderr kind=Other errno=none",
                "bridge-guest: stage=serve phase=command-status kind=Other errno=none",
                "bridge-guest: stage=serve phase=command-status kind=Other errno=none",
                "bridge-guest: stage=serve phase=ready-machine-id-findmnt kind=Other errno=none",
            ]
        );
    }
    #[test]
    fn caller_stage_tracks_the_runtime_root() {
        for (root, stage) in [("/root", "early"), ("", "serve")] {
            assert_eq!(
                Linux {
                    root,
                    device: PathBuf::new()
                }
                .stage(),
                stage
            );
        }
    }
    #[test]
    fn probe_rejects_foreign_ambiguous_and_bad_bytes() {
        assert!(parse_probe(8, b"", STATE_BYTES).is_err());
        assert!(parse_probe(0, b"TYPE=ext4\nPTTYPE=gpt\n", STATE_BYTES).is_err());
        assert!(parse_probe(0, b"TYPE=ext4\nUUID=bad\n", STATE_BYTES).is_err());
        assert!(parse_probe(2, b"error", STATE_BYTES).is_err());
        assert!(parse_probe(2, b"", STATE_BYTES).unwrap().blank);
    }
}
