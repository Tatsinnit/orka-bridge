use std::io;
/// Cooperating app-owned spawns must not overlap a Win32 inheritable-handle window.
pub fn spawn_guard() -> io::Result<tokio::sync::MutexGuard<'static, ()>> {
    Ok(crate::runtime::spawn_guard::blocking_guard())
}
/// Release only after observed exit; otherwise quarantine all still-owned capabilities.
pub fn release_bundle<T>(bundle: T, observed_stop: bool) {
    if !observed_stop {
        std::mem::forget(bundle);
    }
}
pub fn owner_lock_error(e: io::Error) -> crate::runtime::types::RuntimeError {
    if matches!(e.raw_os_error(), Some(32 | 33)) {
        crate::runtime::types::RuntimeError::Busy
    } else {
        e.into()
    }
}

/// The two attribute lists are inseparable: only these handles, already in this Job.
pub struct SpawnPolicy {
    job: [usize; 1],
    handles: Vec<usize>,
}
impl SpawnPolicy {
    pub fn new(job: usize, handles: Vec<usize>) -> io::Result<Self> {
        let unique: std::collections::BTreeSet<_> = handles.iter().copied().collect();
        if job == 0
            || handles.len() != 4
            || unique.len() != 4
            || unique.contains(&0)
            || unique.contains(&job)
        {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        Ok(Self {
            job: [job],
            handles,
        })
    }
    pub fn jobs(&self) -> &[usize] {
        &self.job
    }
    pub fn handles(&self) -> &[usize] {
        &self.handles
    }
    pub fn flags(&self) -> u32 {
        0x08080404
    } // CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED
}
pub fn create_verified<T>(
    policy: &SpawnPolicy,
    create: impl FnOnce(&SpawnPolicy) -> io::Result<T>,
    member: impl FnOnce(&T) -> io::Result<bool>,
    reap: impl FnOnce(&T) -> io::Result<()>,
) -> io::Result<T> {
    let child = create(policy)?;
    let failure = match member(&child) {
        Ok(true) => return Ok(child),
        Ok(false) => io::Error::new(
            io::ErrorKind::PermissionDenied,
            "child was not atomically assigned to its Job",
        ),
        Err(e) => e,
    };
    if let Err(cleanup) = reap(&child) {
        // Quarantine capabilities when observed stop failed, just like RuntimeWorker.
        std::mem::forget(child);
        return Err(io::Error::other(crate::runtime::types::UnconfirmedStop(
            format!("{failure}; owned child stop unconfirmed: {cleanup}"),
        )));
    }
    Err(failure)
}

#[cfg(any(windows, test))]
fn stop_budget(deadline: &std::sync::OnceLock<std::time::Instant>) -> std::time::Duration {
    let until =
        deadline.get_or_init(|| std::time::Instant::now() + std::time::Duration::from_secs(45));
    until.saturating_duration_since(std::time::Instant::now())
}
#[cfg(test)]
mod coordinator_tests {
    #[test]
    fn later_cleanup_does_not_restart_an_expired_stop_budget() {
        let deadline = std::sync::OnceLock::new();
        deadline
            .set(std::time::Instant::now() - std::time::Duration::from_secs(1))
            .unwrap();
        assert!(super::stop_budget(&deadline).is_zero());
        assert!(super::stop_budget(&deadline).is_zero());
    }
    #[test]
    fn inheritance_window_and_async_spawns_share_one_gate() {
        let guard = super::spawn_guard().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        assert!(
            runtime
                .block_on(async {
                    tokio::time::timeout(
                        std::time::Duration::from_millis(30),
                        crate::runtime::spawn_guard::run(|| ()),
                    )
                    .await
                })
                .is_err(),
            "async process creation must wait for the inheritance window"
        );
        drop(guard);
        assert!(
            runtime
                .block_on(async {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        crate::runtime::spawn_guard::run(|| ()),
                    )
                    .await
                })
                .is_ok(),
            "releasing the window must permit process creation"
        );
    }
}

#[cfg(windows)]
pub use native::OwnedQemuProcess;
#[cfg(windows)]
mod native {
    use super::super::{inputs::VerifiedWindowsInputs, launch::quote_windows_arg, security::*};
    use super::*;
    use std::{
        ffi::OsString,
        fs::File,
        mem::{size_of, zeroed},
        os::windows::io::AsRawHandle,
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::SECURITY_ATTRIBUTES,
        System::{
            JobObjects::*, Pipes::CreatePipe, SystemInformation::GetWindowsDirectoryW, Threading::*,
        },
    };

