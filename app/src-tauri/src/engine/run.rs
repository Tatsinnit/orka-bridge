//! Native-only authoring API. Not a renderer command or guest protocol extension.
#[path = "run_source.rs"]
mod source;
pub use source::{Assertion, DeclaredTool, ParsedAgent, ParsedPair, RunCase, ToolKind};
#[path = "run_http.rs"]
mod http;
use bridge_runtime_protocol::{SnapshotId, Uuid};
pub use http::{NativeRunTarget, RunFuture, RunLimits, RunTransport};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt;
use tokio::{sync::watch, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Source,
    Configuration,
    Transport,
    Authentication,
    Permission,
    Deadline,
    Response,
    Identity,
    Stopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Passed,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct AssertionReport {
    pub id: String,
    pub kind: &'static str,
    pub expected: String,
    pub verdict: Verdict,
}
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseReport {
    pub id: String,
    pub question: String,
    pub phase: String,
    pub answer: Option<String>,
    /// Names from attributable completed calls, not a complete URL/body trace.
    pub completed_tools: Vec<String>,
    pub assertions: Vec<AssertionReport>,
    pub verdict: Verdict,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct RunReport {
    pub cases: Vec<CaseReport>,
    pub verdict: Verdict,
}
// Missing optional evidence remains unknown; Stop must not be swallowed as
// an unavailable answer/event page and continue issuing requests.
fn optional_evidence<T>(result: Result<T, ErrorKind>) -> Result<Option<T>, ErrorKind> {
    match result {
        Err(kind @ (ErrorKind::Stopped | ErrorKind::Authentication | ErrorKind::Permission)) => {
            Err(kind)
        }
        other => Ok(other.ok()),
    }
}
fn aggregate(rows: impl Iterator<Item = Verdict>) -> Verdict {
    let rows: Vec<_> = rows.collect();
    if rows.contains(&Verdict::Failed) {
        Verdict::Failed
    } else if rows.is_empty() || rows.contains(&Verdict::Unknown) {
        Verdict::Unknown
    } else {
        Verdict::Passed
    }
}
#[derive(Clone)]
pub struct ResourceId {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub generation: u64,
    spec: Value,
    kind: &'static str,
}
impl ResourceId {
    pub(crate) fn acknowledge(
        value: &Value,
        requested: &Value,
        name: &str,
        namespace: &str,
        kind: &'static str,
    ) -> Result<Self, ErrorKind> {
        let metadata = &value["metadata"];
        let uid = metadata["uid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(ErrorKind::Identity)?;
        let generation = metadata["generation"].as_u64().unwrap_or(0);
        let mut requested = requested.clone();
        if kind == "Task" {
            // Go serializes this fixed 120-second duration canonically and omits
            // MaxRetries=0. Still refuse any actual retry authority in the reply.
            if requested["timeout"] == "120s" {
                requested["timeout"] = json!("2m0s");
            }
            if value["spec"]["retryPolicy"]
                .get("maxRetries")
                .is_some_and(|v| v != 0)
            {
                return Err(ErrorKind::Identity);
            }
            if let Some(policy) = requested["retryPolicy"].as_object_mut() {
                policy.remove("maxRetries");
            }
        }
        // Create handlers omit TypeMeta; the fixed POST endpoint supplies its
        // type. An explicit conflicting type is never accepted.
        if value
            .get("apiVersion")
            .is_some_and(|v| v != "core.orka.ai/v1alpha1")
            || value.get("kind").is_some_and(|v| v != kind)
            || metadata["name"] != name
            || metadata["namespace"] != namespace
            || kind == "Agent" && generation == 0
            || !includes(&value["spec"], &requested)
        {
            return Err(ErrorKind::Identity);
        }
        Ok(Self {
            name: name.into(),
            namespace: namespace.into(),
            uid: uid.into(),
            generation,
            spec: value["spec"].clone(),
            kind,
        })
    }
    pub(crate) fn unchanged(&self, value: &Value) -> Result<(), ErrorKind> {
        if value["apiVersion"] != "core.orka.ai/v1alpha1" || value["kind"] != self.kind {
            return Err(ErrorKind::Identity);
        }
        let current = Self::acknowledge(value, &self.spec, &self.name, &self.namespace, self.kind)?;
        if current.uid != self.uid
            || current.generation != self.generation
            || current.spec != self.spec
        {
            return Err(ErrorKind::Identity);
        }
        Ok(())
    }
    pub(crate) fn path(&self) -> String {
        format!(
            "{}/{}",
            if self.kind == "Agent" {
                "agents"
            } else {
                "tasks"
            },
            self.name
        )
    }
}
fn includes(value: &Value, requested: &Value) -> bool {
    match (value, requested) {
        (Value::Object(value), Value::Object(requested)) => requested
            .iter()
            .all(|(k, v)| value.get(k).is_some_and(|got| includes(got, v))),
        (Value::Array(value), Value::Array(requested)) => {
            value.len() == requested.len()
                && value.iter().zip(requested).all(|(a, b)| includes(a, b))
        }
        _ => value == requested,
    }
}
/// An in-memory receipt, tied to this runner. Clone and retain it before observing.
#[derive(Clone)]
pub struct RunHandle {
    pair: ParsedPair,
    target: Uuid,
    agent: Option<ResourceId>,
    tasks: Vec<ResourceId>,
}
impl RunHandle {
    pub fn snapshot(&self) -> &SnapshotId {
        self.pair.snapshot()
    }
    pub fn agent(&self) -> Option<&ResourceId> {
        self.agent.as_ref()
    }
    pub fn tasks(&self) -> &[ResourceId] {
        &self.tasks
    }
}
impl fmt::Debug for RunHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunHandle")
            .field("acknowledged_tasks", &self.tasks.len())
            .finish_non_exhaustive()
    }
}
/// This generated name may have been created even when no response was received.
#[derive(Clone)]
pub struct PendingMutation {
    pub kind: &'static str,
    pub name: String,
}
pub struct RunError {
    pub kind: ErrorKind,
    pub handle: Box<RunHandle>,
    pub pending: Option<PendingMutation>,
}
impl fmt::Debug for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Native Run error: {:?}", self.kind)
    }
}
impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
impl std::error::Error for RunError {}

