use super::*;

// Exact guest versions of paired cancel-capable guest/Orka images. The image
// pin update adds its qualified buildSourceCommit; existing images lack cancel.
#[cfg(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
fn run_cancel_guest(build: &str) -> bool {
    const BUILDS: &[&str] = &[];
    BUILDS.contains(&build)
}
#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
#[path = "local_vm_image.rs"]
mod image;
#[cfg(test)]
use crate::runtime::types::BackendKind;
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
use crate::runtime::windows::inputs::{verify_windows_runtime, FilePin};
#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
use crate::runtime::{
    image_install::InstalledImageSet,
    journal::{Journal, JournalCustody},
};
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
use std::io::Read;
use std::path::PathBuf;
#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
use std::{fs, path::Path};

#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
async fn blocking_start(
    work: impl FnOnce(tokio::runtime::Handle) -> Result<Box<dyn LocalSession>, RuntimeError>
        + Send
        + 'static,
) -> Result<Box<dyn LocalSession>, RuntimeError> {
    let runtime = tokio::runtime::Handle::current();
    // Only async image operations may enter block_on. Synchronous backend
    // calls must run outside that context, including inheritable-handle guards.
    tokio::task::spawn_blocking(move || work(runtime))
        .await
        .map_err(|_| RuntimeError::Unavailable)?
}

#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
fn wait_for_infrastructure(
    mut session: Box<dyn LocalSession>,
    mut closed: watch::Receiver<bool>,
    runtime: tokio::runtime::Handle,
) -> Result<Box<dyn LocalSession>, RuntimeError> {
    use bridge_runtime_protocol::{BootstrapPhase, InfrastructureService};
    let result = (|| loop {
        if *closed.borrow() {
            return Err(RuntimeError::Closed);
        }
        let mut observed = None;
        session.with_client(&mut |_, client| {
            observed = Some(client.readiness()?);
            Ok(())
        })?;
        // Never publish readiness after a close admitted during synchronous I/O.
        if *closed.borrow() {
            return Err(RuntimeError::Closed);
        }
        let state = observed.ok_or(RuntimeError::Unavailable)?;
        match state.service {
            InfrastructureService::Ready
                if state.phase == Some(BootstrapPhase::InfrastructureReady) =>
            {
                return Ok(())
            }
            InfrastructureService::Starting => {}
            _ => {
                return Err(RuntimeError::Infrastructure(format!(
                    "Orka infrastructure failed (phase: {:?}, service: {:?})",
                    state.phase, state.service
                )))
            }
        }
        // Time only schedules the next observation; it never grants readiness.
        runtime.block_on(async {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => Ok(()),
                _ = closed.changed() => Err(RuntimeError::Closed),
            }
        })?;
    })();
    match result {
        Ok(()) => Ok(session),
        Err(error) => {
            if let Err(cleanup) = session.stop() {
                // Keep the entire session, not just its error, until process exit.
                std::mem::forget(session);
                return Err(RuntimeError::StopUnconfirmed(format!(
                    "Orka infrastructure startup failed: {}; stopping the VM failed: {}",
                    failure_reason(&error),
                    failure_reason(&cleanup)
                )));
            }
            Err(error)
        }
    }
}

