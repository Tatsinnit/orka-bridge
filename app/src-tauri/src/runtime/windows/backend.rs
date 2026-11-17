#[cfg(any(windows, test))]
use std::{
    fs::File,
    io::{self, Seek},
};
#[cfg(any(windows, test))]
fn fill_state(scratch: &mut File, state: &mut File) -> io::Result<()> {
    if state.metadata()?.len() != 0 {
        return Err(io::Error::other("existing state is retained"));
    }
    scratch.rewind()?;
    state.rewind()?;
    io::copy(scratch, state)?;
    state.sync_all()
}
#[cfg(any(windows, test))]
fn finish_startup<T>(
    result: Result<T, crate::runtime::types::RuntimeError>,
    stop: impl FnOnce() -> io::Result<()>,
) -> Result<T, crate::runtime::types::RuntimeError> {
    result.map_err(|error| match stop() {
        Ok(()) => error,
        Err(cleanup) => crate::runtime::types::RuntimeError::StopUnconfirmed(format!(
            "{error}; stopping the native child failed: {cleanup}"
        )),
    })
}
#[cfg(windows)]
pub struct Diagnostics {
    pub rx: Vec<u8>,
    pub tx: Vec<u8>,
    pub stderr: Vec<u8>,
}
#[cfg(windows)]
mod native {
    use super::*;
    use crate::runtime::{
        types::*,
        windows::{
            control::{ControlEndpoint, GuestStream},
            inputs::VerifiedWindowsInputs,
            job::OwnedQemuProcess,
            launch::{launch_argv, validate_qcow2_info},
            qmp::{Qmp, Transport},
            qmp_io::NativeTransport,
        },
    };
    use std::{
        ffi::OsString,
        fs::{self, OpenOptions},
        path::Path,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    #[derive(Clone, Default)]
    pub struct FailureDiagnostics {
        capture: Arc<Mutex<Option<crate::runtime::windows::qmp_io::StderrCapture>>>,
        stage: Arc<Mutex<&'static str>>,
        operation: Arc<Mutex<Option<WorkspaceOperation>>>,
    }
    impl FailureDiagnostics {
        pub fn operation(&self) -> WorkspaceOperation {
            self.operation
                .lock()
                .ok()
                .and_then(|operation| *operation)
                .unwrap_or(WorkspaceOperation::Start)
        }
        fn begin(&self, operation: WorkspaceOperation) -> Result<(), RuntimeError> {
            *self.operation.lock().map_err(|_| RuntimeError::Closed)? = Some(operation);
            Ok(())
        }
        pub fn stage(&self) -> &'static str {
            self.stage
                .lock()
                .map(|stage| {
                    if stage.is_empty() {
                        "Prepare local VM"
                    } else {
                        *stage
                    }
                })
                .unwrap_or("Prepare local VM")
        }
        fn enter(&self, stage: &'static str) -> Result<(), RuntimeError> {
            *self.stage.lock().map_err(|_| RuntimeError::Closed)? = stage;
            Ok(())
        }
        pub fn stderr(&self) -> std::io::Result<Vec<u8>> {
            self.capture
                .lock()
                .map_err(|_| std::io::Error::other("diagnostics poisoned"))?
                .as_ref()
                .map_or(Ok(Vec::new()), |capture| capture.snapshot())
        }
    }
    pub struct WindowsBackend {
        inputs: Arc<VerifiedWindowsInputs>,
        failed: bool,
        diagnostics: FailureDiagnostics,
    }
    #[derive(Clone)]
    pub(crate) struct Lease(pub Arc<Mutex<Vm>>);
    pub(crate) struct Vm {
        pub owner: Arc<OwnedQemuProcess>,
        qmp: Qmp<NativeTransport>,
        guest: Option<GuestStream>,
    }
    pub(super) fn pin_boot(
        image: &crate::runtime::image_install::InstalledImageSet,
        root: &Path,
    ) -> Result<Vec<File>, RuntimeError> {
        use sha2::{Digest, Sha256};
        use std::{
            io::Read,
            os::windows::fs::{MetadataExt, OpenOptionsExt},
        };
        let mut held = Vec::new();
        for (path, identity, (bytes, sha)) in [
            (
                image.kernel_path(),
                image.kernel_identity(),
                image.kernel_pin(),
            ),
            (
                image.initrd_path(),
                image.initrd_identity(),
                image.initrd_pin(),
            ),
        ] {
            let mut file = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .custom_flags(0x00200000)
                .open(root.join(path))?;
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.file_attributes() & 0x400 != 0
                || metadata.len() != bytes
                || crate::runtime::identity::file_identity(&file)? != *identity
            {
                return Err(RuntimeError::RetainedState);
            }
            let mut digest = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
            }
            if format!("{:x}", digest.finalize()) != sha {
                return Err(RuntimeError::RetainedState);
            }
            held.push(file);
        }
        Ok(held)
    }
    impl WindowsBackend {
        pub fn new(inputs: Arc<VerifiedWindowsInputs>) -> Self {
            Self {
                inputs,
                failed: false,
                diagnostics: FailureDiagnostics::default(),
            }
        }
        pub fn failure_diagnostics(&self) -> FailureDiagnostics {
            self.diagnostics.clone()
        }
        fn copy_state(&self, scratch: &mut File, state: &mut File) -> Result<(), RuntimeError> {
            *self
                .diagnostics
                .capture
                .lock()
                .map_err(|_| RuntimeError::Closed)? = None;
            self.diagnostics.enter("Fill private state disk")?;
            fill_state(scratch, state).map_err(Into::into)
        }
        fn tool(&self, spec: &LaunchSpec, args: &[OsString]) -> Result<Vec<u8>, RuntimeError> {
            *self
                .diagnostics
                .capture
                .lock()
                .map_err(|_| RuntimeError::Closed)? = None;
            self.diagnostics.enter("Run qemu-img")?;
            let endpoint = ControlEndpoint::create(&spec.root)?;
            let mut child = OwnedQemuProcess::spawn_image_tool(
                self.inputs.clone(),
                spec.ownership.clone(),
                &spec.root,
                args,
                endpoint,
            )?;
            let stdio = finish_startup(child.take_stdio().map_err(Into::into), || child.abort())?;
            let owner = Arc::new(child);
            let result = (|| {
                let mut io = NativeTransport::new(owner.clone(), stdio)?;
                *self
                    .diagnostics
                    .capture
                    .lock()
                    .map_err(|_| RuntimeError::Closed)? = Some(io.stderr_capture());
                let until = io.prepare_stop()?;
                owner.resume()?;
                let mut output = Vec::new();
                loop {
                    match io.receive(until) {
                        Ok(frame) => {
                            if output.len() + frame.bytes.len() > 128 * 1024 {
                                return Err(
                                    std::io::Error::from(std::io::ErrorKind::InvalidData).into()
                                );
                            }
                            output.extend(frame.bytes);
                            if let Some(error) = frame.failure {
                                if error.kind() == std::io::ErrorKind::UnexpectedEof {
                                    break;
                                }
                                return Err(error.into());
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                        Err(error) => return Err(error.into()),
                    }
                }
                io.wait_for_stop()?;
                if owner.exit_code()? != 0 {
                    return Err(std::io::Error::other("owned qemu-img failed").into());
                }
                Ok(output)
            })();
            finish_startup(result, || owner.abort())
        }
        fn inspect(&self, spec: &LaunchSpec, path: &Path, size: u64) -> Result<(), RuntimeError> {
            let output = self.tool(
                spec,
                &[
                    "info".into(),
                    "-f".into(),
                    "qcow2".into(),
                    "--output=json".into(),
                    path.as_os_str().into(),
                ],
            )?;
            let size = if size == 0 {
                serde_json::from_slice::<serde_json::Value>(&output)
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?
                    .get("virtual-size")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|n| *n > 0)
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?
            } else {
                size
            };
            validate_qcow2_info(&output, size)
        }
        fn start_vm(&self, spec: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
            self.inspect(spec, &spec.root.join(spec.image.disk_path()), 0)?;
            self.inspect(spec, &spec.state_path, spec.state_bytes)?;
            // Image-tool output must not be attributed to a later VM-launch failure.
            *self
                .diagnostics
                .capture
                .lock()
                .map_err(|_| RuntimeError::Closed)? = None;
            self.diagnostics.enter("Prepare QEMU control endpoint")?;
            let endpoint = ControlEndpoint::create(&spec.root)?;
            let args = launch_argv(
                &self.inputs,
                spec,
                endpoint
                    .path()
                    .to_str()
                    .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
            )?;
            self.diagnostics.enter("Pin installed boot files")?;
            let boot = pin_boot(&spec.image, &spec.root)?;
            self.diagnostics.enter("Create QEMU process")?;
            let mut child = OwnedQemuProcess::spawn_with_boot(
                self.inputs.clone(),
                spec.ownership.clone(),
                &spec.root,
                &args,
                endpoint,
                boot,
            )?;
            finish_startup(self.diagnostics.enter("Open QEMU standard I/O"), || {
                child.abort()
            })?;
            let stdio = finish_startup(child.take_stdio().map_err(Into::into), || child.abort())?;
            let owner = Arc::new(child);
            let result = (|| {
                let transport = NativeTransport::new(owner.clone(), stdio)?;
                *self
                    .diagnostics
                    .capture
                    .lock()
                    .map_err(|_| RuntimeError::Closed)? = Some(transport.stderr_capture());
                self.diagnostics.enter("Resume QEMU process")?;
                owner.resume()?;
                self.diagnostics.enter("Negotiate QMP")?;
                let until = Instant::now() + Duration::from_secs(30);
                let mut qmp = Qmp::negotiate(transport, until)?;
                self.diagnostics.enter("Check QEMU prelaunch status")?;
                let status = qmp.query_status(until)?;
                if status.running || status.status != "prelaunch" {
                    return Err(std::io::Error::other("QEMU did not remain prelaunch").into());
                }
                self.diagnostics.enter("Connect guest control")?;
                let guest = owner.guest_stream()?;
                self.diagnostics.enter("Start guest CPUs")?;
                qmp.set_running(true, Instant::now() + Duration::from_secs(30))?;
                self.diagnostics.enter("Establish guest session")?;
                Ok(BackendHandle::new(Lease(Arc::new(Mutex::new(Vm {
                    owner: owner.clone(),
                    qmp,
                    guest: Some(guest),
                })))))
            })();
            finish_startup(result, || owner.abort())
        }
    }
    impl Lease {
        pub(crate) fn prepare_shutdown(&self) -> Result<(), RuntimeError> {
            self.0
                .lock()
                .map_err(|_| RuntimeError::Closed)?
                .qmp
                .prepare_stop()?;
            Ok(())
        }
        pub(crate) fn guest(&self) -> Result<GuestStream, RuntimeError> {
            self.0
                .lock()
                .map_err(|_| RuntimeError::Closed)?
                .guest
                .take()
                .ok_or(RuntimeError::Closed)
        }
        pub(crate) fn diagnostics(&self) -> Result<Diagnostics, RuntimeError> {
            let vm = self.0.lock().map_err(|_| RuntimeError::Closed)?;
            Ok(Diagnostics {
                rx: vm.qmp.transcript().rx.clone(),
                tx: vm.qmp.transcript().tx.clone(),
                stderr: vm.qmp.transport().stderr()?,
            })
        }
        pub(crate) fn owner(&self) -> Result<Arc<OwnedQemuProcess>, RuntimeError> {
            Ok(self
                .0
                .lock()
                .map_err(|_| RuntimeError::Closed)?
                .owner
                .clone())
        }
    }
    fn vm(handle: &BackendHandle) -> Result<std::sync::MutexGuard<'_, Vm>, RuntimeError> {
        handle
            .get::<Lease>()
            .ok_or(RuntimeError::Unavailable)?
            .0
            .lock()
            .map_err(|_| RuntimeError::Closed)
    }
    impl VmBackend for WindowsBackend {
        fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
            self.diagnostics.begin(WorkspaceOperation::Prepare)?;
            if self.failed || spec.backend != BackendKind::Qemu {
                return Err(RuntimeError::Unavailable);
            }
            let mut state = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&spec.state_path)?;
            if state.metadata()?.len() != 0 {
                return Err(RuntimeError::RetainedState);
            }
            let path = spec.root.join(format!(
                "state-{}.qcow2",
                bridge_runtime_protocol::Uuid::new_v4()
            ));
            let mut scratch = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)?;
            let result = (|| {
                self.tool(
                    spec,
                    &[
                        "create".into(),
                        "-f".into(),
                        "qcow2".into(),
                        path.as_os_str().into(),
                        spec.state_bytes.to_string().into(),
                    ],
                )?;
                self.inspect(spec, &path, spec.state_bytes)?;
                self.copy_state(&mut scratch, &mut state)?;
                Ok(())
            })();
            drop(scratch);
            if result.is_ok() {
                fs::remove_file(path)?;
            } else {
                // Preserve partial scratch evidence, including when the exact
                // original owner/process bundle has been quarantined.
                self.failed = true;
            }
            result
        }
        fn start(&mut self, spec: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
            self.diagnostics.begin(WorkspaceOperation::Start)?;
            if self.failed {
                return Err(RuntimeError::Unavailable);
            }
            let result = self.start_vm(spec);
            if result.is_err() {
                self.failed = true;
            }
            result
        }
        fn stop(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
            let mut vm = vm(handle)?;
            if vm.owner.stopping_deadline().is_some() || vm.owner.exited() {
                // A guest ACK schedules poweroff; it is not filesystem flush
                // completion. Prepared guest shutdown must remain wait-only.
                vm.qmp.wait_for_stop()?;
            } else {
                let until = vm
                    .owner
                    .stopping_deadline()
                    .unwrap_or_else(|| Instant::now() + Duration::from_secs(45));
                if vm.qmp.quit(until).is_err() {
                    // A guest may close QMP while completing its acknowledged
                    // poweroff. Only independent held exits and exit0 can prove
                    // stop; a forced abort's nonzero exit never becomes clean.
                    vm.owner.wait_for_stop()?;
                }
            }
            // Stop success is held whole-Job termination proof, not a clean
            // guest exit. The session reports exit status independently.
            Ok(())
        }
        fn pause(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
            vm(handle)?
                .qmp
                .set_running(false, Instant::now() + Duration::from_secs(30))
                .map_err(Into::into)
        }
        fn resume(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
            vm(handle)?
                .qmp
                .set_running(true, Instant::now() + Duration::from_secs(30))
                .map_err(Into::into)
        }
    }
    #[cfg(test)]
    mod diagnostic_tests {
        use super::*;
        use crate::runtime::{
            image_install::materialize_image,
            image_types::ImageTarget,
            journal::Journal,
            windows::{inputs, qmp_io},
        };
        use bridge_runtime_protocol::Uuid;
        #[test]
        fn failed_image_tool_does_not_report_a_previous_invocations_stderr() {
            let temp = tempfile::tempdir_in(std::env::var_os("USERPROFILE").unwrap()).unwrap();
            let root = temp.path().join("install");
            let journal =
                Journal::create(&root, BackendKind::Qemu, b"pc-q35-11.0".to_vec()).unwrap();
            let source = temp.path().join("source");
            fs::create_dir(&source).unwrap();
            let verified =
                super::super::boot_tests::fixture::verified(&source, ImageTarget::WindowsAmd64);
            let target = journal
                .image_target(&verified.image().identity().state_compatibility_id)
                .unwrap();
            let (_sender, closed) = tokio::sync::watch::channel(false);
            let image = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(materialize_image(&verified, &target, closed, |_| {}))
                .unwrap();
            let mut spec = journal.spec(Uuid::new_v4(), Arc::new(image));
            spec.root = temp.path().join("missing-root");
            let runtime = temp.path().join("runtime");
            fs::create_dir_all(runtime.join("bin")).unwrap();
            let mut files = Vec::new();
            for name in ["bin/qemu-system-x86_64.exe", "bin/qemu-img.exe"] {
                fs::write(runtime.join(name), b"abc").unwrap();
                files.push(inputs::FilePin {
                    name: name.into(),
                    bytes: 3,
                    sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                        .into(),
                });
            }
            let backend = WindowsBackend::new(Arc::new(
                inputs::verify_windows_runtime(&files, runtime).unwrap(),
            ));
            *backend.diagnostics.capture.lock().unwrap() = Some(qmp_io::tests::captured_stderr(
                b"previous image-tool warning",
            ));
            assert_eq!(
                backend.diagnostics.stderr().unwrap(),
                b"previous image-tool warning"
            );
            // Endpoint creation fails before spawning anything or creating a new capture.
            assert!(backend.tool(&spec, &[]).is_err());
            assert!(backend.diagnostics.stderr().unwrap().is_empty());
            assert_eq!(backend.diagnostics.stage(), "Run qemu-img");

            let mut scratch = tempfile::tempfile().unwrap();
            use std::io::Write;
            scratch.write_all(b"prepared state").unwrap();
            let mut readonly_state = File::open(&spec.state_path).unwrap();
            *backend.diagnostics.capture.lock().unwrap() = Some(qmp_io::tests::captured_stderr(
                b"successful inspection warning",
            ));
            // Copying into a read-only held file fails without a process or disk-full simulation.
            assert!(backend
                .copy_state(&mut scratch, &mut readonly_state)
                .is_err());
            assert!(backend.diagnostics.stderr().unwrap().is_empty());
            assert_eq!(backend.diagnostics.stage(), "Fill private state disk");
            assert_eq!(readonly_state.metadata().unwrap().len(), 0);
        }
    }
}
#[cfg(windows)]
pub(crate) use native::Lease;
#[cfg(windows)]
pub use native::WindowsBackend;
#[cfg(all(test, windows))]
#[path = "backend_boot_tests.rs"]
pub(crate) mod boot_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[cfg(windows)]
    #[test]
    fn new_journal_root_is_the_protected_parent_required_by_containment() {
        let home = std::env::var_os("USERPROFILE").unwrap();
        let temp = tempfile::tempdir_in(home).unwrap();
        let root = temp.path().join("install");
        let mut journal = crate::runtime::journal::Journal::create(
            &root,
            crate::runtime::types::BackendKind::Qemu,
            b"pc-q35-11.0".to_vec(),
        )
        .unwrap();
        let endpoint = crate::runtime::windows::control::ControlEndpoint::create(&root).unwrap();
        assert!(endpoint.path().parent().unwrap().is_dir());
        journal
            .set_phase(crate::runtime::journal::StatePhase::Preparing)
            .unwrap();
        assert_eq!(
            journal.record().phase,
            crate::runtime::journal::StatePhase::Preparing
        );
        use std::os::windows::fs::OpenOptionsExt;
        let delete = std::fs::OpenOptions::new()
            .access_mode(0x00010000)
            .share_mode(7)
            .custom_flags(0x02200000)
            .open(&root)
            .unwrap_err();
        assert_eq!(delete.raw_os_error(), Some(32));
        let moved = temp.path().join("moved");
        assert!(std::fs::rename(&root, &moved).is_err());
        assert!(root.is_dir() && !moved.exists());
    }
    #[test]
    fn prepared_state_is_filled_without_replacing_its_held_identity() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("state.disk");
        let mut state = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let identity = crate::runtime::identity::file_identity(&state).unwrap();
        let mut scratch = tempfile::tempfile().unwrap();
        scratch.write_all(b"prepared-qcow").unwrap();
        fill_state(&mut scratch, &mut state).unwrap();
        assert_eq!(
            identity,
            crate::runtime::identity::file_identity(&state).unwrap()
        );
        assert_eq!(
            identity,
            crate::runtime::identity::file_identity(&std::fs::File::open(&path).unwrap()).unwrap()
        );
        let mut bytes = Vec::new();
        state.rewind().unwrap();
        state.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"prepared-qcow");
        assert!(fill_state(&mut scratch, &mut state).is_err());
    }
}

#[cfg(test)]
mod startup_cleanup_tests {
    use super::*;
    use crate::runtime::types::RuntimeError;
    #[test]
    fn startup_error_checks_stop_and_propagates_unconfirmed_custody() {
        let error = finish_startup::<()>(Err(RuntimeError::Unavailable), || {
            Err(io::Error::other("still running"))
        })
        .unwrap_err();
        assert!(matches!(error, RuntimeError::StopUnconfirmed(_)));
        assert!(matches!(
            finish_startup::<()>(Err(RuntimeError::Unavailable), || Ok(())),
            Err(RuntimeError::Unavailable)
        ));
        assert_eq!(
            finish_startup(Ok(7), || panic!("a live lease must not stop")).unwrap(),
            7
        );
    }
}