    pub struct OwnedQemuProcess {
        resources: Option<Resources>,
    }
    struct Resources {
        process: Arc<Handle>,
        thread: Handle,
        job: Handle,
        stop_observed: AtomicBool,
        stop_deadline: std::sync::OnceLock<std::time::Instant>,
        members: std::sync::Mutex<MemberRegistry>,
        stdio: RetainedStdio,
        _ownership: Arc<File>,
        _inputs: Arc<VerifiedWindowsInputs>,
        _boot: Vec<File>,
        _cwd: File,
        control: super::super::control::ControlEndpoint,
        control_started: AtomicBool,
    }
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct ProcessIdentity {
        pid: u32,
        created: u64,
    }
    struct MemberObservation {
        handle: Handle,
        confirmed: Option<ProcessIdentity>,
    }
    #[derive(Default)]
    struct MemberRegistry {
        entries: Vec<MemberObservation>,
        prepared_generation: Option<u32>,
    }
    struct RetainedStdio {
        files: (File, File, File),
        taken: bool,
    }
    impl RetainedStdio {
        fn new(files: (File, File, File)) -> Self {
            Self {
                files,
                taken: false,
            }
        }
        fn take(&mut self) -> io::Result<(File, File, File)> {
            if self.taken {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let _guard = spawn_guard()?;
            let copies = (
                self.files.0.try_clone()?,
                self.files.1.try_clone()?,
                self.files.2.try_clone()?,
            );
            for copy in [&copies.0, &copies.1, &copies.2] {
                unsafe {
                    check(SetHandleInformation(
                        copy.as_raw_handle(),
                        HANDLE_FLAG_INHERIT,
                        0,
                    ))?;
                }
            }
            self.taken = true;
            Ok(copies)
        }
    }
    struct Attributes {
        ptr: LPPROC_THREAD_ATTRIBUTE_LIST,
        _storage: Vec<usize>,
    }
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe { DeleteProcThreadAttributeList(self.ptr) }
        }
    }
    impl Attributes {
        unsafe fn new(policy: &SpawnPolicy) -> io::Result<Self> {
            let mut size = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut size);
            if size == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
            let ptr = storage.as_mut_ptr().cast();
            check(InitializeProcThreadAttributeList(ptr, 2, 0, &mut size))?;
            let result = Self {
                ptr,
                _storage: storage,
            };
            check(UpdateProcThreadAttribute(
                ptr,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                policy.jobs().as_ptr().cast(),
                size_of::<HANDLE>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ))?;
            check(UpdateProcThreadAttribute(
                ptr,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                policy.handles().as_ptr().cast(),
                4 * size_of::<HANDLE>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ))?;
            Ok(result)
        }
    }
    unsafe fn anonymous(input: bool) -> io::Result<(File, Handle)> {
        let mut read = std::ptr::null_mut();
        let mut write = std::ptr::null_mut();
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        check(CreatePipe(&mut read, &mut write, &sa, 0))?;
        let (parent, child) = if input {
            (Handle(write), Handle(read))
        } else {
            (Handle(read), Handle(write))
        };
        check(SetHandleInformation(parent.0, HANDLE_FLAG_INHERIT, 0))?;
        Ok((parent.into_file(), child))
    }
    impl OwnedQemuProcess {
        fn resources(&self) -> &Resources {
            self.resources.as_ref().expect("live process capability")
        }
        /// Internal capability API. argv comes only from the fixed host launch builders.
        pub fn spawn_suspended(
            inputs: Arc<VerifiedWindowsInputs>,
            ownership: Arc<File>,
            cwd: &Path,
            argv: &[OsString],
            control: super::super::control::ControlEndpoint,
        ) -> io::Result<Self> {
            Self::spawn_executable(
                inputs,
                ownership,
                cwd,
                argv,
                control,
                "qemu-system-x86_64.exe",
                Vec::new(),
            )
        }
        pub(crate) fn spawn_image_tool(
            inputs: Arc<VerifiedWindowsInputs>,
            ownership: Arc<File>,
            cwd: &Path,
            argv: &[OsString],
            control: super::super::control::ControlEndpoint,
        ) -> io::Result<Self> {
            Self::spawn_executable(
                inputs,
                ownership,
                cwd,
                argv,
                control,
                "qemu-img.exe",
                Vec::new(),
            )
        }
        pub(crate) fn spawn_with_boot(
            inputs: Arc<VerifiedWindowsInputs>,
            ownership: Arc<File>,
            cwd: &Path,
            argv: &[OsString],
            control: super::super::control::ControlEndpoint,
            boot: Vec<File>,
        ) -> io::Result<Self> {
            Self::spawn_executable(
                inputs,
                ownership,
                cwd,
                argv,
                control,
                "qemu-system-x86_64.exe",
                boot,
            )
        }
        fn spawn_executable(
            inputs: Arc<VerifiedWindowsInputs>,
            ownership: Arc<File>,
            cwd: &Path,
            argv: &[OsString],
            control: super::super::control::ControlEndpoint,
            name: &str,
            boot: Vec<File>,
        ) -> io::Result<Self> {
            inputs.recheck_runtime().map_err(io::Error::other)?;
            let cwd_file = private_directory(cwd)?;
            let exe = inputs.executable(name).map_err(io::Error::other)?;
            let mut command = quote_windows_arg(
                exe.to_str()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?,
            )
            .map_err(io::Error::other)?;
            for arg in argv {
                command.push(' ');
                command.push_str(
                    &quote_windows_arg(
                        arg.to_str()
                            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?,
                    )
                    .map_err(io::Error::other)?,
                );
            }
            let mut command = wide(&command)?;
            let exe = wide_path(&exe)?;
            let cwd_w = wide_path(cwd)?;
            unsafe {
                let token = restricted_token()?;
                let job = Handle::new(CreateJobObjectW(std::ptr::null(), std::ptr::null()))?;
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                check(SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ))?;
                // Declared first so it is released after every temporary inheritable handle.
                let _spawn_guard = spawn_guard()?;
                let (stdin, child_in) = anonymous(true)?;
                let (stdout, child_out) = anonymous(false)?;
                let (stderr, child_err) = anonymous(false)?;
                let mut guard = std::ptr::null_mut();
                check(DuplicateHandle(
                    GetCurrentProcess(),
                    ownership.as_raw_handle(),
                    GetCurrentProcess(),
                    &mut guard,
                    0,
                    1,
                    DUPLICATE_SAME_ACCESS,
                ))?;
                let guard = Handle::new(guard)?;
                let policy = SpawnPolicy::new(
                    job.0 as usize,
                    vec![
                        child_in.0 as usize,
                        child_out.0 as usize,
                        child_err.0 as usize,
                        guard.0 as usize,
                    ],
                )?;
                let attrs = Attributes::new(&policy)?;
                let mut startup: STARTUPINFOEXW = zeroed();
                startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
                startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = child_in.0;
                startup.StartupInfo.hStdOutput = child_out.0;
                startup.StartupInfo.hStdError = child_err.0;
                startup.lpAttributeList = attrs.ptr;
                let mut windows = [0u16; 32768];
                let n = GetWindowsDirectoryW(windows.as_mut_ptr(), windows.len() as u32) as usize;
                if n == 0 || n >= windows.len() {
                    return Err(io::Error::last_os_error());
                }
                let windows = String::from_utf16(&windows[..n]).map_err(io::Error::other)?;
                let mut env = Vec::new();
                for row in [
                    format!(
                        "PATH={};{windows}\\System32",
                        inputs.paths.runtime.join("bin").display()
                    ),
                    format!("SYSTEMROOT={windows}"),
                    format!("TEMP={}", cwd.display()),
                    format!("TMP={}", cwd.display()),
                ] {
                    env.extend(wide(&row)?)
                }
                env.push(0);
                let mut info: PROCESS_INFORMATION = zeroed();
                // No AssignProcessToJobObject fallback exists: JOB_LIST is part of creation.
                check(CreateProcessAsUserW(
                    token.0,
                    exe.as_ptr(),
                    command.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                    policy.flags(),
                    env.as_ptr().cast(),
                    cwd_w.as_ptr(),
                    &startup.StartupInfo,
                    &mut info,
                ))?;
                let process = Self {
                    resources: Some(Resources {
                        process: Arc::new(Handle::new(info.hProcess)?),
                        thread: Handle::new(info.hThread)?,
                        job,
                        stop_observed: AtomicBool::new(false),
                        stop_deadline: std::sync::OnceLock::new(),
                        members: std::sync::Mutex::new(MemberRegistry::default()),
                        stdio: RetainedStdio::new((stdin, stdout, stderr)),
                        _ownership: ownership,
                        _inputs: inputs,
                        _boot: boot,
                        _cwd: cwd_file,
                        control,
                        control_started: AtomicBool::new(false),
                    }),
                };
                // Creation copied the selected handles. Close the temporary inherited copies
                // before releasing other app spawns; exceptional cleanup must not hold this gate.
                drop((attrs, child_in, child_out, child_err, guard));
                drop(_spawn_guard);
                // Exercise the same postcondition/cleanup gate as the injected tests.
                create_verified(
                    &policy,
                    |_| Ok(process),
                    |p| {
                        let mut inside = 0;
                        check(IsProcessInJob(
                            p.resources().process.0,
                            p.resources().job.0,
                            &mut inside,
                        ))?;
                        Ok(inside != 0)
                    },
                    |p| p.abort_suspended(),
                )
            }
        }
        fn abort_suspended(&self) -> io::Result<()> {
            let resources = self.resources();
            stop_suspended(
                resources.job.0,
                resources.process.0,
                stop_budget(&resources.stop_deadline),
                &resources.stop_observed,
            )
        }
        pub fn resume(&self) -> io::Result<()> {
            unsafe {
                if ResumeThread(self.resources().thread.0) == u32::MAX {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
        }
        pub fn take_stdio(&mut self) -> io::Result<(File, File, File)> {
            self.resources
                .as_mut()
                .expect("live process capability")
                .stdio
                .take()
        }
        pub fn control_path(&self) -> &Path {
            self.resources().control.path()
        }
        pub fn connect_control(&self) -> io::Result<super::super::control::ControlConnection<'_>> {
            self.connect_with(self)
        }
        pub fn guest_stream(self: &Arc<Self>) -> io::Result<super::super::control::GuestStream> {
            self.connect_with(self.clone())
                .map(super::super::control::GuestStream)
        }
        fn connect_with<O: std::borrow::Borrow<Self>>(
            &self,
            owner: O,
        ) -> io::Result<super::super::control::ControlIo<O>> {
            let resources = self.resources();
            if resources.control_started.swap(true, Ordering::AcqRel) {
                let _ = self.abort();
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "AF_UNIX channel already acquired; reconnect is forbidden",
                ));
            }
            resources
                .control
                .connect(owner)
                .map_err(|error| match self.abort() {
                    Ok(()) => error,
                    Err(stop) => io::Error::other(format!(
                        "AF_UNIX setup failed: {error}; QEMU stop unobserved: {stop}"
                    )),
                })
        }
        pub fn process_handle(&self) -> Arc<Handle> {
            self.resources().process.clone()
        }
        pub(crate) fn stopping_deadline(&self) -> Option<std::time::Instant> {
            self.resources().stop_deadline.get().copied()
        }
        pub fn exit_code(&self) -> io::Result<u32> {
            if !self.exited() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let mut code = 0;
            unsafe {
                check(GetExitCodeProcess(self.resources().process.0, &mut code))?;
            }
            Ok(code)
        }
        pub fn exited(&self) -> bool {
            unsafe { WaitForSingleObject(self.resources().process.0, 0) == WAIT_OBJECT_0 }
        }
        pub fn wait_exit(&self, timeout: Duration) -> io::Result<()> {
            wait_handle(self.resources().process.0, timeout)
        }
        /// Capture every live member before a caller sends graceful QMP quit.
        /// This begins, rather than renews, the original stop deadline.
        pub fn prepare_graceful_stop(&self) -> io::Result<()> {
            let resources = self.resources();
            let budget = stop_budget(&resources.stop_deadline);
            let result = (|| {
                let mut members = resources
                    .members
                    .try_lock()
                    .map_err(|_| io::Error::other("member observation registry unavailable"))?;
                let generation = capture_members(
                    resources.job.0,
                    resources.process.0,
                    std::time::Instant::now() + budget,
                    &mut members,
                )?;
                members.prepared_generation = Some(generation);
                Ok(())
            })();
            // Never recurse into abort while holding the observation registry.
            result.map_err(|error| match self.abort() {
                Ok(()) => error,
                Err(stop) => io::Error::other(format!(
                    "graceful preparation failed: {error}; stop unobserved: {stop}"
                )),
            })
        }
        /// Natural quit and complete Job observation share one stop proof/budget.
        pub fn wait_for_stop(&self) -> io::Result<()> {
            let resources = self.resources();
            let root = wait_handle(resources.process.0, stop_budget(&resources.stop_deadline));
            let stopped = self.abort(); // Always request the whole Job, even on root-wait failure.
            root.and(stopped)
        }
        pub(crate) fn stop_deadline(&self) -> std::time::Instant {
            let resources = self.resources();
            let _ = stop_budget(&resources.stop_deadline);
            *resources
                .stop_deadline
                .get()
                .expect("original stop deadline")
        }
        pub fn abort(&self) -> io::Result<()> {
            let resources = self.resources();
            stop_job(
                resources.job.0,
                resources.process.0,
                stop_budget(&resources.stop_deadline),
                &resources.stop_observed,
                &resources.members,
            )
        }
    }
    fn wait_handle(handle: HANDLE, timeout: Duration) -> io::Result<()> {
        unsafe {
            match WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128 - 1) as u32)
            {
                WAIT_OBJECT_0 => Ok(()),
                WAIT_TIMEOUT => Err(io::Error::from(io::ErrorKind::TimedOut)),
                _ => Err(io::Error::last_os_error()),
            }
        }
    }
    fn stop_suspended(
        job: HANDLE,
        leader: HANDLE,
        timeout: Duration,
        observed: &AtomicBool,
    ) -> io::Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        // Before the membership postcondition, only the retained process is authority.
        // It has never resumed, so there cannot be descendants to stop here.
        if wait_handle(leader, Duration::ZERO).is_err() {
            unsafe {
                check(TerminateProcess(leader, 1))?;
            }
        }
        wait_handle(
            leader,
            deadline.saturating_duration_since(std::time::Instant::now()),
        )?;
        let info = accounting(job)?;
        // This fresh, private Job had only this suspended creation attempt. Observe
        // both authorities, including the empty Job when membership was rejected.
        if info.ActiveProcesses != 0 || info.TotalProcesses > 1 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        observed.store(true, Ordering::Release);
        Ok(())
    }
    fn accounting(job: HANDLE) -> io::Result<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        unsafe {
            check(QueryInformationJobObject(
                job,
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                std::ptr::null_mut(),
            ))?;
        }
        Ok(info)
    }
    fn job_process_ids(job: HANDLE, deadline: std::time::Instant) -> io::Result<Vec<u32>> {
        let mut capacity = 16usize;
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            // Derive the flexible-array pointer from the allocation, not from a
            // reference to the one-element array in the FFI struct.
            let offset = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);
            let size = offset + capacity * size_of::<usize>();
            let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
            let bytes =
                u32::try_from(size).map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
            let ok = unsafe {
                QueryInformationJobObject(
                    job,
                    JobObjectBasicProcessIdList,
                    storage.as_mut_ptr().cast(),
                    bytes,
                    std::ptr::null_mut(),
                )
            };
            let error = if ok == 0 {
                Some(io::Error::last_os_error())
            } else {
                None
            };
            let info = unsafe { &*storage.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>() };
            if let Some(error) = error {
                if error.raw_os_error() != Some(ERROR_MORE_DATA as i32) {
                    return Err(error);
                }
                capacity = capacity
                    .checked_mul(2)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::OutOfMemory))?
                    .max(info.NumberOfAssignedProcesses as usize);
                continue;
            }
            if info.NumberOfProcessIdsInList as usize > capacity
                || info.NumberOfProcessIdsInList != info.NumberOfAssignedProcesses
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "incomplete Job PID list: assigned={}, listed={}, capacity={capacity}",
                        info.NumberOfAssignedProcesses, info.NumberOfProcessIdsInList
                    ),
                ));
            }
            let ids = unsafe {
                std::slice::from_raw_parts(
                    storage.as_ptr().cast::<u8>().add(offset).cast::<usize>(),
                    info.NumberOfProcessIdsInList as usize,
                )
            };
            return ids
                .iter()
                .map(|id| {
                    u32::try_from(*id).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
                })
                .collect();
        }
    }
    fn process_identity(handle: HANDLE) -> io::Result<ProcessIdentity> {
        unsafe {
            let pid = GetProcessId(handle);
            if pid == 0 {
                return Err(io::Error::last_os_error());
            }
            let (mut created, mut exit, mut kernel, mut user) =
                (zeroed(), zeroed(), zeroed(), zeroed());
            check(GetProcessTimes(
                handle,
                &mut created,
                &mut exit,
                &mut kernel,
                &mut user,
            ))?;
            Ok(ProcessIdentity {
                pid,
                created: (u64::from(created.dwHighDateTime) << 32)
                    | u64::from(created.dwLowDateTime),
            })
        }
    }
    fn capture_members(
        job: HANDLE,
        leader: HANDLE,
        deadline: std::time::Instant,
        members: &mut MemberRegistry,
    ) -> io::Result<u32> {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if members
            .entries
            .iter()
            .any(|entry| entry.confirmed.is_none())
        {
            return Err(io::Error::other(
                "an unconfirmed member capability remains quarantined",
            ));
        }
        let before = accounting(job)?;
        if members
            .prepared_generation
            .is_some_and(|generation| before.TotalProcesses < generation)
        {
            return Err(io::Error::other(
                "Job generation regressed after graceful preparation",
            ));
        }
        let original = process_identity(leader)?;
        for pid in job_process_ids(job, deadline)? {
            if std::time::Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            let handle = unsafe {
                Handle::new(OpenProcess(
                    0x00100000 | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    pid,
                ))?
            };
            // Register the capability before any further fallible query. Pending
            // observations survive errors but cannot contribute to stop proof.
            let index = members.entries.len();
            members.entries.push(MemberObservation {
                handle,
                confirmed: None,
            });
            let raw = members.entries[index].handle.0;
            let identity = process_identity(raw)?;
            if identity == original
                || members.entries[..index]
                    .iter()
                    .any(|entry| entry.confirmed == Some(identity))
            {
                // An equivalent confirmed capability is already retained, including
                // its historical membership after that same process has exited.
                members.entries.pop();
                continue;
            }
            let mut inside = 0;
            unsafe {
                check(IsProcessInJob(raw, job, &mut inside))?;
            }
            if inside == 0 {
                return Err(io::Error::other(format!(
                    "process {pid} is not a confirmed Job member"
                )));
            }
            members.entries[index].confirmed = Some(identity);
        }
        let mut known = std::collections::BTreeSet::from([original]);
        for member in &members.entries {
            known.insert(
                member
                    .confirmed
                    .ok_or_else(|| io::Error::other("unconfirmed member"))?,
            );
        }
        if known.len() != before.TotalProcesses as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unobserved Job process history: total={}, active={}, retained={}",
                    before.TotalProcesses,
                    before.ActiveProcesses,
                    known.len()
                ),
            ));
        }
        let after = accounting(job)?;
        if after.TotalProcesses != before.TotalProcesses {
            return Err(io::Error::other(format!(
                "Job generation changed during capture: {} -> {}",
                before.TotalProcesses, after.TotalProcesses
            )));
        }
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(before.TotalProcesses)
    }
    fn stop_job(
        job: HANDLE,
        leader: HANDLE,
        timeout: Duration,
        observed: &AtomicBool,
        members: &std::sync::Mutex<MemberRegistry>,
    ) -> io::Result<()> {
        // This private Job is assigned once and no external owner can add members.
        if observed.load(Ordering::Acquire) {
            return Ok(());
        }
        let deadline = std::time::Instant::now() + timeout;
        let mut registry = members
            .try_lock()
            .map_err(|_| io::Error::other("member observation registry unavailable"));
        let snapshot = match &mut registry {
            Ok(members) => capture_members(job, leader, deadline, members),
            Err(error) => Err(io::Error::other(error.to_string())),
        };
        // Failure to query/lock/capture must NEVER skip whole-Job termination.
        unsafe {
            check(TerminateJobObject(job, 1))?;
        }
        let generation = snapshot?;
        let members = registry?;
        wait_handle(
            leader,
            deadline.saturating_duration_since(std::time::Instant::now()),
        )?;
        for member in &members.entries {
            wait_handle(
                member.handle.0,
                deadline.saturating_duration_since(std::time::Instant::now()),
            )?;
        }
        loop {
            let info = accounting(job)?;
            if info.TotalProcesses != generation {
                return Err(io::Error::other(format!(
                    "Job process generation changed: expected={generation}, observed={}",
                    info.TotalProcesses
                )));
            }
            if info.ActiveProcesses == 0 {
                observed.store(true, Ordering::Release);
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }
    impl Drop for OwnedQemuProcess {
        fn drop(&mut self) {
            let observed_stop = self.abort().is_ok();
            if let Some(resources) = self.resources.take() {
                release_bundle(resources, observed_stop);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{
            fs::OpenOptions,
            io::Write,
            os::windows::{fs::OpenOptionsExt, process::CommandExt},
            process::{Child, Command, Stdio},
        };

        #[test]
        fn stdio_duplicates_are_noninheritable_and_originals_survive_quarantine() {
            let root = tempfile::tempdir().unwrap();
            let make = |name| {
                OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .share_mode(0)
                    .open(root.path().join(name))
                    .unwrap()
            };
            let mut stdio = RetainedStdio::new((make("in"), make("out"), make("err")));
            let copies = stdio.take().unwrap();
            for file in [&copies.0, &copies.1, &copies.2] {
                let mut flags = HANDLE_FLAG_INHERIT;
                unsafe {
                    check(GetHandleInformation(file.as_raw_handle(), &mut flags)).unwrap();
                }
                assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
            }
            assert_eq!(stdio.take().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
            drop(copies);
            release_bundle(stdio, false);
            assert_eq!(
                OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(root.path().join("in"))
                    .unwrap_err()
                    .raw_os_error(),
                Some(32),
                "the quarantined bundle still owns the original object"
            );
        }
        fn kill_on_close_job() -> Handle {
            unsafe {
                let job =
                    Handle::new(CreateJobObjectW(std::ptr::null(), std::ptr::null())).unwrap();
                let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                check(SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ))
                .unwrap();
                job
            }
        }
        fn active(job: &Handle) -> u32 {
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = zeroed();
                check(QueryInformationJobObject(
                    job.0,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                ))
                .unwrap();
                info.ActiveProcesses
            }
        }
        fn cmd(args: &str) -> Child {
            // Cooperate with app spawns so no product handle is inherited by this tree.
            let _guard = spawn_guard().unwrap();
            Command::new("cmd.exe")
                .raw_arg(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .unwrap()
        }
        fn raw(child: &Child) -> HANDLE {
            child.as_raw_handle() as HANDLE
        }

        fn stop_fresh(
            job: HANDLE,
            leader: HANDLE,
            timeout: Duration,
            observed: &AtomicBool,
        ) -> io::Result<()> {
            stop_job(job, leader, timeout, observed, &std::sync::Mutex::default())
        }
        #[test]
        fn retained_members_prove_stop_after_all_members_exit_naturally() {
            let job = kill_on_close_job();
            let port = completion_port(&job);
            let mut leader = blocked_member(&job);
            let mut member = blocked_member(&job);
            let retained = std::sync::Mutex::new(MemberRegistry::default());
            capture_members(
                job.0,
                raw(&leader),
                std::time::Instant::now() + Duration::from_secs(5),
                &mut retained.lock().unwrap(),
            )
            .unwrap();
            assert_eq!(retained.lock().unwrap().entries.len(), 1);
            leader.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            member.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            assert!(leader.wait().unwrap().success());
            assert!(member.wait().unwrap().success());
            // Process exit handles can signal before Job accounting drains.
            // This is the empty-Job success case; incomplete PID accounting
            // during rundown must still fail closed and retain ownership.
            observe_empty_job(&job, &port);
            let observed = AtomicBool::new(false);
            stop_job(
                job.0,
                raw(&leader),
                Duration::from_secs(45),
                &observed,
                &retained,
            )
            .expect("retained member exit handles plus the unchanged empty Job prove stop");
            assert_eq!(active(&job), 0);
            assert!(observed.load(Ordering::Acquire));
        }

        #[test]
        fn owned_member_fixture() {
            if std::env::var_os("BRIDGE_TEST_OWNED_MEMBER").is_some() {
                use std::io::Read;
                println!("OWNED-MEMBER-READY");
                std::io::stdout().flush().unwrap();
                std::io::stdin().read_exact(&mut [0; 2]).unwrap();
                std::process::exit(0);
            }
        }
        fn blocked_member(job: &Handle) -> Child {
            use std::io::{BufRead, BufReader};
            let gate = spawn_guard().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "runtime::windows::job::native::tests::owned_member_fixture",
                    "--nocapture",
                ])
                .env("BRIDGE_TEST_OWNED_MEMBER", "1")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .unwrap();
            drop(gate);
            // Do not use a command shell whose console helpers can join the Job.
            let output = child.stdout.take().unwrap();
            assert!(BufReader::new(output)
                .lines()
                .take(8)
                .any(|line| line.unwrap().contains("OWNED-MEMBER-READY")));
            unsafe {
                check(AssignProcessToJobObject(job.0, raw(&child))).unwrap();
            }
            child
        }
        fn completion_port(job: &Handle) -> Handle {
            use windows_sys::Win32::System::IO::CreateIoCompletionPort;
            unsafe {
                let port = Handle::new(CreateIoCompletionPort(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null_mut(),
                    0,
                    1,
                ))
                .unwrap();
                let info = JOBOBJECT_ASSOCIATE_COMPLETION_PORT {
                    CompletionKey: job.0,
                    CompletionPort: port.0,
                };
                check(SetInformationJobObject(
                    job.0,
                    JobObjectAssociateCompletionPortInformation,
                    (&info as *const JOBOBJECT_ASSOCIATE_COMPLETION_PORT).cast(),
                    size_of::<JOBOBJECT_ASSOCIATE_COMPLETION_PORT>() as u32,
                ))
                .unwrap();
                port
            }
        }
        fn observe_empty_job(job: &Handle, port: &Handle) {
            observe_empty_job_until(
                job,
                port,
                std::time::Instant::now() + Duration::from_secs(5),
            );
        }
        fn observe_empty_job_until(job: &Handle, port: &Handle, until: std::time::Instant) {
            use windows_sys::Win32::System::{
                SystemServices::JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO, IO::GetQueuedCompletionStatus,
            };
            loop {
                let (mut message, mut key, mut process) = (0, 0, std::ptr::null_mut());
                let remaining = until.saturating_duration_since(std::time::Instant::now());
                assert!(
                    !remaining.is_zero(),
                    "Job zero-process notification missing"
                );
                unsafe {
                    check(GetQueuedCompletionStatus(
                        port.0,
                        &mut message,
                        &mut key,
                        &mut process,
                        remaining.as_millis().min(u32::MAX as u128) as u32,
                    ))
                    .unwrap();
                }
                assert_eq!(key, job.0 as usize);
                if message == JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO {
                    assert_eq!(active(job), 0);
                    return;
                }
            }
        }
        #[test]
        fn contended_member_registry_still_terminates_without_observed_stop() {
            let job = kill_on_close_job();
            let mut child = blocked_member(&job);
            let registry = std::sync::Mutex::new(MemberRegistry::default());
            let held = registry.lock().unwrap();
            let observed = AtomicBool::new(false);
            assert!(stop_job(
                job.0,
                raw(&child),
                Duration::from_secs(5),
                &observed,
                &registry
            )
            .is_err());
            wait_handle(raw(&child), Duration::from_secs(45)).unwrap();
            child.wait().unwrap();
            assert!(!observed.load(Ordering::Acquire));
            drop(held);
        }
        #[test]
        fn poisoned_member_registry_still_terminates_without_observed_stop() {
            let job = kill_on_close_job();
            let mut child = blocked_member(&job);
            let registry = std::sync::Mutex::new(MemberRegistry::default());
            let _ = std::panic::catch_unwind(|| {
                let _held = registry.lock().unwrap();
                panic!("injected registry poison");
            });
            let observed = AtomicBool::new(false);
            assert!(stop_job(
                job.0,
                raw(&child),
                Duration::from_secs(5),
                &observed,
                &registry
            )
            .is_err());
            wait_handle(raw(&child), Duration::from_secs(45)).unwrap();
            child.wait().unwrap();
            assert!(!observed.load(Ordering::Acquire));
        }
        #[test]
        fn a_later_unobserved_exit_cannot_join_a_prepared_generation() {
            let job = kill_on_close_job();
            let mut leader = blocked_member(&job);
            let port = completion_port(&job);
            let registry = std::sync::Mutex::new(MemberRegistry::default());
            {
                let mut members = registry.lock().unwrap();
                let generation = capture_members(
                    job.0,
                    raw(&leader),
                    std::time::Instant::now() + Duration::from_secs(5),
                    &mut members,
                )
                .unwrap();
                members.prepared_generation = Some(generation);
            }
            let mut later = blocked_member(&job);
            later.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            assert!(later.wait().unwrap().success());
            leader.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            assert!(leader.wait().unwrap().success());
            // Construct a genuinely missing historical member, not the proxy
            // that a signaled process has already left the Job PID list.
            observe_empty_job(&job, &port);
            let observed = AtomicBool::new(false);
            assert!(stop_job(
                job.0,
                raw(&leader),
                Duration::from_secs(5),
                &observed,
                &registry
            )
            .is_err());
            wait_handle(raw(&leader), Duration::from_secs(45)).unwrap();
            leader.wait().unwrap();
            assert!(!observed.load(Ordering::Acquire));
        }
        #[test]
        fn pending_unqueryable_member_keeps_the_entire_bundle_quarantined() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("guard");
            let guard = OpenOptions::new()
                .create_new(true)
                .write(true)
                .share_mode(0)
                .open(&path)
                .unwrap();
            let job = kill_on_close_job();
            let mut child = blocked_member(&job);
            let mut raw_handle = std::ptr::null_mut();
            unsafe {
                check(DuplicateHandle(
                    GetCurrentProcess(),
                    raw(&child),
                    GetCurrentProcess(),
                    &mut raw_handle,
                    0x00100000,
                    0,
                    0,
                ))
                .unwrap();
            }
            let handle = Handle::new(raw_handle).unwrap();
            let mut members = MemberRegistry::default();
            members.entries.push(MemberObservation {
                handle,
                confirmed: None,
            });
            assert!(process_identity(raw_handle).is_err());
            let registry = std::sync::Mutex::new(members);
            let observed = AtomicBool::new(false);
            assert!(stop_job(
                job.0,
                raw(&child),
                Duration::from_secs(5),
                &observed,
                &registry
            )
            .is_err());
            wait_handle(raw(&child), Duration::from_secs(45)).unwrap();
            child.wait().unwrap();
            release_bundle((guard, registry), observed.load(Ordering::Acquire));
            assert_eq!(
                OpenOptions::new()
                    .write(true)
                    .share_mode(0)
                    .open(&path)
                    .unwrap_err()
                    .raw_os_error(),
                Some(32)
            );
        }
        #[test]
        fn repeated_member_preparation_deduplicates_held_identities() {
            let job = kill_on_close_job();
            let mut leader = blocked_member(&job);
            let mut member = blocked_member(&job);
            let port = completion_port(&job);
            let registry = std::sync::Mutex::new(MemberRegistry::default());
            {
                let mut members = registry.lock().unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let generation =
                    capture_members(job.0, raw(&leader), deadline, &mut members).unwrap();
                members.prepared_generation = Some(generation);
                capture_members(job.0, raw(&leader), deadline, &mut members).unwrap();
                assert_eq!(members.entries.len(), 1);
                let mut flags = HANDLE_FLAG_INHERIT;
                unsafe {
                    check(GetHandleInformation(
                        members.entries[0].handle.0,
                        &mut flags,
                    ))
                    .unwrap();
                }
                assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
            }
            let until = std::time::Instant::now() + Duration::from_secs(5);
            leader.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            member.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            leader.wait().unwrap();
            member.wait().unwrap();
            observe_empty_job_until(&job, &port, until);
            stop_job(
                job.0,
                raw(&leader),
                until.saturating_duration_since(std::time::Instant::now()),
                &AtomicBool::new(false),
                &registry,
            )
            .unwrap();
        }

        struct SuspendedChild(Child);
        impl Drop for SuspendedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                if wait_handle(raw(&self.0), Duration::from_secs(45)).is_err() {
                    std::process::abort();
                }
            }
        }
        fn suspended_cmd() -> SuspendedChild {
            let _guard = spawn_guard().unwrap();
            SuspendedChild(
                Command::new("cmd.exe")
                    .raw_arg("/d /c exit 0")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
                    .spawn()
                    .unwrap(),
            )
        }
        #[test]
        fn unqueryable_job_does_not_record_suspended_stop() {
            let job = kill_on_close_job();
            let child = suspended_cmd();
            let mut query_denied = std::ptr::null_mut();
            unsafe {
                check(DuplicateHandle(
                    GetCurrentProcess(),
                    job.0,
                    GetCurrentProcess(),
                    &mut query_denied,
                    8,
                    0,
                    0,
                ))
                .unwrap(); // JOB_OBJECT_TERMINATE, not QUERY
            }
            let query_denied = Handle::new(query_denied).unwrap();
            let observed = AtomicBool::new(false);
            let error = stop_suspended(
                query_denied.0,
                raw(&child.0),
                Duration::from_secs(45),
                &observed,
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(5));
            assert!(wait_handle(raw(&child.0), Duration::ZERO).is_ok());
            assert!(
                !observed.load(Ordering::Acquire),
                "unobserved Job state must quarantine"
            );
        }
        #[test]
        fn stopped_suspended_nonmember_releases_the_ownership_capability() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("guard");
            let guard = OpenOptions::new()
                .create_new(true)
                .write(true)
                .share_mode(0)
                .open(&path)
                .unwrap();
            let job = kill_on_close_job();
            let child = suspended_cmd();
            let mut member = 1;
            unsafe {
                check(IsProcessInJob(raw(&child.0), job.0, &mut member)).unwrap();
            }
            assert_eq!(
                member, 0,
                "model a rejected membership postcondition before resume"
            );
            let observed = AtomicBool::new(false);
            stop_suspended(job.0, raw(&child.0), Duration::from_secs(45), &observed).unwrap();
            assert!(wait_handle(raw(&child.0), Duration::ZERO).is_ok());
            // Same recheck and whole-bundle release that OwnedQemuProcess::drop uses.
            release_bundle(
                guard,
                stop_fresh(job.0, raw(&child.0), Duration::from_secs(45), &observed).is_ok(),
            );
            drop(
                OpenOptions::new()
                    .write(true)
                    .share_mode(0)
                    .open(&path)
                    .expect("a stopped pre-resume nonmember must not quarantine ownership"),
            );
            assert!(observed.load(Ordering::Acquire));
        }

        #[test]
        fn leader_exit_is_not_job_stop_while_a_descendant_holds_a_capability() {
            let dir = tempfile::tempdir().unwrap();
            let held = dir.path().join("held.txt");
            let exclusive = || OpenOptions::new().read(true).share_mode(0).open(&held);
            let job = kill_on_close_job();
            // The leader blocks on stdin until it is in the Job, then leaves a descendant that
            // keeps the redirected file open after the leader itself exits.
            let mut leader = cmd(&format!(
                "/d /c set /p x= & start /b ping -n 120 127.0.0.1 > \"{}\"",
                held.display()
            ));
            unsafe { check(AssignProcessToJobObject(job.0, raw(&leader))).unwrap() };
            leader.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            leader.wait().unwrap();
            assert!(active(&job) >= 1, "descendant stays in the Job");
            assert_eq!(exclusive().unwrap_err().raw_os_error(), Some(32));

            let observed = AtomicBool::new(false);
            stop_fresh(job.0, raw(&leader), Duration::from_secs(45), &observed).unwrap();
            assert_eq!(active(&job), 0, "observed stop is Job-wide");
            drop(exclusive().expect("no descendant still holds the file"));
            // A later teardown of the same empty Job reuses its proved stop.
            stop_fresh(job.0, raw(&leader), Duration::from_secs(45), &observed).unwrap();
        }

        #[test]
        fn unobserved_historical_descendant_exit_cannot_release_the_bundle() {
            let job = kill_on_close_job();
            let mut leader = cmd("/d /c set /p x= & start /b ping -n 1 127.0.0.1 > nul");
            unsafe { check(AssignProcessToJobObject(job.0, raw(&leader))).unwrap() };
            leader.stdin.take().unwrap().write_all(b"\r\n").unwrap();
            leader.wait().unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while active(&job) != 0 {
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(accounting(job.0).unwrap().TotalProcesses > 1);
            let observed = AtomicBool::new(false);
            assert!(stop_fresh(job.0, raw(&leader), Duration::from_secs(45), &observed).is_err());
            assert!(!observed.load(Ordering::Acquire));
        }

        #[test]
        fn job_stop_without_terminate_authority_is_unobserved_after_leader_exit() {
            let job = kill_on_close_job();
            let mut leader = cmd("/d /c exit 0");
            leader.wait().unwrap();
            let mut query_only = std::ptr::null_mut();
            unsafe {
                check(DuplicateHandle(
                    GetCurrentProcess(),
                    job.0,
                    GetCurrentProcess(),
                    &mut query_only,
                    4, // JOB_OBJECT_QUERY
                    0,
                    0,
                ))
                .unwrap();
            }
            let query_only = Handle::new(query_only).unwrap();
            assert!(stop_fresh(
                query_only.0,
                raw(&leader),
                Duration::from_secs(45),
                &AtomicBool::new(false)
            )
            .is_err());
        }
    }
}
