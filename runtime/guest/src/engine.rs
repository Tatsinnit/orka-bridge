use crate::staging::{create, directory, invalid, Staging};
use bridge_runtime_protocol::*;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};
pub trait Runner {
    fn execute(&mut self, action: &EngineActionWire) -> io::Result<EngineResult>;
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    action: EngineActionWire,
    snapshot: Option<SnapshotId>,
    state: OperationState,
    result: Option<EngineResult>,
}
pub struct Operations<R> {
    root: PathBuf,
    runner: R,
    cached: std::cell::RefCell<Option<(Uuid, EngineResult)>>,
}
impl<R: Runner> Operations<R> {
    pub fn new(root: PathBuf, runner: R) -> io::Result<Self> {
        directory(&root)?;
        directory(&root.join("results"))?;
        Ok(Self {
            root,
            runner,
            cached: std::cell::RefCell::new(None),
        })
    }
    pub fn runner(&self) -> &R {
        &self.runner
    }
    pub fn runner_mut(&mut self) -> &mut R {
        &mut self.runner
    }
    fn path(&self, id: Uuid) -> io::Result<PathBuf> {
        if id.is_nil() {
            return Err(invalid());
        }
        Ok(self.root.join(id.to_string()))
    }
    fn metadata(&self, id: Uuid) -> io::Result<Option<Record>> {
        let path = self.path(id)?;
        match crate::linux::read_regular(&path, (MAX_FILE * 5) as u64) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|_| invalid())?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn payload(&self, id: Uuid) -> io::Result<std::cell::Ref<'_, EngineResult>> {
        if self
            .cached
            .borrow()
            .as_ref()
            .is_none_or(|(old, _)| *old != id)
        {
            let data = crate::linux::read_regular(
                &self.root.join("results").join(id.to_string()),
                (MAX_FILE * 5) as u64,
            )?;
            let output: EngineResult = serde_json::from_slice(&data).map_err(|_| invalid())?;
            if output.stdout.len() > MAX_FILE {
                return Err(invalid());
            }
            *self.cached.borrow_mut() = Some((id, output));
        }
        Ok(std::cell::Ref::map(self.cached.borrow(), |entry| {
            &entry.as_ref().expect("initialized payload cache").1
        }))
    }
    fn record(&self, id: Uuid) -> io::Result<Option<Record>> {
        let Some(mut record) = self.metadata(id)? else {
            return Ok(None);
        };
        record.result = None;
        if matches!(
            record.state,
            OperationState::Completed | OperationState::Failed
        ) {
            match self.payload(id) {
                Ok(output) => record.result = Some(output.clone()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(Some(record))
    }
    pub fn status(&self, id: Uuid) -> io::Result<OperationState> {
        Ok(self
            .record(id)?
            .map(|r| match r.state {
                OperationState::Pending | OperationState::Running => OperationState::Unknown,
                _ if r.result.is_none() => OperationState::Unknown,
                s => s,
            })
            .unwrap_or(OperationState::Unknown))
    }
    pub fn info(&self, id: Uuid) -> io::Result<OperationInfo> {
        let state = self.status(id)?;
        let r = self.record(id)?.and_then(|r| r.result);
        Ok(OperationInfo {
            state,
            exit: r.as_ref().map(|r| r.exit),
            bytes: r.as_ref().map_or(0, |r| r.stdout.len()),
            has_diagnostics: r.is_some_and(|r| r.has_diagnostics),
        })
    }
    pub fn read(&self, id: Uuid, offset: usize) -> io::Result<Vec<u8>> {
        let record = self.metadata(id)?.ok_or_else(invalid)?;
        if !matches!(
            record.state,
            OperationState::Completed | OperationState::Failed
        ) {
            return Err(invalid());
        }
        let output = self.payload(id)?;
        if offset > output.stdout.len() {
            return Err(invalid());
        }
        Ok(
            output.stdout[offset..output.stdout.len().min(offset.saturating_add(MAX_CHUNK))]
                .to_vec(),
        )
    }
    pub fn execute(
        &mut self,
        id: Uuid,
        action: EngineActionWire,
        snapshot: Option<SnapshotId>,
        staging: &Staging,
    ) -> io::Result<OperationState> {
        validate_action(&action).map_err(|_| invalid())?;
        if let Some(old) = self.record(id)? {
            if old.action != action || old.snapshot != snapshot {
                return Err(invalid());
            }
            return self.status(id);
        }
        if let Some(s) = &snapshot {
            if !staging.contains(s)? {
                return Err(invalid());
            }
        }
        let mut r = Record {
            action,
            snapshot,
            state: OperationState::Running,
            result: None,
        };
        let path = self.path(id)?;
        create(&path, &serde_json::to_vec(&r)?)?;
        fs::File::open(&self.root)?.sync_all()?;
        match self.runner.execute(&r.action) {
            Ok(mut out) if out.stdout.len() <= MAX_FILE => {
                if out.exit != 0 {
                    out.stdout.clear();
                } else {
                    match projection(&r.action, &out.stdout) {
                        Ok(bytes) => {
                            out.has_diagnostics |= matches!(r.action, EngineActionWire::Version {})
                                && out.stdout != bytes;
                            out.stdout = bytes;
                        }
                        Err(_) => {
                            out.exit = 1;
                            out.stdout.clear();
                        }
                    }
                }
                r.state = if out.exit == 0 {
                    OperationState::Completed
                } else {
                    OperationState::Failed
                };
                r.result = Some(out)
            }
            _ => r.state = OperationState::Unknown,
        }
        if let Some(output) = r.result.take() {
            let root = self.root.join("results");
            let mut entries = fs::read_dir(&root)?.collect::<io::Result<Vec<_>>>()?;
            entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
            while entries.len() >= MAX_OPERATIONS {
                let old = entries.remove(0);
                if self
                    .cached
                    .borrow()
                    .as_ref()
                    .is_some_and(|(cached, _)| old.file_name() == cached.to_string().as_str())
                {
                    self.cached.borrow_mut().take();
                }
                fs::remove_file(old.path())?;
            }
            let final_path = root.join(id.to_string());
            let temporary = final_path.with_extension("tmp");
            create(&temporary, &serde_json::to_vec(&output)?)?;
            fs::rename(temporary, final_path)?;
            fs::File::open(root)?.sync_all()?;
        }
        let next = path.with_extension("tmp");
        create(&next, &serde_json::to_vec(&r)?)?;
        fs::rename(next, path)?;
        fs::File::open(&self.root)?.sync_all()?;
        Ok(r.state)
    }
    pub fn handle(&mut self, staging: &mut Staging, body: &RequestBody) -> io::Result<ResultBody> {
        match body {
            RequestBody::StageBegin { upload, snapshot } => {
                staging.begin(*upload, snapshot.clone())?;
                Ok(ResultBody::Ready {})
            }
            RequestBody::StageChunk {
                upload,
                slot,
                offset,
                data,
            } => {
                staging.chunk(
                    *upload,
                    *slot,
                    *offset,
                    &decode_chunk(data).map_err(|_| invalid())?,
                )?;
                Ok(ResultBody::Ready {})
            }
            RequestBody::StageCommit { upload } => Ok(ResultBody::Snapshot {
                snapshot: staging.commit(*upload)?,
            }),
            RequestBody::Execute {
                operation,
                action,
                snapshot,
            } => {
                self.execute(*operation, action.clone(), snapshot.clone(), staging)?;
                Ok(ResultBody::Operation {
                    operation: *operation,
                    info: self.info(*operation)?,
                })
            }
            RequestBody::Status { operation } => Ok(ResultBody::Operation {
                operation: *operation,
                info: self.info(*operation)?,
            }),
            RequestBody::ReadResult { operation, offset } => Ok(ResultBody::Chunk {
                data: encode_chunk(&self.read(*operation, *offset)?),
            }),
            _ => Err(invalid()),
        }
    }
}
// Persist only B's finite response projection, not raw CRDs, specs or stderr.
#[derive(Deserialize)]
struct Pagination {
    #[serde(rename = "continue")]
    next: Option<String>,
    #[serde(rename = "remainingItemCount")]
    remaining: Option<u64>,
}
fn projection(action: &EngineActionWire, bytes: &[u8]) -> io::Result<Vec<u8>> {
    use serde_json::{json, Value};
    match action {
        EngineActionWire::Version {} => {
            let s = std::str::from_utf8(bytes).map_err(|_| invalid())?;
            let token = s
                .lines()
                .next()
                .and_then(|s| s.strip_prefix("kmx "))
                .and_then(|s| s.split_whitespace().next())
                .ok_or_else(invalid)?;
            if token != "dev"
                && semver::Version::parse(token.strip_prefix('v').unwrap_or(token)).is_err()
            {
                return Err(invalid());
            }
            Ok(format!("kmx {token}\n").into_bytes())
        }
        // Exact status plus bounded JSON; the runner already refused bearer echoes.
        EngineActionWire::Orka { .. } | EngineActionWire::OrkaChat { .. } => {
            let reply: OrkaReply = serde_json::from_slice(bytes).map_err(|_| invalid())?;
            let bytes = serde_json::to_vec(&reply).map_err(|_| invalid())?;
            if bytes.len() > MAX_FILE {
                return Err(invalid());
            }
            Ok(bytes)
        }
        EngineActionWire::RunWorker { .. } => {
            let reply: RunWorkerReply = serde_json::from_slice(bytes).map_err(|_| invalid())?;
            serde_json::to_vec(&reply).map_err(|_| invalid())
        }
        EngineActionWire::WorkspaceCatalog {} => {
            let catalog: WorkspaceCatalog = serde_json::from_slice(bytes).map_err(|_| invalid())?;
            if !catalog.valid() {
                return Err(invalid());
            }
            serde_json::to_vec(&catalog).map_err(|_| invalid())
        }
        EngineActionWire::ListAgents { .. } => {
            let v: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
            if !((v["kind"] == "List" && v["apiVersion"] == "v1")
                || (v["kind"] == "AgentList" && v["apiVersion"] == "core.orka.ai/v1alpha1"))
            {
                return Err(invalid());
            }
            let page: Option<Pagination> =
                serde_json::from_value(v["metadata"].clone()).map_err(|_| invalid())?;
            if page.is_some_and(|p| {
                p.next.is_some_and(|s| !s.is_empty()) || p.remaining.unwrap_or(0) > 0
            }) {
                return Err(invalid());
            }
            let mut items = Vec::new();
            for item in v["items"].as_array().ok_or_else(invalid)? {
                if item["kind"] != "Agent" || item["apiVersion"] != "core.orka.ai/v1alpha1" {
                    return Err(invalid());
                }
                let mut metadata = serde_json::Map::new();
                for key in ["name", "namespace", "uid"] {
                    let s = item["metadata"][key]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 4096)
                        .ok_or_else(invalid)?;
                    metadata.insert(key.into(), json!(s));
                }
                items.push(json!({"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":metadata}));
            }
            serde_json::to_vec(&json!({"apiVersion":"v1","kind":"List","items":items}))
                .map_err(|_| invalid())
        }
    }
}
pub struct Kmx;
/// One per serving lease: Orka keeps its verified identity, bearer and forward across calls.
pub struct GuestRunner {
    pub kmx: Kmx,
    pub orka: crate::orka::Orka,
    pub chat: crate::orka::Orka,
    pub catalog_provider: Option<String>,
}
impl GuestRunner {
    pub fn new(installation: Uuid) -> Self {
        Self {
            kmx: Kmx,
            orka: crate::orka::Orka::new(installation),
            chat: crate::orka::Orka::new_chat(installation),
            catalog_provider: None,
        }
    }
}
impl Runner for GuestRunner {
    fn execute(&mut self, action: &EngineActionWire) -> io::Result<EngineResult> {
        match action {
            EngineActionWire::Orka { request } => self.orka.execute(request),
            EngineActionWire::OrkaChat { request } => self.chat.execute_chat(request),
            EngineActionWire::RunWorker { name, uid } => self.orka.run_worker(name, uid),
            EngineActionWire::WorkspaceCatalog {} => {
                let provider = self.catalog_provider.take().ok_or_else(invalid)?;
                Ok(EngineResult {
                    exit: 0,
                    stdout: serde_json::to_vec(&self.orka.catalog(&provider))?,
                    has_diagnostics: false,
                })
            }
            _ => self.kmx.execute(action),
        }
    }
}
pub fn argv(action: &EngineActionWire) -> io::Result<Vec<String>> {
    validate_action(action).map_err(|_| invalid())?;
    Ok(match action {
        EngineActionWire::Version {} => vec!["version".into()],
        EngineActionWire::ListAgents { context, namespace } => [
            "cluster",
            "orka",
            "list",
            "--context",
            context,
            "--namespace",
            namespace,
            "-o",
            "json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        EngineActionWire::Orka { .. }
        | EngineActionWire::OrkaChat { .. }
        | EngineActionWire::RunWorker { .. }
        | EngineActionWire::WorkspaceCatalog {} => return Err(invalid()),
    })
}
impl Kmx {
    fn command(action: &EngineActionWire) -> io::Result<std::process::Command> {
        use std::{os::unix::process::CommandExt, process::Stdio};
        let mut command = std::process::Command::new("/usr/local/bin/kmx");
        command
            .args(argv(action)?)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/root")
            .env("KUBECONFIG", "/root/.kube/config")
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if matches!(action, EngineActionWire::ListAgents { .. }) {
            command
                .env("KUBECONFIG", "/var/lib/bridge/kubeconfig")
                .env("KMX_HOME", "/var/lib/bridge/kmx");
        }
        Ok(command)
    }
}
impl Runner for Kmx {
    fn execute(&mut self, action: &EngineActionWire) -> io::Result<EngineResult> {
        let child = Self::command(action)?.spawn()?;
        collect(child, std::time::Duration::from_secs(60))
    }
}
pub(crate) struct Collected {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed_ms: u64,
    pub exit_code: Option<i32>,
    pub error: Option<io::Error>,
}
// Preserve the ordinary engine contract; only model diagnostics consume partial bytes.
pub(crate) fn collect(
    child: std::process::Child,
    timeout: std::time::Duration,
) -> io::Result<EngineResult> {
    let out = collect_diagnostic(child, timeout);
    if let Some(error) = out.error {
        return Err(if error.kind() == io::ErrorKind::FileTooLarge {
            invalid()
        } else {
            error
        });
    }
    Ok(EngineResult {
        exit: out.exit_code.unwrap_or(-1),
        stdout: out.stdout,
        has_diagnostics: !out.stderr.is_empty(),
    })
}
// Nonblocking reads retain partial output without waiting for pipe EOF on timeout.
fn drain(pipe: &mut impl io::Read, bytes: &mut Vec<u8>) -> io::Result<bool> {
    let mut buffer = [0; 8192];
    loop {
        let remaining = (MAX_FILE + 1).saturating_sub(bytes.len()).min(buffer.len());
        if remaining == 0 {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        match pipe.read(&mut buffer[..remaining]) {
            Ok(0) => return Ok(true),
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}
// Caller supplies a newly-owned, unreaped child in its own process group.
pub(crate) fn collect_diagnostic(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> Collected {
    use std::{os::fd::AsRawFd, thread, time::Instant};
    let start = Instant::now();
    let mut out = Collected {
        stdout: Vec::new(),
        stderr: Vec::new(),
        elapsed_ms: 0,
        exit_code: None,
        error: None,
    };
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut nonblocking = false;
    out.error = (|| {
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // SAFETY: both descriptors are live owned pipes; flags do not change ownership.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        nonblocking = true;
        loop {
            let stdout_done = drain(&mut stdout, &mut out.stdout)?;
            let stderr_done = drain(&mut stderr, &mut out.stderr)?;
            let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: child is ours and never reaped until group cleanup; status is writable.
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    child.id(),
                    &mut status,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            if unsafe { status.si_pid() } != 0 && stdout_done && stderr_done {
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(io::ErrorKind::TimedOut.into());
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
    })()
    .err();
    // The owned group is still unreaped: its ID cannot be recycled before this kill.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    match child.wait() {
        Ok(status) => out.exit_code = status.code(),
        Err(e) => out.error = Some(e),
    }
    if nonblocking {
        // Drain bytes already in the pipes after kill/reap; never wait for escaped writers.
        for result in [
            drain(&mut stdout, &mut out.stdout),
            drain(&mut stderr, &mut out.stderr),
        ] {
            if out.error.is_none() {
                out.error = result.err();
            }
        }
    }
    out.elapsed_ms = start.elapsed().as_millis().min(u64::MAX as u128) as u64;
    out
}

#[cfg(test)]
mod driver_tests {
    use super::*;
    use std::{
        os::unix::process::CommandExt,
        process::{Child, Command, Stdio},
        time::Duration,
    };
    fn child(script: &str) -> Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap()
    }
    #[test]
    fn list_agents_uses_bootstrap_state_and_version_keeps_its_environment() {
        let list = EngineActionWire::ListAgents {
            context: "kind-bridge-fixture".into(),
            namespace: "orka-system".into(),
        };
        for (action, kubeconfig, kmx_home) in [
            (
                list,
                "/var/lib/bridge/kubeconfig",
                Some("/var/lib/bridge/kmx"),
            ),
            (EngineActionWire::Version {}, "/root/.kube/config", None),
        ] {
            let command = Kmx::command(&action).unwrap();
            assert_eq!(command.get_program(), "/usr/local/bin/kmx");
            assert_eq!(command.get_current_dir(), Some(std::path::Path::new("/")));
            let mut expected = std::collections::BTreeMap::from([
                ("HOME", "/root"),
                ("KUBECONFIG", kubeconfig),
                ("LC_ALL", "C"),
                ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ]);
            if let Some(home) = kmx_home {
                expected.insert("KMX_HOME", home);
            }
            let actual: std::collections::BTreeMap<_, _> = command
                .get_envs()
                .map(|(key, value)| (key.to_str().unwrap(), value.unwrap().to_str().unwrap()))
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn worker_result_projection_refuses_pod_details_or_invalid_counts() {
        let action: EngineActionWire =
            serde_json::from_value(serde_json::json!({"type":"runWorker",
            "name":"bridge-task-0123456789abcdef0123456789abcdef","uid":"ack-uid"}))
            .unwrap();
        assert_eq!(
            projection(&action, br#"{"finished":true,"pods":0}"#).unwrap(),
            br#"{"finished":true,"pods":0}"#
        );
        for raw in [
            br#"{"finished":true,"pods":0,"items":[{"spec":{"secret":"private"}}]}"#.as_slice(),
            br#"{"finished":true,"pods":-1}"#,
            br#"{"pods":0}"#,
        ] {
            assert!(projection(&action, raw).is_err());
        }
    }
    #[test]
    fn drains_both_pipes_and_preserves_nonzero_exit() {
        let output = collect(
            child("head -c 131072 /dev/zero & head -c 131072 /dev/zero >&2 & wait; exit 7"),
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(output.exit, 7);
        assert_eq!(output.stdout.len(), 131072);
        assert!(output.has_diagnostics);
    }
    #[test]
    fn output_bound_stops_and_reaps_the_owned_child() {
        let c = child("head -c 1048577 /dev/zero");
        let id = c.id();
        assert!(collect(c, Duration::from_secs(3)).is_err());
        assert_eq!(
            unsafe { libc::waitpid(id as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
    #[test]
    fn deadline_stops_the_owned_group_including_descendants() {
        let t = tempfile::tempdir().unwrap();
        let pid = t.path().join("pid");
        let c = Command::new("/bin/sh")
            .args(["-c", "sleep 60 & echo $! > \"$PIDFILE\"; wait"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("PIDFILE", &pid)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        assert_eq!(
            collect(c, Duration::from_millis(200)).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        let id = fs::read_to_string(pid).unwrap();
        let status = PathBuf::from(format!("/proc/{}/stat", id.trim()));
        let limit = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match fs::read_to_string(&status) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => break,
                Ok(s) if s.rsplit_once(") ").is_some_and(|(_, s)| s.starts_with('Z')) => break,
                _ => {
                    assert!(std::time::Instant::now() < limit);
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
}