#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn retain_start_failure(
    root: &Path,
    stage: &str,
    error: RuntimeError,
    stderr: std::io::Result<Vec<u8>>,
    operation: WorkspaceOperation,
) -> RuntimeError {
    use std::io::Write;
    let unconfirmed = matches!(error, RuntimeError::StopUnconfirmed(_));
    let reason = failure_reason(&error);
    let (stderr, capture_error) = match stderr {
        Ok(bytes) => (bytes, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let directory = root.join(format!(
        "startup-failure-{}",
        bridge_runtime_protocol::Uuid::new_v4()
    ));
    let saved = (|| -> std::io::Result<()> {
        // Fresh names never overwrite evidence or follow an existing file.
        #[cfg(windows)]
        let _held = crate::runtime::windows::security::create_af_directory(&directory)?;
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
        }
        for (name, bytes) in [
            ("qemu-stderr.txt", stderr.clone()),
            (
                "failure.json",
                serde_json::to_vec_pretty(&serde_json::json!({
                    "stage": stage, "reason": reason, "stderrCaptureError": capture_error,
                }))?,
            ),
        ] {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(directory.join(name))?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        Ok(())
    })();
    let diagnostic: String = String::from_utf8_lossy(&stderr)
        .trim()
        .chars()
        .take(2048)
        .collect();
    let mut message = format!("{stage}: {reason}");
    if !diagnostic.is_empty() {
        message.push_str(&format!(". QEMU: {diagnostic}"));
    }
    match saved {
        Ok(()) => message.push_str(&format!(". Details: {}", directory.display())),
        Err(error) => message.push_str(&format!(". Could not save diagnostics: {error}")),
    }
    if unconfirmed {
        RuntimeError::StopUnconfirmed(message)
    } else {
        // Keep the original typed error; only the display text is private.
        RuntimeError::Startup(Box::new(crate::runtime::types::StartupFailure {
            error,
            operation,
            diagnostic: message,
        }))
    }
}

/// Native selection is Rust-owned; only debug builds may select development paths.
pub fn configured(resources: Option<PathBuf>, data: Option<PathBuf>) -> Arc<LocalVm> {
    #[cfg(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    ))]
    if let (Some(resources), Some(data)) = (&resources, &data) {
        let root = data.join("local-vm-v1");
        #[cfg(windows)]
        let target = crate::runtime::image_types::ImageTarget::WindowsAmd64;
        #[cfg(target_os = "macos")]
        let target = crate::runtime::image_types::ImageTarget::MacosArm64;
        let release = image::shipping_image(target).and_then(|profile| {
            image::native_host_target(target)
                .ok()
                .map(|host| (profile, host))
        });
        #[cfg(debug_assertions)]
        let development = (
            std::env::var_os("BRIDGE_DEV_IMAGE_DIR").is_some(),
            root.exists(),
        );
        #[cfg(not(debug_assertions))]
        let development = (false, false);
        if let Some(source) = image::select_source(
            cfg!(debug_assertions),
            development.0,
            development.1,
            release,
        ) {
            return LocalVm::new(Arc::new(NativeFactory {
                resources: resources.clone(),
                root,
                cache: data.join("image-cache-v1"),
                source,
                pending: Arc::new(Mutex::new(JournalCustody(None))),
            }));
        }
    }
    let _ = (resources, data);
    LocalVm::unavailable()
}
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn runtime_manifest_pin() -> FilePin {
    FilePin {
        name: "provenance/runtime-manifest.json".into(),
        bytes: 117615,
        sha256: "a1dead19c0d1dcef558008e556cd9bd4a8c5082daf4ecea6f531d03173748385".into(),
    }
}
#[cfg(any(test, all(windows, target_arch = "x86_64")))]
fn read_runtime_manifest(root: &Path, pin: &FilePin) -> Result<Vec<FilePin>, RuntimeError> {
    use sha2::{Digest, Sha256};
    #[derive(serde::Deserialize)]
    struct Manifest {
        files: Vec<Entry>,
    }
    #[derive(serde::Deserialize)]
    struct Entry {
        path: String,
        bytes: u64,
        sha256: String,
    }
    let path = root.join(&pin.name);
    if !fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(RuntimeError::Unavailable);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() != pin.bytes {
        return Err(RuntimeError::Unavailable);
    }
    let mut bytes = Vec::new();
    file.take(pin.bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != pin.bytes || format!("{:x}", Sha256::digest(&bytes)) != pin.sha256 {
        return Err(RuntimeError::Unavailable);
    }
    // The compiled hash authenticates every producer row before projection.
    let manifest: Manifest =
        serde_json::from_slice(&bytes).map_err(|_| RuntimeError::Unavailable)?;
    let mut pins: Vec<_> = manifest
        .files
        .into_iter()
        .map(|e| FilePin {
            name: e.path,
            bytes: e.bytes,
            sha256: e.sha256,
        })
        .collect();
    pins.push(pin.clone()); // Nested provenance manifest is itself in the exact inventory.
    Ok(pins)
}
#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
fn reopen(
    root: &Path,
    source: &mut image::ImageSource,
) -> Result<Option<(Journal, Arc<InstalledImageSet>)>, RuntimeError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => {
            let journal = Journal::open(root)?;
            let image = image::open_selected(source, &journal)?;
            Ok(Some((journal, image)))
        }
    }
}
#[cfg(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
struct NativeFactory {
    resources: PathBuf,
    root: PathBuf,
    cache: PathBuf,
    source: image::ImageSource,
    pending: Arc<Mutex<JournalCustody>>,
}
#[cfg(any(
    test,
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
impl JournalCustody {
    fn take_for_launch(&mut self, closed: &watch::Receiver<bool>) -> Result<Journal, RuntimeError> {
        // Keep original creation authority through the very last cancellation checkpoint.
        if *closed.borrow() {
            return Err(RuntimeError::Closed);
        }
        self.0.take().ok_or(RuntimeError::Unavailable)
    }
}
#[cfg(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
impl LocalFactory for NativeFactory {
    fn launch(
        &self,
        closed: watch::Receiver<bool>,
        progress: Arc<dyn Fn(LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        let (resources, root, pending, source, cache) = (
            self.resources.clone(),
            self.root.clone(),
            self.pending.clone(),
            self.source.clone(),
            self.cache.clone(),
        );
        Box::pin(blocking_start(move |runtime| {
            use crate::runtime::{self, types::BackendKind};
            if *closed.borrow() {
                return Err(RuntimeError::Closed);
            }
            crate::setup::preflight::before_image_work(
                crate::setup::preflight::observe_native(),
                || {
                    #[cfg(windows)]
                    let inputs = {
                        runtime::windows::launch::control_path(
                            &root,
                            bridge_runtime_protocol::Uuid::nil(),
                        )?;
                        let path = resources.join("native/qemu");
                        Arc::new(verify_windows_runtime(
                            &read_runtime_manifest(&path, &runtime_manifest_pin())?,
                            path,
                        )?)
                    };
                    // The factory retains the original owner through preparation retries.
                    let mut custody = pending.lock().map_err(|_| RuntimeError::Unavailable)?;
                    let installed = image::prepare_installation(
                        &source,
                        &root,
                        &cache,
                        &mut custody,
                        closed.clone(),
                        |event| progress(event),
                        || {
                            #[cfg(windows)]
                            let (backend, identity) = (BackendKind::Qemu, b"pc-q35-11.0".to_vec());
                            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                            let (backend, identity) = (
                                BackendKind::Apple,
                                runtime::apple_input::machine_identifier(&resources)?,
                            );
                            if *closed.borrow() {
                                return Err(RuntimeError::Closed);
                            }
                            fs::create_dir_all(root.parent().ok_or(RuntimeError::Unavailable)?)?;
                            Journal::create(&root, backend, identity)
                        },
                        &runtime,
                    )?;
                    progress(LocalProgress::Status(LocalVmStatus::Starting));
                    let utc = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|_| RuntimeError::Unavailable)?
                        .as_secs();
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let installation = installed.installation_id();
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let cancel_supported =
                        run_cancel_guest(&installed.identity().build_source_commit);
                    let journal = custody.take_for_launch(&closed)?;
                    drop(custody); // A stopped worker returns the original owner through this slot.
                    #[cfg(windows)]
                    let native =
                        start_windows_session(inputs, journal, installed, &root, utc, pending)?;
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let session = runtime::apple_session::AppleVmSession::launch_retaining(
                        resources, journal, installed, utc, pending,
                    )?;
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    let native = NativeSession(session, installation, cancel_supported);
                    wait_for_infrastructure(Box::new(native), closed, runtime)
                },
            )?
        }))
    }
}
#[cfg(all(windows, target_arch = "x86_64"))]
fn start_windows_session(
    inputs: Arc<crate::runtime::windows::inputs::VerifiedWindowsInputs>,
    journal: Journal,
    image: Arc<InstalledImageSet>,
    root: &Path,
    utc: u64,
    custody: Arc<Mutex<JournalCustody>>,
) -> Result<NativeSession, RuntimeError> {
    // Keep the verified private root non-replaceable even if failed startup or
    // shutdown releases its worker before diagnostics are written.
    let diagnostic_root = crate::runtime::windows::security::private_af_directory(root)?;
    let installation = image.installation_id();
    let cancel_supported = run_cancel_guest(&image.identity().build_source_commit);
    let backend = crate::runtime::windows::backend::WindowsBackend::new(inputs);
    let failure = backend.failure_diagnostics();
    let result =
        crate::runtime::RuntimeWorker::new(Box::new(backend), journal, image).and_then(|worker| {
            crate::runtime::windows::session::WindowsSession::start(
                worker.retain_journal(custody),
                utc,
            )
        });
    match result {
        Ok(session) => Ok(NativeSession {
            session,
            installation,
            cancel_supported,
            root: root.to_owned(),
            _diagnostic_root: diagnostic_root,
        }),
        Err(error) => Err(retain_start_failure(
            root,
            failure.stage(),
            error,
            failure.stderr(),
            failure.operation(),
        )),
    }
}
#[cfg(all(windows, target_arch = "x86_64"))]
struct NativeSession {
    session: crate::runtime::windows::session::WindowsSession,
    installation: bridge_runtime_protocol::Uuid,
    cancel_supported: bool,
    root: PathBuf,
    _diagnostic_root: fs::File,
}
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct NativeSession(
    crate::runtime::apple_session::AppleVmSession,
    bridge_runtime_protocol::Uuid,
    bool,
);
#[cfg(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
))]
impl LocalSession for NativeSession {
    fn run_cancel_supported(&self) -> bool {
        #[cfg(windows)]
        {
            self.cancel_supported
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.2
        }
    }
    fn with_client(
        &mut self,
        work: &mut dyn FnMut(
            bridge_runtime_protocol::Uuid,
            &mut VmClient,
        ) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        #[cfg(windows)]
        {
            work(self.installation, self.session.client_mut()?)
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            self.0
                .transport()
                .ok_or(RuntimeError::Unavailable)?
                .borrow_client(|client| work(self.1, client))
                .map_err(|_| RuntimeError::Unavailable)?
        }
    }
    fn execute(&mut self, request: EngineRequest) -> TransportFuture<'_> {
        #[cfg(windows)]
        {
            Box::pin(async move {
                let client = self
                    .session
                    .client_mut()
                    .map_err(|_| EngineError::new(ErrorKind::Unavailable))?;
                let id = client
                    .execute(request.action, None)
                    .map_err(|_| EngineError::new(ErrorKind::Execution))?;
                let result = client
                    .result(id)
                    .map_err(|_| EngineError::new(ErrorKind::Execution))?;
                Ok(EngineOutput {
                    exit: result.exit,
                    stdout: result.stdout,
                    stderr: if result.has_diagnostics {
                        vec![1]
                    } else {
                        Vec::new()
                    },
                })
            })
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        Box::pin(async move {
            self.0
                .transport()
                .ok_or_else(|| EngineError::new(ErrorKind::Unavailable))?
                .execute(request)
                .await
        })
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        #[cfg(windows)]
        {
            self.session.shutdown().map_err(|error| {
                let stderr = self
                    .session
                    .diagnostics()
                    .map(|diagnostics| diagnostics.stderr)
                    .map_err(|error| std::io::Error::other(error.to_string()));
                retain_start_failure(
                    &self.root,
                    "Shut down guest",
                    error,
                    stderr,
                    WorkspaceOperation::Stop,
                )
            })
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.0.stop()
    }
}
#[cfg(test)]
#[path = "local_vm_native_tests.rs"]
mod tests;
