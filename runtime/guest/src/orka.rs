//! Guest-owned native Run transport: dedicated identity, in-memory bearer and
//! loopback port-forward. Bearer and forward state stay in guest memory;
//! only bounded API replies reach the host.
use bridge_runtime_protocol::*;
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[path = "catalog.rs"]
mod catalog;
#[path = "run_worker.rs"]
mod run_worker;

const IDENTITY: &str = "bridge-native-run";
const CHAT_IDENTITY: &str = "bridge-native-chat";
const OWNER: &str = "bridge-guest";
/// Renew well before the ten-minute expiry so no request is sent with a stale token.
const RENEW_AFTER: Duration = Duration::from_secs(8 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
// Include lazy setup in one budget, below the native channel's 120-second read bound.
const REQUEST_BUDGET: Duration = Duration::from_secs(90);

fn failed() -> io::Error {
    io::Error::other("orka transport unavailable")
}
fn manifests(identity: &str) -> [(&'static str, Value); 3] {
    let metadata = json!({"name":identity,"namespace":ORKA_NAMESPACE,
        "labels":{"app.kubernetes.io/managed-by":OWNER}});
    let rbac = "rbac.authorization.k8s.io";
    let mut rules = if identity == CHAT_IDENTITY {
        // RBAC grants Task deletion namespace-wide. ChatRequest validation and
        // the physical-only channel restrict its use to generated Chat Tasks.
        vec![
            json!({"apiGroups":["core.orka.ai"],"resources":["agents"],"verbs":["create","get"]}),
            json!({"apiGroups":["core.orka.ai"],"resources":["tasks"],"verbs":["create","get","delete"]}),
            json!({"apiGroups":["core.orka.ai"],"resources":["sessions"],"verbs":["get"]}),
        ]
    } else {
        vec![
            json!({"apiGroups":["core.orka.ai"],"resources":["agents","tasks"],"verbs":["create","get"]}),
        ]
    };
    rules.push(
        json!({"apiGroups":["core.orka.ai"],"resources":["tools","providers"],"verbs":["get"]}),
    );
    if identity == IDENTITY {
        rules.push(json!({"apiGroups":["core.orka.ai"],"resources":["tools"],"verbs":["list"]}));
        // Namespace-wide permissions; only validated Run names are reachable
        // through the physical host channel. Pod data stays inside the guest.
        rules.push(
            json!({"apiGroups":["core.orka.ai"],"resources":["tasks/cancel"],"verbs":["update"]}),
        );
        rules.push(json!({"apiGroups":[""],"resources":["pods"],"verbs":["list"]}));
    }
    [
        (
            "serviceaccount",
            json!({"apiVersion":"v1","kind":"ServiceAccount","metadata":metadata,
            "automountServiceAccountToken":false}),
        ),
        (
            "role.rbac.authorization.k8s.io",
            json!({"apiVersion":format!("{rbac}/v1"),"kind":"Role","metadata":metadata,"rules":rules}),
        ),
        (
            "rolebinding.rbac.authorization.k8s.io",
            json!({"apiVersion":format!("{rbac}/v1"),"kind":"RoleBinding","metadata":metadata,
            "roleRef":{"apiGroup":rbac,"kind":"Role","name":identity},
            "subjects":[{"kind":"ServiceAccount","name":identity,"namespace":ORKA_NAMESPACE}]}),
        ),
    ]
}
/// Exactly our owned object: every non-metadata manifest field matches and no secrets.
fn owned(found: &Value, expected: &Value) -> bool {
    let m = &found["metadata"];
    m["name"] == expected["metadata"]["name"]
        && m["namespace"] == ORKA_NAMESPACE
        && m["labels"]["app.kubernetes.io/managed-by"] == OWNER
        && found["secrets"].as_array().is_none_or(Vec::is_empty)
        && expected
            .as_object()
            .is_some_and(|e| e.iter().all(|(k, v)| k == "metadata" || found[k] == *v))
}
struct Token {
    value: String,
    issued: Instant,
}
struct Forward {
    child: Child,
    port: u16,
}
impl Forward {
    fn alive(&self) -> bool {
        let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: the child is ours and unreaped; WNOWAIT keeps its group ID reserved.
        let r = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut status,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        r == 0 && unsafe { status.si_pid() } == 0
    }
}
impl Drop for Forward {
    fn drop(&mut self) {
        // The group leader is still unreaped, so its process group cannot be recycled.
        unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) };
        let _ = self.child.wait();
    }
}
/// Deliberately not Debug: it holds the in-memory bearer.
pub struct Orka {
    kubectl: PathBuf,
    context: String,
    identity: &'static str,
    rbac: bool,
    token: Option<Token>,
    previous_token: Option<String>,
    forward: Option<Forward>,
    client: Option<reqwest::blocking::Client>,
}
impl Orka {
    pub fn new(installation: Uuid) -> Self {
        Self::with_kubectl(installation, "/usr/local/bin/kubectl".into())
    }
    pub fn new_chat(installation: Uuid) -> Self {
        Self {
            identity: CHAT_IDENTITY,
            ..Self::new(installation)
        }
    }
    pub fn execute_chat(&mut self, request: &ChatRequest) -> io::Result<EngineResult> {
        if self.identity != CHAT_IDENTITY {
            return Err(failed());
        }
        request.validate().map_err(|_| failed())?;
        let method = match request.method {
            ChatMethod::Get => reqwest::Method::GET,
            ChatMethod::Post => reqwest::Method::POST,
            ChatMethod::Delete => reqwest::Method::DELETE,
        };
        let reply = self.http(
            method,
            &request.path,
            request.after,
            request.body.as_ref(),
            None,
            Instant::now() + REQUEST_BUDGET,
        )?;
        Ok(EngineResult {
            exit: 0,
            stdout: serde_json::to_vec(&reply)?,
            has_diagnostics: false,
        })
    }
    fn with_kubectl(installation: Uuid, kubectl: PathBuf) -> Self {
        Self {
            kubectl,
            identity: IDENTITY,
            context: format!("--context=kind-bridge-{}", installation.simple()),
            rbac: false,
            token: None,
            previous_token: None,
            forward: None,
            client: None,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.kubectl);
        command
            .arg(&self.context)
            .arg(format!("--namespace={ORKA_NAMESPACE}"))
            .args(args)
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/root")
            .env("KUBECONFIG", "/var/lib/bridge/kubeconfig")
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        command
    }
    /// Bounded output (engine::collect); stdin is a small fixed manifest or null.
    fn run(&self, args: &[&str], input: Option<&[u8]>, deadline: Instant) -> io::Result<Vec<u8>> {
        if Instant::now() >= deadline {
            return Err(failed());
        }
        let mut command = self.command(args);
        command.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = command.spawn()?;
        if let Some(input) = input {
            let mut stdin = child.stdin.take().ok_or_else(failed)?;
            let written = stdin.write_all(input);
            drop(stdin);
            if written.is_err() {
                let _ = crate::engine::collect(
                    child,
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(COMMAND_TIMEOUT),
                );
                return Err(failed());
            }
        }
        let out = crate::engine::collect(
            child,
            deadline
                .saturating_duration_since(Instant::now())
                .min(COMMAND_TIMEOUT),
        )?;
        if out.exit != 0 {
            return Err(failed());
        }
        Ok(out.stdout)
    }
    /// Reuse only exact owned objects; create missing ones; never overwrite.
    fn identity(&mut self, deadline: Instant) -> io::Result<()> {
        for (kind, expected) in manifests(self.identity) {
            let found = self.run(
                &[
                    "get",
                    kind,
                    self.identity,
                    "--ignore-not-found",
                    "-o",
                    "json",
                ],
                None,
                deadline,
            )?;
            let found = if found.iter().all(u8::is_ascii_whitespace) {
                let manifest = serde_json::to_vec(&expected)?;
                self.run(
                    &["create", "-f", "-", "-o", "json"],
                    Some(&manifest),
                    deadline,
                )?
            } else {
                found
            };
            let found: Value = serde_json::from_slice(&found).map_err(|_| failed())?;
            if !owned(&found, &expected) {
                return Err(failed());
            }
        }
        self.rbac = true;
        Ok(())
    }
    fn renew(&mut self, deadline: Instant) -> io::Result<()> {
        let bytes = self.run(
            &["create", "token", self.identity, "--duration=10m"],
            None,
            deadline,
        )?;
        let value = String::from_utf8(bytes)
            .map_err(|_| failed())?
            .trim_end()
            .to_owned();
        if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(failed());
        }
        // Eight-minute renewal overlaps the requested ten-minute lifetime.
        // Only the immediately preceding token can still be valid at the next renewal.
        self.previous_token = self.token.take().map(|token| token.value);
        self.token = Some(Token {
            value,
            issued: Instant::now(),
        });
        Ok(())
    }
    fn forward(&self, deadline: Instant) -> io::Result<Forward> {
        let mut child = self
            .command(&[
                "port-forward",
                "--address=127.0.0.1",
                "svc/orka-api",
                "0:api",
            ])
            .stdin(Stdio::null())
            .spawn()?;
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let mut forward = Forward { child, port: 0 };
        let (tx, rx) = std::sync::mpsc::channel();
        let (mut stdout, mut stderr) = (stdout.ok_or_else(failed)?, stderr.ok_or_else(failed)?);
        // Pumps keep kubectl from blocking on full pipes; memory stays bounded.
        std::thread::spawn(move || {
            let mut line = Vec::new();
            let mut byte = [0];
            while line.len() < 128
                && stdout.read(&mut byte).is_ok_and(|n| n == 1)
                && byte[0] != b'\n'
            {
                line.push(byte[0]);
            }
            let _ = tx.send(line);
            let _ = io::copy(&mut stdout, &mut io::sink());
        });
        std::thread::spawn(move || io::copy(&mut stderr, &mut io::sink()));
        let line = rx
            .recv_timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(COMMAND_TIMEOUT),
            )
            .map_err(|_| failed())?;
        // The guest image pins kubectl; require its loopback banner and fail closed.
        forward.port = std::str::from_utf8(&line)
            .ok()
            .and_then(|l| l.strip_prefix("Forwarding from 127.0.0.1:"))
            .and_then(|l| l.split_once(" -> "))
            .and_then(|(port, _)| port.parse().ok())
            .filter(|port| *port != 0)
            .ok_or_else(failed)?;
        Ok(forward)
    }
    /// One HTTP attempt per call: never retried or replayed, including POSTs.
    pub fn execute(&mut self, request: &OrkaRequest) -> io::Result<EngineResult> {
        self.execute_until(request, Instant::now() + REQUEST_BUDGET)
    }
    fn execute_until(
        &mut self,
        request: &OrkaRequest,
        deadline: Instant,
    ) -> io::Result<EngineResult> {
        if self.identity != IDENTITY {
            return Err(failed());
        }
        let post = request.validate().map_err(|_| failed())?;
        let reply = self.http(
            if post {
                reqwest::Method::POST
            } else {
                reqwest::Method::GET
            },
            &request.path,
            request.after,
            request.body.as_ref(),
            None,
            deadline,
        )?;
        Ok(EngineResult {
            exit: 0,
            stdout: serde_json::to_vec(&reply)?,
            has_diagnostics: false,
        })
    }
    fn http(
        &mut self,
        method: reqwest::Method,
        path: &str,
        after: Option<u64>,
        body: Option<&Value>,
        continuation: Option<&str>,
        deadline: Instant,
    ) -> io::Result<OrkaReply> {
        if !self.rbac {
            self.identity(deadline)?;
        }
        if self
            .token
            .as_ref()
            .is_none_or(|t| t.issued.elapsed() >= RENEW_AFTER)
        {
            self.renew(deadline)?;
        }
        match &self.forward {
            Some(f) if !f.alive() => {
                self.forward = None;
                return Err(failed());
            }
            Some(_) => {}
            None => self.forward = Some(self.forward(deadline)?),
        }
        if self.client.is_none() {
            self.client = Some(
                reqwest::blocking::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .pool_max_idle_per_host(0)
                    .connect_timeout(Duration::from_secs(5))
                    .timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|_| failed())?,
            );
        }
        let (Some(client), Some(token), Some(forward)) = (&self.client, &self.token, &self.forward)
        else {
            return Err(failed());
        };
        let mut url = format!(
            "http://127.0.0.1:{}/api/v1/{}?namespace={ORKA_NAMESPACE}",
            forward.port, path
        );
        if let Some(cursor) = continuation {
            let mut parsed = reqwest::Url::parse(&url).map_err(|_| failed())?;
            parsed
                .query_pairs_mut()
                .append_pair("limit", "100")
                .append_pair("continue", cursor);
            url = parsed.into();
        }
        if let Some(after) = after {
            url.push_str(&format!("&after={after}&limit=1000"));
        }
        let mut http = client
            .request(method, url)
            .bearer_auth(&token.value)
            .header("Accept-Encoding", "identity");
        if let Some(body) = body {
            http = http
                .header("Content-Type", "application/json")
                .body(serde_json::to_vec(body)?);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(failed());
        }
        let response = http
            .timeout(remaining.min(Duration::from_secs(10)))
            .send()
            .map_err(|_| failed())?;
        let status = response.status().as_u16();
        let mut raw = Vec::new();
        response
            .take(MAX_ORKA_REPLY as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| failed())?;
        if raw.len() > MAX_ORKA_REPLY || Instant::now() >= deadline {
            return Err(failed());
        }
        let body = serde_json::from_slice(&raw).unwrap_or(Value::Null);
        let reply = OrkaReply { status, body };
        let stdout = serde_json::to_vec(&reply)?;
        if stdout.len() > MAX_FILE
            || std::iter::once(token.value.as_str())
                .chain(self.previous_token.as_deref())
                .any(|secret| {
                    [&raw, &stdout]
                        .iter()
                        .any(|b| b.windows(secret.len()).any(|w| w == secret.as_bytes()))
                })
        {
            return Err(failed());
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("orka_chat_tests.rs");
    include!("orka_run_worker_tests.rs");
    use serde_json::{json, Value};
    use std::{
        collections::VecDeque,
        fs,
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
        time::Duration,
    };
    const TASK: &str = "bridge-task-0123456789abcdef0123456789abcdef";
    const RUN: &str = "bridge-run-0123456789abcdef0123456789abcdef";
    fn installation() -> Uuid {
        Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap()
    }
    const CTX: &str = "--context=kind-bridge-1234567812344abc8def123456789abc";
    /// Canned replies: Some(raw HTTP response) or None to close after reading.
    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<String>>>,
    }
    fn server(replies: Vec<Option<Vec<u8>>>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let mut replies = VecDeque::from(replies);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                        length = n.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let auth = head
                    .lines()
                    .find_map(|l| l.strip_prefix("authorization: "))
                    .unwrap_or("")
                    .to_owned();
                head.push_str(&String::from_utf8(body).unwrap());
                log.lock().unwrap().push(head);
                if let Some(reply) = replies.pop_front().flatten() {
                    let reply = String::from_utf8(reply).unwrap().replace("{AUTH}", &auth);
                    stream.write_all(reply.as_bytes()).unwrap();
                }
            }
        });
        Server { port, seen }
    }
    fn http(status: &str, body: &str) -> Option<Vec<u8>> {
        // Echo bodies are close-delimited: their length changes after substitution.
        let length = if body.contains("{AUTH}") {
            String::new()
        } else {
            format!("Content-Length: {}\r\n", body.len())
        };
        Some(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{length}Connection: close\r\n\r\n{body}"
            )
            .into_bytes(),
        )
    }
    fn write_kubectl(path: &Path, script: &str) {
        // A parallel spawn can inherit even O_CLOEXEC writers until it execs.
        // Keep the writer out of this process, and wait for it to close before use.
        let mut writer = Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\"", "kubectl-fixture"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        writer
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        assert!(writer.wait().unwrap().success());
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    /// A fake kubectl recording argv; existing objects live in `<kind>.json`.
    fn kube(port: u16) -> (tempfile::TempDir, Orka) {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        let script = format!(
            r#"#!/bin/sh
D={d}
printf '%s\n' "$*" >> $D/log
case "$3" in
get) if [ "$4" = "--raw" ]; then cat $D/models.json; else cat $D/$4.json 2>/dev/null || true; fi ;;
create)
  if [ "$4" = token ]; then
    echo x >> $D/tokens
    printf 'synthetic-bearer-%s\n' $(wc -l < $D/tokens)
  else
    cat | tee -a $D/created
    echo >> $D/created
  fi ;;
port-forward)
  sleep 600 &
  echo $! > $D/grandchild
  echo "Forwarding from 127.0.0.1:{port} -> 8080"
  wait ;;