/// A one-use, in-memory plan tied to the runner that prepared it. Only the
/// planned Agent name and source identity are public; Task specs stay private.
pub struct PreparedRun {
    handle: RunHandle,
    creates: Vec<Value>,
}
impl PreparedRun {
    pub fn agent_name(&self) -> &str {
        self.creates[0]["name"].as_str().unwrap()
    }
    pub fn snapshot(&self) -> &SnapshotId {
        self.handle.snapshot()
    }
}
fn check_stop(stop: &watch::Receiver<bool>) -> Result<(), ErrorKind> {
    if *stop.borrow() {
        Err(ErrorKind::Stopped)
    } else {
        Ok(())
    }
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            // A caller dropping its sender is not an explicit Stop (legacy wrappers).
            std::future::pending::<()>().await;
        }
    }
}
async fn stoppable<T>(
    stop: &mut watch::Receiver<bool>,
    work: impl std::future::Future<Output = Result<T, ErrorKind>>,
) -> Result<T, ErrorKind> {
    check_stop(stop)?;
    tokio::select! {
        // Preserve a ready acknowledgment, including a synchronous VM response
        // that cannot yield mid-request. The next request still checks Stop.
        biased;
        result = work => result,
        _ = stopped(stop) => Err(ErrorKind::Stopped),
    }
}

