//! Native, memory-only conversations. No renderer paths, specs or transport authority.
use crate::{
    agent_folder::{AgentFolder, DocumentId},
    engine::run::{ParsedAgent, ResourceId},
    local_vm::{LocalVm, LocalVmStatus},
    runtime::types::RuntimeError,
};
use bridge_runtime_protocol::{
    ChatMethod, ChatRequest, ModelConnection, ModelPhase, OrkaReply, Uuid, MAX_ORKA_BODY,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{watch, OwnedMutexGuard},
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatState {
    Ready,
    Preparing,
    Working,
    WaitingForInput,
    WaitingForSession,
    ObservationLost,
    RequestingStop,
    StopRequested,
    StopUnconfirmed,
    ContextUnavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatError {
    InvalidMessage,
    Busy,
    Source,
    UnsupportedTool,
    Model,
    Connection,
    Identity,
    Response,
    ContextUnavailable,
    WorkspaceChanged,
    Interrupted,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolObservation {
    pub name: String,
    pub call_id: String,
    pub phase: String,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurn {
    pub message: String,
    pub answer: Option<String>,
    pub phase: String,
    pub tools: Vec<ToolObservation>,
    pub context_retained: bool,
    pub termination_unconfirmed: bool,
    pub stop_unconfirmed: bool,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct ChatVersion {
    pub turns: Vec<ChatTurn>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSnapshot {
    pub document_id: DocumentId,
    pub state: ChatState,
    pub busy: bool,
    /// Local admission only; Send also checks the VM and saved source natively.
    pub can_send: bool,
    pub versions: Vec<ChatVersion>,
    pub error: Option<ChatError>,
}
struct Entry {
    view: ChatSnapshot,
    context: Option<Context>,
    lease: Option<Arc<OwnedMutexGuard<()>>>,
    stop: watch::Sender<bool>,
}
impl Entry {
    fn new(id: DocumentId) -> Self {
        Self {
            view: ChatSnapshot {
                document_id: id,
                state: ChatState::Ready,
                busy: false,
                can_send: true,
                versions: vec![],
                error: None,
            },
            context: None,
            lease: None,
            stop: watch::channel(false).0,
        }
    }
}
impl Entry {
    fn finish(&mut self, safe: bool, success: bool) {
        // Publish completion under the same lock as Stop admission.
        if success && safe {
            self.view.state = ChatState::Ready;
        }
        self.view.busy = false;
        self.view.can_send = safe && self.view.state == ChatState::Ready;
    }
}
#[derive(Clone, PartialEq)]
struct Binding {
    installation: Uuid,
    boot: Uuid,
    connection: ModelConnection,
}
struct Planned {
    body: Value,
    receipt: Option<ResourceId>,
    sent: bool,
}
impl Planned {
    fn new(prefix: &str, spec: Value) -> Self {
        Self {
            body: json!({"name":format!("{prefix}{}",Uuid::new_v4().simple()),"namespace":"orka-system","spec":spec}),
            receipt: None,
            sent: false,
        }
    }
    fn name(&self) -> &str {
        self.body["name"].as_str().unwrap()
    }
    fn request(&self, path: &str) -> ChatRequest {
        request(ChatMethod::Post, path, Some(self.body.clone()), None)
    }
}
struct Context {
    source: Vec<u8>,
    binding: Binding,
    agent: Planned,
    task: Planned,
    session: String,
    cursor: u64,
    deleted: bool,
    stopping: bool,
}
pub struct ChatOwner(
    Mutex<HashMap<DocumentId, Entry>>,
    tokio::runtime::Handle,
    AtomicU64,
);
impl ChatOwner {
    pub fn new(runtime: tokio::runtime::Handle) -> Arc<Self> {
        Arc::new(Self(Mutex::default(), runtime, AtomicU64::new(0)))
    }
    /// Forget local conversations without sending cleanup to either workspace.
    pub fn retire_for_workspace_replacement(&self) {
        let mut entries = self.0.lock().unwrap();
        for entry in entries.values() {
            entry.stop.send_replace(true);
        }
        self.2.fetch_add(1, Ordering::Relaxed);
        entries.clear();
    }
    pub fn status(&self, id: DocumentId) -> ChatSnapshot {
        self.0
            .lock()
            .unwrap()
            .get(&id)
            .map(|e| e.view.clone())
            .unwrap_or_else(|| Entry::new(id).view)
    }
    pub fn send(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        folder: Arc<AgentFolder>,
        message: String,
    ) -> Result<ChatSnapshot, ChatError> {
        if message.trim().is_empty() || message.len() > MAX_ORKA_BODY {
            return Err(ChatError::InvalidMessage);
        }
        let id = folder.document_id();
        let mut entries = self.0.lock().unwrap();
        // Retirement shares this lock: capture the lease and launch epoch together,
        // and admit nothing in the gap before the replacement VM is ready.
        let lease = vm.try_authoring().ok_or(ChatError::Busy)?;
        if vm.status() != LocalVmStatus::Ready || !vm.authoring_current(&lease) {
            return Err(ChatError::Busy);
        }
        let e = entries.entry(id).or_insert_with(|| Entry::new(id));
        if e.view.busy || e.lease.is_some() {
            return Err(ChatError::Busy);
        }
        if e.view.state == ChatState::ContextUnavailable {
            return Err(ChatError::ContextUnavailable);
        }
        e.lease = Some(Arc::new(lease));
        e.view.busy = true;
        e.view.can_send = false;
        e.view.state = ChatState::Preparing;
        e.view.error = None;
        e.stop = watch::channel(false).0;
        self.launch(vm, id, e, Some((folder, message)));
        Ok(e.view.clone())
    }
    pub fn reconnect(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
    ) -> Result<ChatSnapshot, ChatError> {
        self.resume(vm, id, false)
    }
    pub fn stop(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
    ) -> Result<ChatSnapshot, ChatError> {
        self.resume(vm, id, true)
    }
    fn resume(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
        stop: bool,
    ) -> Result<ChatSnapshot, ChatError> {
        let mut entries = self.0.lock().unwrap();
        let e = entries.get_mut(&id).ok_or(ChatError::ContextUnavailable)?;
        if e.view.busy {
            if stop {
                e.stop.send_replace(true);
                e.view.state = ChatState::RequestingStop;
            }
            return Ok(e.view.clone());
        }
        if e.lease.is_none() {
            return Err(ChatError::ContextUnavailable);
        }
        e.stop = watch::channel(stop).0;
        e.view.busy = true;
        e.view.error = None;
        if stop {
            e.view.state = ChatState::RequestingStop;
        }
        self.launch(vm, id, e, None);
        Ok(e.view.clone())
    }
    pub fn new_conversation(&self, id: DocumentId) -> Result<ChatSnapshot, ChatError> {
        let mut entries = self.0.lock().unwrap();
        let e = entries.entry(id).or_insert_with(|| Entry::new(id));
        if e.view.busy || e.lease.is_some() {
            return Err(ChatError::Busy);
        }
        e.context = None;
        e.view.state = ChatState::Ready;
        e.view.can_send = true;
        e.view.error = None;
        Ok(e.view.clone())
    }
    fn edit(&self, id: DocumentId, epoch: u64, f: impl FnOnce(&mut ChatSnapshot)) {
        let mut entries = self.0.lock().unwrap();
        if self.2.load(Ordering::Relaxed) == epoch {
            if let Some(entry) = entries.get_mut(&id) {
                f(&mut entry.view);
            }
        }
    }
    fn launch(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
        entry: &mut Entry,
        send: Option<(Arc<AgentFolder>, String)>,
    ) {
        let owner = self.clone();
        let mut worker = Worker {
            owner: owner.clone(),
            vm,
            id,
            epoch: self.2.load(Ordering::Relaxed),
            context: entry.context.take(),
            lease: entry.lease.take().unwrap(),
            stop: entry.stop.subscribe(),
            deadline: Instant::now() + Duration::from_secs(150),
            stopping: false,
            safe: send.is_some(),
            preflight: send.is_some(),
        };
        self.1.spawn(async move {
            let mut result = worker.run(send).await;
            if result == Err(ChatError::Interrupted) && !worker.preflight {
                result = worker.stop_task().await;
            }
            let mut entries = owner.0.lock().unwrap();
            // Retirement can remove and recreate the same document while this
            // detached worker is awaiting a response. Never restore its old lease.
            if owner.2.load(Ordering::Relaxed) != worker.epoch {
                return;
            }
            let Some(e) = entries.get_mut(&id) else {
                return;
            };
            if let Err(error) = result {
                e.view.error = Some(error);
                e.view.state = if worker.preflight
                    && matches!(
                        error,
                        ChatError::InvalidMessage
                            | ChatError::Source
                            | ChatError::UnsupportedTool
                            | ChatError::Model
                            | ChatError::Connection
                            | ChatError::Interrupted
                    ) {
                    ChatState::Ready
                } else if worker.safe || error == ChatError::WorkspaceChanged {
                    ChatState::ContextUnavailable
                } else if worker.stopping
                    || *worker.stop.borrow()
                    || worker.context.as_ref().is_some_and(|c| c.stopping)
                {
                    ChatState::StopUnconfirmed
                } else {
                    ChatState::ObservationLost
                };
            }
            e.context = worker.context;
            e.lease = if worker.safe || result == Err(ChatError::WorkspaceChanged) {
                drop(worker.lease);
                None
            } else {
                Some(worker.lease)
            };
            e.finish(worker.safe, result.is_ok());
        });
    }
}
struct Worker {
    owner: Arc<ChatOwner>,
    vm: Arc<LocalVm>,
    id: DocumentId,
    epoch: u64,
    context: Option<Context>,
    lease: Arc<OwnedMutexGuard<()>>,
    stop: watch::Receiver<bool>,
    deadline: Instant,
    stopping: bool,
    safe: bool,
    preflight: bool,
}
impl Worker {
    fn state(&self, state: ChatState) {
        self.owner.edit(self.id, self.epoch, |s| s.state = state);
    }
    fn turn(&self, f: impl FnOnce(&mut ChatTurn)) {
        self.owner.edit(self.id, self.epoch, |s| {
            if let Some(t) = s.versions.last_mut().and_then(|v| v.turns.last_mut()) {
                f(t);
            }
        });
    }
    async fn pause(&self) -> Result<(), ChatError> {
        if Instant::now() >= self.deadline {
            return Err(ChatError::Connection);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(())
    }
    async fn borrow<T: Send + 'static>(
        &self,
        retained: bool,
        work: impl Fn(Uuid, &mut crate::runtime::client::VmClient) -> Result<T, ChatError>
            + Send
            + Sync
            + 'static,
    ) -> Result<T, ChatError> {
        let work = Arc::new(work);
        loop {
            if !self.stopping && *self.stop.borrow() {
                return Err(ChatError::Interrupted);
            }
            if Instant::now() >= self.deadline {
                return Err(ChatError::Connection);
            }
            let (work, vm, lease) = (work.clone(), self.vm.clone(), self.lease.clone());
            let (owner, epoch) = (self.owner.clone(), self.epoch);
            let binding = self
                .context
                .as_ref()
                .filter(|_| retained)
                .map(|c| c.binding.clone());
            match self
                .vm
                .with_client(move |id, client| {
                    // Check admission *inside* the serialized borrow: startup resets its epoch.
                    if owner.2.load(Ordering::Relaxed) != epoch
                        || !vm.authoring_current(&lease)
                        || binding
                            .is_some_and(|b| id != b.installation || client.boot_nonce() != b.boot)
                    {
                        return Err(ChatError::WorkspaceChanged);
                    }
                    work(id, client)
                })
                .await
            {
                Err(RuntimeError::Busy) => self.pause().await?,
                Ok(result) => return result,
                Err(_) => return Err(ChatError::Connection),
            }
        }
    }
    async fn binding(&self, retained: bool) -> Result<Binding, ChatError> {
        let previous = self
            .context
            .as_ref()
            .filter(|_| retained)
            .map(|c| c.binding.clone());
        self.borrow(retained, move |installation, client| {
            if let Some(binding) = &previous {
                return Ok(binding.clone());
            }
            let model = client.model_status().map_err(|_| ChatError::Connection)?;
            if model.phase != ModelPhase::Ready {
                return Err(ChatError::Model);
            }
            Ok(Binding {
                installation,
                boot: client.boot_nonce(),
                connection: model.connection.ok_or(ChatError::Model)?,
            })
        })
        .await
    }
    async fn request(&self, r: ChatRequest) -> Result<OrkaReply, ChatError> {
        self.borrow(true, move |_, client| {
            client
                .chat_request(r.clone())
                .map_err(|_| ChatError::Connection)
        })
        .await
    }
    async fn get(&self, path: &str) -> Result<OrkaReply, ChatError> {
        self.request(request(ChatMethod::Get, path, None, None))
            .await
    }
    async fn checked(&self, id: &ResourceId) -> Result<Option<Value>, ChatError> {
        let reply = self.get(&id.path()).await?;
        match reply.status {
            200 => {
                id.unchanged(&reply.body).map_err(|_| ChatError::Identity)?;
                Ok(Some(reply.body))
            }
            404 => Ok(None),
            _ => Err(ChatError::Response),
        }
    }
    async fn run(&mut self, send: Option<(Arc<AgentFolder>, String)>) -> Result<(), ChatError> {
        // Identify the boot before sending *any* retained name, including during reconnect.
        let binding = self.binding(send.is_none()).await?;
        if let Some((folder, message)) = send {
            let source = tokio::task::spawn_blocking(move || folder.capture_agent())
                .await
                .map_err(|_| ChatError::Source)?
                .map_err(|_| ChatError::Source)?;
            let parsed = ParsedAgent::parse(&source).map_err(|_| ChatError::Source)?;
            if parsed.has_unknown_local_builtin() {
                return Err(ChatError::Source);
            }
            if parsed.has_unsupported_local_tool() {
                return Err(ChatError::UnsupportedTool);
            }
            let fresh = self
                .context
                .as_ref()
                .is_none_or(|c| c.source != source || c.binding != binding);
            let session = if fresh {
                format!("bridge-chat-{}", Uuid::new_v4().simple())
            } else {
                self.context.as_ref().unwrap().session.clone()
            };
            if !fresh && !self.session(self.context.as_ref().unwrap().deleted).await? {
                return Err(ChatError::ContextUnavailable);
            }
            let agent = if fresh {
                Planned::new(
                    "bridge-chat-agent-",
                    parsed.spec(&binding.connection.selection.provider),
                )
            } else {
                let c = self.context.as_ref().unwrap();
                let receipt = c.agent.receipt.as_ref().ok_or(ChatError::Identity)?;
                self.checked(receipt).await?.ok_or(ChatError::Identity)?;
                Planned {
                    body: c.agent.body.clone(),
                    receipt: Some(receipt.clone()),
                    sent: true,
                }
            };
            let mut task_spec = json!({"type":"ai","agentRef":{"name":agent.name()},"prompt":message,"timeout":"120s","retryPolicy":{"maxRetries":0},"sessionRef":{"name":session,"append":true,"maxMessages":50}});
            // Go omits false Create; only the first turn may create a Session.
            if fresh {
                task_spec["sessionRef"]["create"] = json!(true);
            }
            let task = Planned::new("bridge-chat-task-", task_spec);
            // Validate both *whole* create bodies before the first mutation.
            agent
                .request("agents")
                .validate()
                .map_err(|_| ChatError::Source)?;
            task.request("tasks")
                .validate()
                .map_err(|_| ChatError::InvalidMessage)?;
            // Preflight rejection must leave the preceding conversation usable.
            self.preflight = false;
            self.context = Some(Context {
                source,
                binding,
                agent,
                task,
                session,
                cursor: 0,
                deleted: false,
                stopping: false,
            });
            self.owner.edit(self.id, self.epoch, |s| {
                if fresh {
                    s.versions.push(ChatVersion::default());
                }
                s.versions.last_mut().unwrap().turns.push(ChatTurn {
                    message,
                    ..ChatTurn::default()
                });
            });
            self.adopt(true, true).await?;
            self.safe = true;
            self.adopt(false, true).await?;
        } else {
            if self.context.as_ref().is_some_and(|c| c.stopping) || *self.stop.borrow() {
                return self.stop_task().await;
            }
            self.adopt(true, false).await?;
            if !self.context.as_ref().unwrap().task.sent {
                self.safe = true;
                return Err(ChatError::ContextUnavailable);
            }
            self.adopt(false, false).await?;
        }
        self.observe().await
    }
    async fn adopt(&mut self, agent: bool, create: bool) -> Result<(), ChatError> {
        let c = self.context.as_ref().unwrap();
        let p = if agent { &c.agent } else { &c.task };
        if p.receipt.is_some() {
            return Ok(());
        }
        let collection = if agent { "agents" } else { "tasks" };
        let body = p.body.clone();
        let name = p.name().to_owned();
        let sent = p.sent;
        if create && !sent {
            let c = self.context.as_mut().unwrap();
            (if agent { &mut c.agent } else { &mut c.task }).sent = true;
            self.safe = agent; // An Agent create alone cannot be executing a turn.
            match self
                .request(request(
                    ChatMethod::Post,
                    collection,
                    Some(body.clone()),
                    None,
                ))
                .await
            {
                Ok(reply) if reply.status == 201 => return self.ack(agent, reply.body),
                Ok(reply)
                    if (400..500).contains(&reply.status)
                        && reply.status != 408
                        && reply.status != 409
                        && reply.status != 429 =>
                {
                    self.safe = true;
                    return Err(ChatError::Response);
                }
                Err(error @ (ChatError::Interrupted | ChatError::WorkspaceChanged)) => {
                    return Err(error)
                }
                _ => {} // A dispatched POST is never retried, even when its reply is lost.
            }
        }
        let reply = self.get(&format!("{collection}/{name}")).await?;
        if reply.status == 404 {
            if !agent {
                let c = self.context.as_mut().unwrap();
                c.deleted = true;
                self.safe = false;
                if c.stopping {
                    self.stopped();
                }
                return Ok(()); // Task absence alone does not establish Session release.
            }
            self.safe = true;
            return Err(ChatError::ContextUnavailable);
        }
        if reply.status != 200 {
            return Err(ChatError::Response);
        }
        self.ack(agent, reply.body)
    }
    fn ack(&mut self, agent: bool, value: Value) -> Result<(), ChatError> {
        let c = self.context.as_mut().unwrap();
        let p = if agent { &mut c.agent } else { &mut c.task };
        p.receipt = Some(
            ResourceId::acknowledge(
                &value,
                &p.body["spec"],
                p.name(),
                "orka-system",
                if agent { "Agent" } else { "Task" },
            )
            .map_err(|_| ChatError::Identity)?,
        );
        Ok(())
    }
    async fn stop_task(&mut self) -> Result<(), ChatError> {
        self.stopping = true;
        if self.safe {
            return Err(ChatError::ContextUnavailable);
        }
        let Some(c) = self.context.as_mut() else {
            self.safe = true;
            return Err(ChatError::ContextUnavailable);
        };
        c.stopping = true;
        if !c.task.sent {
            self.safe = true;
            return Err(ChatError::ContextUnavailable);
        }
        let explicit = *self.stop.borrow();
        if c.task.receipt.is_none() {
            match self.adopt(false, false).await {
                Err(ChatError::ContextUnavailable) => self.context.as_mut().unwrap().deleted = true,
                other => other?,
            }
        }
        self.safe = false;
        if explicit && !self.context.as_ref().unwrap().deleted {
            self.state(ChatState::RequestingStop);
            let task = self
                .context
                .as_ref()
                .unwrap()
                .task
                .receipt
                .as_ref()
                .unwrap()
                .clone();
            if self.checked(&task).await?.is_none() {
                self.context.as_mut().unwrap().deleted = true;
                self.stopped();
            } else {
                let reply = self
                    .request(request(ChatMethod::Delete, &task.path(), None, None))
                    .await;
                if reply
                    .as_ref()
                    .is_ok_and(|r| r.status == 204 || r.status == 404)
                {
                    self.stopped();
                } else {
                    self.turn(|t| t.stop_unconfirmed = true);
                    self.state(ChatState::StopUnconfirmed);
                }
            }
        }
        self.observe().await
    }
    fn stopped(&self) {
        self.turn(|t| {
            t.termination_unconfirmed = true;
            t.stop_unconfirmed = false;
        });
        self.state(ChatState::StopRequested);
    }
    async fn observe(&mut self) -> Result<(), ChatError> {
        loop {
            let c = self.context.as_ref().unwrap();
            let stopping = c.stopping;
            let task = c.task.receipt.clone();
            if !c.deleted {
                let task = task.as_ref().ok_or(ChatError::Identity)?;
                if let Some(value) = self.checked(task).await? {
                    if !stopping {
                        let agent = self
                            .context
                            .as_ref()
                            .unwrap()
                            .agent
                            .receipt
                            .as_ref()
                            .unwrap()
                            .clone();
                        self.checked(&agent).await?.ok_or(ChatError::Identity)?;
                        // A just-created Task has no phase until controller reconciliation.
                        let phase = match value["status"].get("phase") {
                            None => "",
                            Some(phase) => phase.as_str().ok_or(ChatError::Response)?,
                        }
                        .to_owned();
                        self.turn(|t| t.phase = phase.clone());
                        let terminal =
                            matches!(phase.as_str(), "Succeeded" | "Failed" | "Cancelled");
                        self.state(if phase == "WaitingForInput" || phase == "InputRequired" {
                            ChatState::WaitingForInput
                        } else {
                            ChatState::Working
                        });
                        if terminal {
                            let reply = self.get(&format!("{}/result", task.path())).await?;
                            match reply.status {
                                200 => {
                                    let answer = reply.body["result"]
                                        .as_str()
                                        .ok_or(ChatError::Response)?
                                        .to_owned();
                                    self.turn(|t| t.answer = Some(answer));
                                }
                                404 => {}
                                _ => return Err(ChatError::Response),
                            }
                        }
                        self.events(task).await?;
                        if terminal {
                            self.checked(task).await?.ok_or(ChatError::Identity)?;
                            self.checked(&agent).await?.ok_or(ChatError::Identity)?;
                            self.state(ChatState::WaitingForSession);
                            if self.session(false).await? {
                                return Ok(());
                            }
                        }
                    }
                } else {
                    self.context.as_mut().unwrap().deleted = true;
                    if stopping {
                        self.stopped();
                    }
                }
            }
            if self.context.as_ref().unwrap().deleted && self.session(true).await? {
                return Ok(());
            }
            self.pause().await?;
        }
    }
    async fn events(&mut self, task: &ResourceId) -> Result<(), ChatError> {
        loop {
            let after = self.context.as_ref().unwrap().cursor;
            let reply = self
                .request(request(
                    ChatMethod::Get,
                    &format!("{}/events", task.path()),
                    None,
                    Some(after),
                ))
                .await?;
            let p = &reply.body;
            let latest = p["latestSeq"].as_u64().ok_or(ChatError::Response)?;
            if reply.status != 200
                || p["namespace"] != "orka-system"
                || p["streamType"] != "task"
                || p["streamID"] != task.name
                || p["afterSeq"] != after
                || latest < after
            {
                return Err(ChatError::Identity);
            }
            let events = p["events"]
                .as_array()
                .filter(|e| e.len() <= 1000)
                .ok_or(ChatError::Response)?;
            for event in events {
                let c = self.context.as_mut().unwrap();
                let next = c.cursor.checked_add(1).ok_or(ChatError::Response)?;
                if event["seq"] != next
                    || next > latest
                    || event["namespace"] != "orka-system"
                    || event["streamType"] != "task"
                    || event["streamID"] != task.name
                    || event
                        .get("taskName")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty() && s != task.name)
                    || event
                        .get("agentName")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty() && s != c.agent.name())
                {
                    return Err(ChatError::Identity);
                }
                c.cursor = next;
                if let (Some(name), Some(call), Some(phase)) = (
                    event["toolName"].as_str(),
                    event["toolCallID"].as_str(),
                    event["type"].as_str(),
                ) {
                    if !name.trim().is_empty()
                        && !call.trim().is_empty()
                        && matches!(
                            phase,
                            "ToolCallStarted" | "ToolCallCompleted" | "ToolCallFailed"
                        )
                    {
                        self.turn(|t| {
                            t.tools.push(ToolObservation {
                                name: name.into(),
                                call_id: call.into(),
                                phase: phase.into(),
                            })
                        });
                    }
                }
            }
            if self.context.as_ref().unwrap().cursor == latest {
                return Ok(());
            }
            if self.context.as_ref().unwrap().cursor == after {
                return Err(ChatError::Response);
            }
        }
    }
    async fn session(&mut self, deleted: bool) -> Result<bool, ChatError> {
        let c = self.context.as_ref().unwrap();
        let reply = self.get(&format!("sessions/{}", c.session)).await?;
        if reply.status == 404 && deleted {
            self.safe = true;
            return Err(ChatError::ContextUnavailable);
        }
        if reply.status == 404 {
            return Ok(false);
        }
        if reply.status != 200 {
            return Err(ChatError::Response);
        }
        if reply.body["name"] != c.session || reply.body["namespace"] != "orka-system" {
            return Err(ChatError::Identity);
        }
        if !reply.body["activeTask"]
            .as_str()
            .ok_or(ChatError::Response)?
            .is_empty()
        {
            return Ok(false);
        }
        // Only observed Session release makes malformed context safe to abandon.
        self.safe = true;
        let text = reply.body["transcript"]
            .as_str()
            .ok_or(ChatError::ContextUnavailable)?;
        let uid = c
            .task
            .receipt
            .as_ref()
            .map(|t| t.uid.as_str())
            .unwrap_or("");
        let prefix = format!("task:{uid}:");
        let mut messages = HashMap::new();
        for line in text.lines() {
            let m: TranscriptMessage =
                serde_json::from_str(line).map_err(|_| ChatError::ContextUnavailable)?;
            if m.id.is_empty()
                || !matches!(m.role.as_str(), "user" | "assistant" | "system" | "tool")
                || m.id
                    .strip_prefix(&prefix)
                    .is_some_and(|role| role != m.role)
                || messages.insert(m.id.clone(), m).is_some()
            {
                return Err(ChatError::ContextUnavailable);
            }
        }
        let user = messages
            .get(&format!("{prefix}user"))
            .map(|m| m.content.as_str());
        let assistant = messages
            .get(&format!("{prefix}assistant"))
            .map(|m| m.content.as_str());
        let turn = {
            let entries = self.owner.0.lock().unwrap();
            if self.owner.2.load(Ordering::Relaxed) != self.epoch {
                return Err(ChatError::WorkspaceChanged);
            }
            entries
                .get(&self.id)
                .and_then(|e| e.view.versions.last())
                .and_then(|v| v.turns.last())
                .cloned()
                .ok_or(ChatError::WorkspaceChanged)?
        };
        if user.is_some_and(|u| u != turn.message)
            || assistant.is_some_and(|a| turn.answer.as_ref().is_some_and(|answer| answer != a))
        {
            return Err(ChatError::ContextUnavailable);
        }
        let needs_answer = turn.phase == "Succeeded" && turn.answer.as_deref() != Some("")
            || turn.answer.as_ref().is_some_and(|a| !a.is_empty());
        if !deleted && (user.is_none() || needs_answer && assistant.is_none()) {
            self.safe = false;
            return Ok(false);
        }
        self.turn(|t| {
            t.context_retained = user.is_some();
            if t.answer.is_none() {
                t.answer = assistant.map(str::to_owned);
            }
        });
        if deleted && !c.stopping {
            return Err(ChatError::ContextUnavailable);
        }
        self.state(ChatState::Ready);
        Ok(true)
    }
}
#[derive(Deserialize)]
struct TranscriptMessage {
    id: String,
    role: String,
    content: String,
}
fn request(method: ChatMethod, path: &str, body: Option<Value>, after: Option<u64>) -> ChatRequest {
    ChatRequest {
        method,
        path: path.into(),
        after,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn workspace_retirement_releases_retained_lease_and_signals_stop() {
        let owner = ChatOwner::new(tokio::runtime::Handle::current());
        let vm = LocalVm::unavailable();
        let id = DocumentId::SAMPLE;
        let mut entry = Entry::new(id);
        entry.lease = Some(Arc::new(vm.try_authoring().unwrap()));
        entry.view.versions.push(ChatVersion {
            turns: vec![ChatTurn {
                message: "old workspace".into(),
                ..ChatTurn::default()
            }],
        });
        let stop = entry.stop.subscribe();
        owner.0.lock().unwrap().insert(id, entry);
        assert!(vm.try_authoring().is_none());
        owner.retire_for_workspace_replacement();
        assert!(*stop.borrow());
        assert!(owner.0.lock().unwrap().is_empty());
        assert!(owner.status(id).versions.is_empty());
        assert!(vm.try_authoring().is_some());
    }

    #[tokio::test]
    async fn retired_worker_edits_cannot_touch_a_new_same_document_conversation() {
        let owner = ChatOwner::new(tokio::runtime::Handle::current());
        let vm = LocalVm::unavailable();
        let id = DocumentId::SAMPLE;
        let worker = Worker {
            owner: owner.clone(),
            vm: vm.clone(),
            id,
            epoch: owner.2.load(std::sync::atomic::Ordering::Relaxed),
            context: None,
            lease: Arc::new(vm.try_authoring().unwrap()),
            stop: watch::channel(false).1,
            deadline: Instant::now() + Duration::from_secs(1),
            stopping: false,
            safe: true,
            preflight: true,
        };
        owner.0.lock().unwrap().insert(id, Entry::new(id));
        owner.retire_for_workspace_replacement();
        // Missing entries as well as a recreated document must reject old edits.
        worker.state(ChatState::Working);
        owner.new_conversation(id).unwrap();
        worker.state(ChatState::ObservationLost);
        worker.turn(|_| panic!("a stale turn edit must never run"));
        let fresh = owner.status(id);
        assert_eq!(fresh.state, ChatState::Ready);
        assert!(fresh.versions.is_empty());
        // Current-epoch edits retain their original semantics.
        owner.edit(
            id,
            owner.2.load(std::sync::atomic::Ordering::Relaxed),
            |s| s.state = ChatState::Working,
        );
        assert_eq!(owner.status(id).state, ChatState::Working);
    }

    #[tokio::test]
    async fn retired_worker_finalization_does_not_restore_old_state_or_lease() {
        let owner = ChatOwner::new(tokio::runtime::Handle::current());
        let vm = LocalVm::unavailable();
        let folder = Arc::new(AgentFolder::new(None));
        let id = folder.document_id();
        // Exercise detached finalization without requiring a running VM.
        {
            let mut entries = owner.0.lock().unwrap();
            let entry = entries.entry(id).or_insert_with(|| Entry::new(id));
            entry.lease = Some(Arc::new(vm.try_authoring().unwrap()));
            owner.launch(vm.clone(), id, entry, Some((folder, "old message".into())));
        }
        owner.retire_for_workspace_replacement();
        owner.new_conversation(id).unwrap();
        // Drive the real detached task through its failed old-client completion.
        for _ in 0..100 {
            if vm.try_authoring().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            vm.try_authoring().is_some(),
            "old worker must drop its lease"
        );
        let fresh = owner.status(id);
        assert_eq!(fresh.state, ChatState::Ready);
        assert!(fresh.error.is_none());
        assert!(fresh.can_send && !fresh.busy && fresh.versions.is_empty());
    }

    #[test]
    fn completion_overrides_late_stop_without_erasing_interruption_notice() {
        let mut entry = Entry::new(DocumentId::SAMPLE);
        entry.view.busy = true;
        entry.view.state = ChatState::RequestingStop;
        entry.view.versions.push(ChatVersion {
            turns: vec![ChatTurn {
                termination_unconfirmed: true,
                ..ChatTurn::default()
            }],
        });
        entry.finish(true, true);
        assert_eq!(entry.view.state, ChatState::Ready);
        assert!(entry.view.can_send);
        assert!(!entry.view.busy);
        assert!(entry.view.versions[0].turns[0].termination_unconfirmed);
    }
}