esac
"#
        );
        let path = dir.path().join("kubectl");
        write_kubectl(&path, &script);
        let orka = Orka::with_kubectl(installation(), path);
        (dir, orka)
    }
    fn log(dir: &Path) -> Vec<String> {
        fs::read_to_string(dir.join("log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    fn get(path: &str) -> OrkaRequest {
        OrkaRequest {
            path: path.into(),
            after: None,
            body: None,
        }
    }
    fn create() -> OrkaRequest {
        OrkaRequest {
            path: "agents".into(),
            after: None,
            body: Some(json!({"name":RUN,"namespace":"orka-system","spec":{"model":{"name":"m"}}})),
        }
    }
    fn reply(result: EngineResult) -> OrkaReply {
        assert_eq!(result.exit, 0);
        assert!(!result.has_diagnostics);
        serde_json::from_slice(&result.stdout).unwrap()
    }
    fn gone(pid: &str) {
        let status = PathBuf::from(format!("/proc/{}/stat", pid.trim()));
        let limit = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match fs::read_to_string(&status) {
                Err(_) => return,
                Ok(s) if s.rsplit_once(") ").is_some_and(|(_, s)| s.starts_with('Z')) => return,
                _ => {
                    assert!(std::time::Instant::now() < limit, "process survived");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    #[test]
    fn production_uses_fixed_kubectl_context_and_kubeconfig() {
        let orka = Orka::new(installation());
        assert_eq!(orka.kubectl, Path::new("/usr/local/bin/kubectl"));
        let command = orka.command(&["version"]);
        assert_eq!(command.get_program(), "/usr/local/bin/kubectl");
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, [CTX, "--namespace=orka-system", "version"]);
        let env: Vec<_> = command
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(
            env,
            [
                ("HOME", "/root"),
                ("KUBECONFIG", "/var/lib/bridge/kubeconfig"),
                ("LC_ALL", "C"),
                ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ]
        );
    }

    #[test]
    fn kubectl_fixtures_execute_during_parallel_publication() {
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..16 {
                        let (dir, orka) = kube(1);
                        let output = orka
                            .run(&["version"], None, Instant::now() + COMMAND_TIMEOUT)
                            .expect("published fixture must be executable, even while peers spawn");
                        assert!(output.is_empty());
                        assert_eq!(
                            log(dir.path()),
                            [format!("{CTX} --namespace=orka-system version")]
                        );
                    }
                });
            }
        });
    }

    #[test]
    fn first_call_creates_exact_owned_identity_then_reuses_session() {
        let s = server(vec![
            http("200 OK", r#"{"kind":"Task"}"#),
            http("201 Created", r#"{"kind":"Agent"}"#),
        ]);
        let (dir, mut orka) = kube(s.port);
        let got = reply(orka.execute(&get(&format!("tasks/{TASK}"))).unwrap());
        assert_eq!(
            got,
            OrkaReply {
                status: 200,
                body: json!({"kind":"Task"})
            }
        );
        let created: Vec<Value> = fs::read_to_string(dir.path().join("created"))
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let labels = json!({"app.kubernetes.io/managed-by":"bridge-guest"});
        let meta = json!({"name":"bridge-native-run","namespace":"orka-system","labels":labels});
        assert_eq!(
            created,
            [
                json!({"apiVersion":"v1","kind":"ServiceAccount","metadata":meta,"automountServiceAccountToken":false}),
                json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":meta,"rules":[
                    {"apiGroups":["core.orka.ai"],"resources":["agents","tasks"],"verbs":["create","get"]},
                    {"apiGroups":["core.orka.ai"],"resources":["tools","providers"],"verbs":["get"]},
                    {"apiGroups":["core.orka.ai"],"resources":["tools"],"verbs":["list"]},
                    {"apiGroups":["core.orka.ai"],"resources":["tasks/cancel"],"verbs":["update"]},
                    {"apiGroups":[""],"resources":["pods"],"verbs":["list"]}]}),
                json!({"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":meta,
                    "roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"Role","name":"bridge-native-run"},
                    "subjects":[{"kind":"ServiceAccount","name":"bridge-native-run","namespace":"orka-system"}]}),
            ]
        );
        assert!(!fs::read_to_string(dir.path().join("created"))
            .unwrap()
            .contains("secret"));
        let first = log(dir.path());
        let p = |rest: &str| format!("{CTX} --namespace=orka-system {rest}");
        assert_eq!(
            first,
            [
                p("get serviceaccount bridge-native-run --ignore-not-found -o json"),
                p("create -f - -o json"),
                p("get role.rbac.authorization.k8s.io bridge-native-run --ignore-not-found -o json"),
                p("create -f - -o json"),
                p("get rolebinding.rbac.authorization.k8s.io bridge-native-run --ignore-not-found -o json"),
                p("create -f - -o json"),
                p("create token bridge-native-run --duration=10m"),
                p("port-forward --address=127.0.0.1 svc/orka-api 0:api"),
            ]
        );
        let got = reply(orka.execute(&create()).unwrap());
        assert_eq!(got.status, 201);
        assert_eq!(
            log(dir.path()),
            first,
            "session reused without new commands"
        );
        let seen = s.seen.lock().unwrap().clone();
        assert!(seen[0].starts_with(&format!(
            "GET /api/v1/tasks/{TASK}?namespace=orka-system HTTP/1.1\r\n"
        )));
        assert!(seen[1].starts_with("POST /api/v1/agents?namespace=orka-system HTTP/1.1\r\n"));
        for request in &seen {
            assert!(request.contains("authorization: Bearer synthetic-bearer-1\r\n"));
            assert!(request.contains(&format!("host: 127.0.0.1:{}\r\n", s.port)));
            assert!(request.contains("accept-encoding: identity\r\n"));
        }
        assert!(seen[1].contains("content-type: application/json\r\n"));
        assert!(seen[1].ends_with(&serde_json::to_string(&create().body).unwrap()));
    }

    #[test]
    fn existing_matching_identity_is_reused_and_mismatch_is_refused_not_overwritten() {
        let s = server(vec![http("200 OK", "{}")]);
        let (dir, mut orka) = kube(s.port);
        let manifests = manifests(IDENTITY);
        for (kind, value) in &manifests {
            let mut value = value.clone();
            value["metadata"]["uid"] = json!("server-added");
            fs::write(dir.path().join(format!("{kind}.json")), value.to_string()).unwrap();
        }
        assert_eq!(
            reply(orka.execute(&get(&format!("agents/{RUN}"))).unwrap()).status,
            200
        );
        assert!(!dir.path().join("created").exists());

        let mismatches = [
            // An old Run Role is not upgraded in place, even for an ordinary read.
            (
                1,
                json!({"rules":[
                {"apiGroups":["core.orka.ai"],"resources":["agents","tasks"],"verbs":["create","get"]},
                {"apiGroups":["core.orka.ai"],"resources":["tools","providers"],"verbs":["get"]},
                {"apiGroups":["core.orka.ai"],"resources":["tools"],"verbs":["list"]}]}),
            ),
            (0, json!({"secrets":[{"name":"x"}]})),
            (0, json!({"automountServiceAccountToken":true})),
            (
                0,
                json!({"metadata":{"name":"bridge-native-run","namespace":"orka-system"}}),
            ),
            (
                1,
                json!({"rules":[{"apiGroups":[""],"resources":["secrets"],"verbs":["get"]}]}),
            ),
            (
                2,
                json!({"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"bridge-native-run"}}),
            ),
        ];
        for (index, patch) in mismatches {
            let (dir, mut orka) = kube(s.port);
            for (i, (kind, value)) in manifests.iter().enumerate() {
                let mut value = value.clone();
                if i == index {
                    for (k, v) in patch.as_object().unwrap() {
                        value[k] = v.clone();
                    }
                }
                fs::write(dir.path().join(format!("{kind}.json")), value.to_string()).unwrap();
            }
            assert!(
                orka.execute(&get(&format!("agents/{RUN}"))).is_err(),
                "{patch}"
            );
            assert!(
                !dir.path().join("created").exists(),
                "never overwrite {patch}"
            );
            assert!(!log(dir.path())
                .iter()
                .any(|l| l.contains("token") || l.contains("port-forward")));
        }
        assert_eq!(s.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn invalid_requests_run_no_command() {
        let (dir, mut orka) = kube(1);
        for request in [get("secrets"), get(&format!("tasks/{TASK}/events"))] {
            assert!(orka.execute(&request).is_err());
        }
        assert!(log(dir.path()).is_empty());
    }

    #[test]
    fn status_is_preserved_non_json_is_null_and_oversize_is_refused() {
        let big = format!("\"{}\"", "x".repeat(MAX_ORKA_REPLY));
        let s = server(vec![
            http("404 Not Found", r#"{"error":"missing"}"#),
            http("500 Internal Server Error", "panic: raw diagnostic"),
            http("200 OK", ""),
            http("200 OK", &big),
        ]);
        let (_dir, mut orka) = kube(s.port);
        let path = format!("tasks/{TASK}/result");
        assert_eq!(
            reply(orka.execute(&get(&path)).unwrap()),
            OrkaReply {
                status: 404,
                body: json!({"error":"missing"})
            }
        );
        let failed = orka.execute(&get(&path)).unwrap();
        assert!(!String::from_utf8_lossy(&failed.stdout).contains("panic"));
        assert_eq!(
            reply(failed),
            OrkaReply {
                status: 500,
                body: Value::Null
            }
        );
        assert_eq!(reply(orka.execute(&get(&path)).unwrap()).body, Value::Null);
        assert!(orka.execute(&get(&path)).is_err());
    }

    #[test]
    fn first_run_setup_shares_one_deadline_and_never_sends_a_late_post() {
        let s = server(vec![http("201 Created", "{}")]);
        let (dir, mut orka) = kube(s.port);
        let script = fs::read_to_string(&orka.kubectl).unwrap();
        write_kubectl(
            &orka.kubectl,
            &script.replace("#!/bin/sh\n", "#!/bin/sh\nsleep 0.04\n"),
        );
        let deadline = Instant::now() + Duration::from_millis(70);
        assert!(orka.execute_until(&create(), deadline).is_err());
        assert!(
            s.seen.lock().unwrap().is_empty(),
            "no POST after setup exhausts its budget"
        );
        assert!(
            log(dir.path()).len() < 8,
            "the budget spans setup commands, not each command separately"
        );
    }

    #[test]
    fn token_renews_before_expiry_and_a_lost_post_is_never_replayed() {
        let s = server(vec![http("201 Created", "{}"), None, http("200 OK", "{}")]);
        let (dir, mut orka) = kube(s.port);
        assert_eq!(reply(orka.execute(&create()).unwrap()).status, 201);
        let token = orka.token.as_mut().unwrap();
        token.issued = token
            .issued
            .checked_sub(Duration::from_secs(8 * 60 + 1))
            .unwrap();
        assert!(orka.execute(&create()).is_err());
        let seen = s.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "lost POST sent exactly once");
        assert!(seen[1].contains("authorization: Bearer synthetic-bearer-2\r\n"));
        let commands = log(dir.path());
        assert_eq!(
            commands
                .iter()
                .filter(|l| l.contains("create token"))
                .count(),
            2
        );
        assert_eq!(
            commands
                .iter()
                .filter(|l| l.contains("port-forward"))
                .count(),
            1
        );
        assert_eq!(
            reply(orka.execute(&get(&format!("agents/{RUN}"))).unwrap()).status,
            200
        );
        assert_eq!(s.seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn echoed_bearer_is_refused_and_never_persisted() {
        let s = server(vec![
            http("201 Created", r#"{"kind":"Agent"}"#),
            http("401 Unauthorized", r#"{"error":"bad {AUTH}"}"#),
            http("200 OK", r#"{"echo":"\u0073ynthetic-bearer-1"}"#),
        ]);
        let (_dir, orka) = kube(s.port);
        let root = tempfile::tempdir().unwrap();
        let staging = crate::staging::Staging::new(root.path().join("snapshots")).unwrap();
        let runner = crate::engine::GuestRunner {
            kmx: crate::engine::Kmx,
            orka,
            chat: Orka::new_chat(installation()),
            catalog_provider: None,
        };
        let mut ops = crate::engine::Operations::new(root.path().join("ops"), runner).unwrap();
        let action = |request| EngineActionWire::Orka { request };
        let ok = Uuid::new_v4();
        assert_eq!(
            ops.execute(ok, action(create()), None, &staging).unwrap(),
            OperationState::Completed
        );
        for _ in 0..2 {
            let echoed = Uuid::new_v4();
            assert_eq!(
                ops.execute(
                    echoed,
                    action(get(&format!("agents/{RUN}"))),
                    None,
                    &staging
                )
                .unwrap(),
                OperationState::Unknown
            );
        }
        assert_eq!(s.seen.lock().unwrap().len(), 3);
        let reply: OrkaReply = serde_json::from_slice(&ops.read(ok, 0).unwrap()).unwrap();
        assert_eq!(reply.body, json!({"kind":"Agent"}));
        let mut files = 0;
        for entry in walk(&root.path().join("ops")) {
            let bytes = fs::read(&entry).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("synthetic-bearer"));
            files += 1;
        }
        assert!(
            files >= 4,
            "records and the completed result were inspected"
        );
    }
    #[test]
    fn rotated_bearer_echo_is_refused_before_operation_cache_publication() {
        let s = server(vec![
            http("200 OK", "{}"),
            http("200 OK", r#"{"echo":"synthetic-bearer-1"}"#),
            http("200 OK", r#"{"echo":"\u0073ynthetic-bearer-1"}"#),
        ]);
        let (_dir, mut orka) = kube(s.port);
        orka.execute(&get(&format!("agents/{RUN}"))).unwrap();
        orka.token.as_mut().unwrap().issued -= RENEW_AFTER;
        let root = tempfile::tempdir().unwrap();
        let staging = crate::staging::Staging::new(root.path().join("snapshots")).unwrap();
        let runner = crate::engine::GuestRunner {
            kmx: crate::engine::Kmx,
            orka,
            chat: Orka::new_chat(installation()),
            catalog_provider: None,
        };
        let mut ops = crate::engine::Operations::new(root.path().join("ops"), runner).unwrap();
        for _ in 0..2 {
            assert_eq!(
                ops.execute(
                    Uuid::new_v4(),
                    EngineActionWire::Orka {
                        request: get(&format!("agents/{RUN}"))
                    },
                    None,
                    &staging
                )
                .unwrap(),
                OperationState::Unknown
            );
        }
        for entry in walk(&root.path().join("ops")) {
            assert!(
                !String::from_utf8_lossy(&fs::read(entry).unwrap()).contains("synthetic-bearer")
            );
        }
        assert_eq!(s.seen.lock().unwrap().len(), 3);
    }
    fn walk(path: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn catalog_uses_fixed_model_service_and_encoded_tool_pages() {
        let s = server(vec![
            http(
                "200 OK",
                r#"{"items":[{"builtin":true,"name":"web_fetch","description":"Fetch"}],"metadata":{"continue":"a+/="}}"#,
            ),
            http(
                "200 OK",
                r#"{"items":[{"builtin":false,"name":"custom","description":"Custom","url":"private"}],"metadata":{}}"#,
            ),
        ]);
        let (dir, mut orka) = kube(s.port);
        fs::write(
            dir.path().join("models.json"),
            r#"{"data":[{"id":"m"}],"token":"synthetic-private"}"#,
        )
        .unwrap();
        let c = orka.catalog("copilot");
        assert!(c.valid());
        assert!(matches!(c.models, CatalogSection::Ready { .. }));
        assert!(matches!(c.tools, CatalogSection::Ready { .. }));
        let seen = s.seen.lock().unwrap();
        assert!(seen[0].starts_with("GET /api/v1/tools?namespace=orka-system&limit=100&continue= "));
        assert!(seen[1]
            .starts_with("GET /api/v1/tools?namespace=orka-system&limit=100&continue=a%2B%2F%3D "));
        assert!(log(dir.path()).iter().any(|l| l == &format!("{CTX} --namespace=orka-system get --raw /api/v1/namespaces/orka-system/services/http:copilot-vekil:1337/proxy/v1/models")));
        let bytes = serde_json::to_string(&c).unwrap();
        assert!(!bytes.contains("private") && !bytes.contains("url"));
    }
    #[test]
    fn catalog_failures_are_independent_and_share_one_budget() {
        let s = server(vec![http(
            "403 Forbidden",
            r#"{"message":"synthetic-private"}"#,
        )]);
        let (dir, mut orka) = kube(s.port);
        fs::write(dir.path().join("models.json"), r#"{"data":[]}"#).unwrap();
        let c = orka.catalog("copilot");
        assert_eq!(c.models, CatalogSection::Ready { items: Vec::new() });
        assert_eq!(c.tools, CatalogSection::Unavailable {});
        let before = log(dir.path());
        let c = orka.catalog_until("copilot", Instant::now());
        assert_eq!(c.models, CatalogSection::Unavailable {});
        assert_eq!(c.tools, CatalogSection::Unavailable {});
        assert_eq!(log(dir.path()), before);
    }
    #[test]
    fn catalog_bearer_echo_is_withheld_before_result_persistence() {
        let s = server(vec![http(
            "200 OK",
            r#"{"items":[{"builtin":false,"name":"custom","description":"{AUTH}"}],"metadata":{}}"#,
        )]);
        let (dir, orka) = kube(s.port);
        fs::write(dir.path().join("models.json"), r#"{"data":[]}"#).unwrap();
        let root = tempfile::tempdir().unwrap();
        let runner = crate::engine::GuestRunner {
            kmx: crate::engine::Kmx,
            orka,
            chat: Orka::new_chat(installation()),
            catalog_provider: Some("copilot".into()),
        };
        let mut ops = crate::engine::Operations::new(root.path().join("ops"), runner).unwrap();
        let staging = crate::staging::Staging::new(root.path().join("staging")).unwrap();
        let id = Uuid::new_v4();
        ops.execute(id, EngineActionWire::WorkspaceCatalog {}, None, &staging)
            .unwrap();
        let c: WorkspaceCatalog = serde_json::from_slice(&ops.read(id, 0).unwrap()).unwrap();
        assert_eq!(c.tools, CatalogSection::Unavailable {});
        assert!(ops.runner().catalog_provider.is_none());
        for path in walk(root.path()) {
            assert!(!String::from_utf8_lossy(&fs::read(path).unwrap()).contains("synthetic-bearer"));
        }
    }
    #[test]
    fn dead_forward_fails_without_replay_and_drop_reaps_the_whole_group() {
        let s = server(vec![http("200 OK", "{}"), http("200 OK", "{}")]);
        let (dir, mut orka) = kube(s.port);
        let path = format!("agents/{RUN}");
        orka.execute(&get(&path)).unwrap();
        let first = orka.forward.as_ref().unwrap().child.id();
        let grandchild = fs::read_to_string(dir.path().join("grandchild")).unwrap();
        unsafe { libc::kill(first as i32, libc::SIGKILL) };
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            orka.execute(&get(&path)).is_err(),
            "dead forward is a transport failure"
        );
        assert_eq!(
            s.seen.lock().unwrap().len(),
            1,
            "no request sent or replayed"
        );
        gone(&grandchild);
        assert_eq!(
            unsafe { libc::waitpid(first as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1,
            "dead forward was reaped"
        );
        // A later call is a new request that may open a fresh forward.
        orka.execute(&get(&path)).unwrap();
        let second = orka.forward.as_ref().unwrap().child.id();
        let grandchild = fs::read_to_string(dir.path().join("grandchild")).unwrap();
        drop(orka);
        gone(&grandchild);
        assert_eq!(
            unsafe { libc::waitpid(second as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(s.seen.lock().unwrap().len(), 2);
    }
}