pub struct NativeRunner<'a> {
    http: Box<dyn RunTransport + 'a>,
    id: Uuid,
}
impl NativeRunner<'static> {
    pub fn new(target: NativeRunTarget, limits: RunLimits) -> Result<Self, ErrorKind> {
        Ok(Self::with_transport(http::Http::new(target, limits)?))
    }
}
impl<'a> NativeRunner<'a> {
    pub fn with_transport(transport: impl RunTransport + 'a) -> Self {
        Self {
            http: Box::new(transport),
            id: Uuid::new_v4(),
        }
    }
    /// Map and validate every identical whole create body before the first POST.
    /// No I/O occurs here; a caller can record the planned Agent before mutation.
    pub fn prepare(&self, pair: ParsedPair) -> Result<PreparedRun, ErrorKind> {
        let namespace = self.http.namespace();
        let name = format!("bridge-run-{}", Uuid::new_v4().simple());
        let spec = pair.agent().spec(self.http.provider());
        let mut creates = vec![json!({"name":name,"namespace":namespace,"spec":spec})];
        for case in pair.cases() {
            let task = format!("bridge-task-{}", Uuid::new_v4().simple());
            let spec = json!({"type":"ai","agentRef":{"name":name},"prompt":case.input(),"timeout":"120s","retryPolicy":{"maxRetries":0}});
            creates.push(json!({"name":task,"namespace":namespace,"spec":spec}));
        }
        if let Some(cap) = self.http.max_create_body_bytes() {
            for body in &creates {
                if serde_json::to_vec(body)
                    .map_err(|_| ErrorKind::Source)?
                    .len()
                    > cap
                {
                    return Err(ErrorKind::Source);
                }
            }
        }
        Ok(PreparedRun {
            handle: RunHandle {
                pair,
                target: self.id,
                agent: None,
                tasks: Vec::new(),
            },
            creates,
        })
    }
    /// Legacy local caller: no new Stop UI. Failed preparation has no mutations.
    pub async fn submit(&self, pair: ParsedPair) -> Result<RunHandle, RunError> {
        let prepared = self.prepare(pair.clone()).map_err(|kind| RunError {
            kind,
            handle: Box::new(RunHandle {
                pair,
                target: self.id,
                agent: None,
                tasks: Vec::new(),
            }),
            pending: None,
        })?;
        let (_sender, stop) = watch::channel(false);
        self.submit_prepared(prepared, stop).await
    }
    pub async fn close(&self) {
        self.http.close().await;
    }
    /// Stop bars new requests and drops asynchronous in-flight waits, not remote
    /// Tasks. An uncertain write retains its attempted identity; never replay it.
    pub async fn submit_prepared(
        &self,
        prepared: PreparedRun,
        mut stop: watch::Receiver<bool>,
    ) -> Result<RunHandle, RunError> {
        let PreparedRun {
            mut handle,
            creates,
        } = prepared;
        let mut pending = None;
        let result = self
            .submit_inner(&mut handle, &creates, &mut pending, &mut stop)
            .await;
        if result == Err(ErrorKind::Stopped) {
            self.close().await;
        }
        match result {
            Ok(()) => Ok(handle),
            Err(kind) => Err(RunError {
                kind,
                handle: Box::new(handle),
                pending,
            }),
        }
    }
    async fn submit_inner(
        &self,
        handle: &mut RunHandle,
        creates: &[Value],
        pending: &mut Option<PendingMutation>,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<(), ErrorKind> {
        if handle.target != self.id {
            return Err(ErrorKind::Configuration);
        }
        let deadline = self.http.deadline();
        for (index, body) in creates.iter().enumerate() {
            check_stop(stop)?;
            let (path, kind) = if index == 0 {
                ("agents", "Agent")
            } else {
                ("tasks", "Task")
            };
            let name = body["name"].as_str().unwrap();
            let request = self.http.request(path, None, Some(body), deadline);
            let value = stoppable(stop, async {
                // Do not label a request uncertain when Stop prevented its poll.
                *pending = Some(PendingMutation {
                    kind,
                    name: name.into(),
                });
                request.await
            })
            .await?;
            let resource =
                ResourceId::acknowledge(&value, &body["spec"], name, self.http.namespace(), kind)?;
            if index == 0 {
                handle.agent = Some(resource);
            } else {
                handle.tasks.push(resource);
            }
            *pending = None;
        }
        // Also report Stop during the final synchronous response, with its receipt.
        check_stop(stop)
    }
    /// GET-only, finite observation. Timeout/drop is not remote cancellation.
    /// Partial receipts can read already acknowledged tasks without replaying writes.
    pub async fn observe(&self, handle: &RunHandle) -> Result<RunReport, RunError> {
        let (_sender, stop) = watch::channel(false);
        self.observe_stoppable(handle, stop).await
    }
    pub async fn observe_stoppable(
        &self,
        handle: &RunHandle,
        mut stop: watch::Receiver<bool>,
    ) -> Result<RunReport, RunError> {
        let result = self.observe_inner(handle, &mut stop).await;
        if matches!(result, Err(ErrorKind::Stopped)) {
            self.close().await;
        }
        result.map_err(|kind| RunError {
            kind,
            handle: Box::new(handle.clone()),
            pending: None,
        })
    }
    async fn checked(
        &self,
        resource: &ResourceId,
        deadline: Instant,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Value, ErrorKind> {
        let path = resource.path();
        let value = stoppable(stop, self.http.request(&path, None, None, deadline)).await?;
        resource.unchanged(&value)?;
        Ok(value)
    }
    async fn observe_inner(
        &self,
        handle: &RunHandle,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<RunReport, ErrorKind> {
        if handle.target != self.id {
            return Err(ErrorKind::Configuration);
        }
        let agent = handle.agent.as_ref().ok_or(ErrorKind::Identity)?;
        let deadline = self.http.deadline();
        let mut cases = Vec::new();
        for (case, task) in handle.pair.cases().iter().zip(&handle.tasks) {
            let mut terminal_rounds = 0;
            loop {
                self.checked(agent, deadline, stop).await?;
                let value = self.checked(task, deadline, stop).await?;
                let phase = value["status"]["phase"].as_str().unwrap_or("");
                if matches!(phase, "Succeeded" | "Failed" | "Cancelled") {
                    terminal_rounds += 1;
                    let path = format!("{}/result", task.path());
                    let result = optional_evidence(
                        stoppable(stop, self.http.request(&path, None, None, deadline)).await,
                    )?;
                    let answer = result
                        .as_ref()
                        .and_then(|v| v["result"].as_str())
                        .map(str::to_owned);
                    // Event recording is best effort. Any untrusted/unavailable drain
                    // loses tool proof, never turns missing evidence into a failure.
                    let tools = optional_evidence(self.tools(task, agent, deadline, stop).await)?
                        .unwrap_or_default();
                    self.checked(task, deadline, stop).await?;
                    self.checked(agent, deadline, stop).await?;
                    let assertions = case.evaluate(answer.as_deref(), &tools);
                    let verdict = if phase == "Succeeded" {
                        aggregate(assertions.iter().map(|r| r.verdict))
                    } else {
                        Verdict::Failed
                    };
                    if verdict != Verdict::Unknown || terminal_rounds == 3 {
                        cases.push(CaseReport {
                            id: case.id.clone(),
                            question: case.input().into(),
                            phase: phase.into(),
                            answer,
                            completed_tools: tools,
                            assertions,
                            verdict,
                        });
                        break;
                    }
                } else if !matches!(
                    phase,
                    "" | "Pending" | "Running" | "Finalizing" | "Scheduled"
                ) {
                    return Err(ErrorKind::Response);
                }
                stoppable(stop, self.http.pause(deadline)).await?;
            }
        }
        self.checked(agent, deadline, stop).await?;
        let verdict = if handle.tasks.len() == handle.pair.cases().len() {
            aggregate(cases.iter().map(|c| c.verdict))
        } else {
            Verdict::Unknown
        };
        check_stop(stop)?;
        Ok(RunReport { cases, verdict })
    }
    async fn tools(
        &self,
        task: &ResourceId,
        agent: &ResourceId,
        deadline: Instant,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<Vec<String>, ErrorKind> {
        let mut after = 0;
        let mut tools = Vec::new();
        for _ in 0..16 {
            let path = format!("{}/events", task.path());
            let value =
                stoppable(stop, self.http.request(&path, Some(after), None, deadline)).await?;
            let page: EventPage = serde_json::from_value(value).map_err(|_| ErrorKind::Response)?;
            if page.namespace != task.namespace
                || page.stream_type != "task"
                || page.stream_id != task.name
                || page.after_seq != after
                || page.latest_seq < after
                || page.events.len() > 1000
            {
                return Err(ErrorKind::Identity);
            }
            for event in page.events {
                if event.seq != after + 1
                    || event.seq > page.latest_seq
                    || event.namespace != task.namespace
                    || event.stream_type != "task"
                    || event.stream_id != task.name
                    || event
                        .task_name
                        .as_ref()
                        .is_some_and(|s| !s.is_empty() && s != &task.name)
                    || event
                        .agent_name
                        .as_ref()
                        .is_some_and(|s| !s.is_empty() && s != &agent.name)
                {
                    return Err(ErrorKind::Identity);
                }
                after = event.seq;
                if event.kind == "ToolCallCompleted"
                    && !event.tool_name.trim().is_empty()
                    && !event.tool_call_id.trim().is_empty()
                {
                    tools.push(event.tool_name);
                }
            }
            if after == page.latest_seq {
                return Ok(tools);
            }
            if after == page.after_seq {
                return Err(ErrorKind::Response);
            }
        }
        Err(ErrorKind::Response)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventPage {
    namespace: String,
    stream_type: String,
    #[serde(rename = "streamID")]
    stream_id: String,
    after_seq: u64,
    latest_seq: u64,
    events: Vec<Event>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    namespace: String,
    stream_type: String,
    #[serde(rename = "streamID")]
    stream_id: String,
    seq: u64,
    #[serde(rename = "type")]
    kind: String,
    task_name: Option<String>,
    agent_name: Option<String>,
    #[serde(default)]
    tool_name: String,
    #[serde(default, rename = "toolCallID")]
    tool_call_id: String,
}
