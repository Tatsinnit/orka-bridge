//! Native remote Send owner. One admitted attempt at a time; credentials, the
//! runner and its handles stay here beyond any IPC waiter. Stop ends admission
//! and observation, never remote Task cancellation.
pub use crate::clusters::preflight::SendCheck;
use crate::{
    agent_folder::{AgentFolder, DocumentId, DocumentRef},
    clusters::{
        commands::{ClusterSession, SendAccess},
        preflight::{CheckRow, CheckStatus},
        ProviderId,
    },
    engine::run::{
        ErrorKind, NativeRunner, ParsedPair, RunCheckpoint, RunReport, RunTransport, Verdict,
    },
    example_run::{receipts, PendingRef, Receipt},
    pair_source::CapturedPair,
    send_summary::{
        AttemptDetails, AttemptSummary, AttemptVerdict, DetailView, SummaryError, SummaryStore,
        TargetUids,
    },
};
use bridge_runtime_protocol::{SnapshotId, Uuid};
use serde::Serialize;
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct SendId(Uuid);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SendState {
    Running,
    Complete,
    Unknown,
}
/// Also the finite error vocabulary: the stage that failed, or Stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Capture,
    Access,
    Preflight,
    Submit,
    Storage,
    Observe,
    Stopped,
    Done,
}
/// Renderer view: IDs, display names, hashes and receipts; never a credential or endpoint.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendView {
    pub document_id: DocumentId,
    pub id: SendId,
    pub target_id: String,
    pub target_name: Option<String>,
    pub provider_id: String,
    pub source: Option<SnapshotId>,
    pub model: Option<String>,
    pub agent_name: Option<String>,
    pub state: SendState,
    pub stage: Stage,
    pub acknowledged: Vec<Receipt>,
    pub pending: Option<PendingRef>,
    pub report: Option<RunReport>,
    pub error: Option<Stage>,
    pub storage_error: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendStatus {
    pub document_id: DocumentId,
    pub view: Option<SendView>,
    pub busy: Option<DocumentRef>,
}
struct Active {
    document: DocumentRef,
    id: SendId,
    stop: watch::Sender<bool>,
    done: watch::Receiver<bool>,
}
impl Active {
    fn halt(&self) -> watch::Receiver<bool> {
        self.stop.send_replace(true);
        self.done.clone()
    }
}
#[derive(Default)]
struct Inner {
    views: HashMap<SendId, SendView>,
    current: HashMap<DocumentId, SendId>,
    active: Option<Active>,
    closed: bool,
}
pub struct SendOwner {
    folder: Arc<AgentFolder>,
    session: ClusterSession,
    summary: SummaryStore,
    inner: Mutex<Inner>,
    history_gate: tokio::sync::Mutex<()>,
}
fn parse(id: &str) -> Option<Uuid> {
    Uuid::parse_str(id).ok()
}
async fn finished(mut done: watch::Receiver<bool>) {
    // A dropped sender also means the job ended.
    let _ = done.wait_for(|d| *d).await;
}
async fn stopped(stop: &mut watch::Receiver<bool>) {
    if stop.wait_for(|s| *s).await.is_err() {
        std::future::pending::<()>().await;
    }
}
/// GET-only or local stages may be dropped on Stop; nothing has been written yet.
async fn until_stop<T, E>(
    stop: &mut watch::Receiver<bool>,
    stage: Stage,
    work: impl Future<Output = Result<T, E>>,
) -> Result<T, Stage> {
    tokio::select! {
        biased;
        _ = stopped(stop) => Err(Stage::Stopped),
        result = work => result.map_err(|_| stage),
    }
}
fn failed(kind: ErrorKind, stage: Stage) -> Stage {
    if kind == ErrorKind::Stopped {
        Stage::Stopped
    } else {
        stage
    }
}
fn capture(folder: &AgentFolder) -> Result<ParsedPair, ()> {
    // Keep the validated directory handles and ownership lock through capture;
    // reopening source()'s pathname would allow an ancestor swap in between.
    let snapshot = folder.capture().map_err(drop)?;
    Ok(CapturedPair::from_snapshot(snapshot)
        .map_err(drop)?
        .into_pair())
}
fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
struct AttemptSeed {
    source: SnapshotId,
    target_display: String,
    target_uids: TargetUids,
    provider: String,
    time: u64,
}
impl SendOwner {
    pub fn new(folder: Arc<AgentFolder>, session: ClusterSession) -> Arc<Self> {
        Self::with_history_root(folder, session, None)
    }
    pub fn with_history_root(
        folder: Arc<AgentFolder>,
        session: ClusterSession,
        app_local_data: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            folder,
            session,
            summary: SummaryStore::new(app_local_data),
            inner: Mutex::default(),
            history_gate: tokio::sync::Mutex::new(()),
        })
    }
    pub fn history(&self) -> Result<Vec<AttemptSummary>, SummaryError> {
        let mut attempts = self.summary.list()?;
        let active = self
            .inner
            .lock()
            .unwrap()
            .active
            .as_ref()
            .map(|active| active.id.0.to_string());
        for attempt in &mut attempts {
            if attempt.verdict == AttemptVerdict::Running
                && Some(attempt.id.as_str()) != active.as_deref()
            {
                attempt.verdict = AttemptVerdict::Unknown;
            }
        }
        attempts.reverse();
        Ok(attempts)
    }
    pub fn history_details(&self, id: &str) -> Result<DetailView, &'static str> {
        let mut view = self.summary.details(id)?;
        let inner = self.inner.lock().unwrap();
        if inner.active.is_some() || inner.closed {
            view.can_check = false;
            view.can_remove = false;
        }
        Ok(view)
    }
    pub async fn history_remove(&self, id: &str) -> Result<DetailView, &'static str> {
        let _gate = self.history_gate.lock().await;
        {
            let inner = self.inner.lock().unwrap();
            if inner.active.is_some() || inner.closed {
                return Err("busy");
            }
        }
        self.summary.remove_details(id)
    }
    pub async fn history_check(self: &Arc<Self>, id: &str) -> Result<DetailView, &'static str> {
        let owner = self.clone();
        let id = id.to_string();
        // Own cleanup and writeback even if the IPC waiter disappears.
        tokio::spawn(async move { owner.check_saved(&id).await })
            .await
            .map_err(|_| "storage")?
    }
    async fn check_saved(&self, id: &str) -> Result<DetailView, &'static str> {
        let _gate = self.history_gate.lock().await;
        {
            let inner = self.inner.lock().unwrap();
            if inner.active.is_some() || inner.closed {
                return Err("busy");
            }
        }
        let (mut summary, details) = self.summary.record(id)?;
        let mut details = details
            .filter(|d| d.view(id).can_check)
            .ok_or("notRecoverable")?;
        let read = async {
            let (access, reader) = self.session.history_access();
            let access = access.ok_or(())?;
            let binding = access.binding();
            let saved = &summary.target_uids;
            if binding.service_namespace != saved.service_namespace
                || binding.service_uid != saved.service_uid
                || binding.deployment_namespace != saved.deployment_namespace
                || binding.deployment_uid != saved.deployment_uid
                || binding.watched_namespace != saved.watched_namespace
            {
                return Err(());
            }
            let kubeconfig = self
                .session
                .read_configuration(reader)
                .await
                .map_err(drop)?;
            let fresh = access.fresh_client(kubeconfig);
            let transport = access.read_transport(&fresh).await.map_err(drop)?;
            let runner = NativeRunner::with_transport(transport);
            Ok(runner
                .recover(
                    &mut details.resources,
                    &details.cases,
                    details.report.clone(),
                )
                .await)
        }
        .await;
        match read {
            Ok(read) => {
                details.report = read.report;
                details.availability = Some(read.availability.into());
                if read.refreshed {
                    details.captured_at = unix_seconds();
                }
                if read.availability == "available"
                    && !details.unresolved()
                    && details
                        .report
                        .as_ref()
                        .is_some_and(|r| r.cases.len() == details.cases.len())
                {
                    details.detail_state = "saved".into();
                }
                summary.verdict = match details.report.as_ref().map(|r| r.verdict) {
                    Some(Verdict::Passed) if !details.unresolved() => AttemptVerdict::Passed,
                    Some(Verdict::Failed) if !details.unresolved() => AttemptVerdict::Failed,
                    _ => AttemptVerdict::Unknown,
                };
            }
            Err(()) => details.availability = Some("unavailable".into()),
        }
        self.summary.checkpoint(summary, details)?;
        self.history_details(id)
    }
    /// Fresh, GET-only preview. Failures are finite display rows, never raw IPC errors.
    pub async fn preflight(&self, target: &str, provider: &str) -> SendCheck {
        self.preflight_document(self.folder.clone(), target, provider)
            .await
    }
    pub async fn preflight_document(
        &self,
        folder: Arc<AgentFolder>,
        target: &str,
        provider: &str,
    ) -> SendCheck {
        use CheckStatus::*;
        let target = parse(target).map(|id| id.to_string()).unwrap_or_default();
        let provider = parse(provider).map(ProviderId);
        let mut check = SendCheck {
            document_id: folder.document_id(),
            source: None,
            target_id: target.clone(),
            provider_id: provider.map(|p| p.0.to_string()).unwrap_or_default(),
            model: None,
            rows: Vec::new(),
            can_send: false,
        };
        let pair = tokio::task::spawn_blocking(move || capture(&folder)).await;
        let Ok(Ok(pair)) = pair else {
            check.rows.push(CheckRow::new(
                "Source",
                Blocked,
                "Bridge cannot read a supported agent file and test file. Check them in Designer.",
            ));
            return check.finish();
        };
        check.source = Some(pair.snapshot().clone());
        check.model = Some(pair.model().into());
        check.rows.push(CheckRow::new(
            "Source",
            Allowed,
            "The current agent file and test file were read.",
        ));
        let rows = async {
            let (access, reader) = self.session.send_access(&target);
            let access = access.ok_or_else(|| {
                CheckRow::new(
                    "Access",
                    CannotCheck,
                    "Reconnect the selected context in Clusters.",
                )
            })?;
            let provider = provider
                .filter(|p| access.providers().iter().any(|v| v.id == *p))
                .ok_or_else(|| {
                    CheckRow::new(
                        "AI connection",
                        Blocked,
                        "Select an AI connection in Clusters.",
                    )
                })?;
            let kubeconfig = self.session.read_configuration(reader).await.map_err(|_| {
                CheckRow::new(
                    "Installation",
                    CannotCheck,
                    "Bridge cannot check the current context configuration.",
                )
            })?;
            let fresh = access.fresh_client(kubeconfig);
            let transport = access.transport(&fresh, provider).await.map_err(|_| {
                CheckRow::new(
                    "Installation",
                    CannotCheck,
                    "Bridge cannot check the selected cluster. Reconnect in Clusters.",
                )
            })?;
            let mut rows = vec![CheckRow::new(
                "Installation",
                Allowed,
                "The selected cluster and required resources were checked.",
            )];
            rows.extend(access.check(&transport, provider, &pair).await);
            transport.close().await;
            Ok::<_, CheckRow>(rows)
        }
        .await;
        match rows {
            Ok(rows) => check.rows.extend(rows),
            Err(row) => check.rows.push(row),
        }
        check.finish()
    }
    /// Admits one job, or observes the running one, without waiting on the
    /// cluster session; access is frozen before Running, capture and rechecks stay inside the job.
    pub async fn start(
        self: &Arc<Self>,
        target_id: &str,
        provider_id: &str,
        expected_source: SnapshotId,
    ) -> SendView {
        self.start_folder(self.folder.clone(), target_id, provider_id, expected_source)
            .await
    }
    pub async fn start_document(
        self: &Arc<Self>,
        folder: Arc<AgentFolder>,
        target: &str,
        provider: &str,
        expected: SnapshotId,
    ) -> SendStatus {
        let id = folder.document_id();
        self.start_folder(folder, target, provider, expected).await;
        self.status_document(id)
    }
    async fn start_folder(
        self: &Arc<Self>,
        folder: Arc<AgentFolder>,
        target_id: &str,
        provider_id: &str,
        expected_source: SnapshotId,
    ) -> SendView {
        let _history = self.history_gate.lock().await;
        let target = parse(target_id).map(|u| u.to_string()).unwrap_or_default();
        let provider = parse(provider_id).map(ProviderId);
        let mut inner = self.inner.lock().unwrap();
        if let Some(active) = &inner.active {
            return inner.views[&active.id].clone();
        }
        let id = SendId(Uuid::new_v4());
        let mut view = SendView {
            document_id: folder.document_id(),
            id,
            target_id: target.clone(),
            target_name: None,
            provider_id: provider.map(|p| p.0.to_string()).unwrap_or_default(),
            source: None,
            model: None,
            agent_name: None,
            state: SendState::Running,
            stage: Stage::Capture,
            acknowledged: Vec::new(),
            pending: None,
            report: None,
            error: None,
            storage_error: false,
        };
        inner.current.insert(folder.document_id(), id);
        if inner.closed {
            (view.state, view.stage, view.error) =
                (SendState::Unknown, Stage::Stopped, Some(Stage::Stopped));
            inner.views.insert(id, view.clone());
            return view;
        }
        let admission = self.session.send_access(&target);
        let (stop, mut stop_rx) = watch::channel(false);
        let (done_tx, done) = watch::channel(false);
        inner.views.insert(id, view.clone());
        inner.active = Some(Active {
            document: folder.document(),
            id,
            stop,
            done,
        });
        drop(inner);
        let owner = self.clone();
        tokio::spawn(async move {
            let result = owner
                .drive(
                    id,
                    folder,
                    admission,
                    provider,
                    &expected_source,
                    &mut stop_rx,
                )
                .await;
            let mut inner = owner.inner.lock().unwrap();
            let view = inner.views.get_mut(&id).unwrap();
            match result {
                Ok(report) => {
                    if view.storage_error {
                        (view.state, view.stage) = (SendState::Unknown, Stage::Done);
                        // Keep actual failed/unknown check details in memory;
                        // a storage error must never become a passing result.
                        view.report = (report.verdict != Verdict::Passed).then_some(report);
                    } else {
                        (view.state, view.stage) = (SendState::Complete, Stage::Done);
                        view.report = Some(report);
                    }
                }
                Err(stage) => {
                    view.state = SendState::Unknown;
                    if stage == Stage::Stopped {
                        view.stage = stage;
                    }
                    view.error = Some(stage);
                }
            }
            // Release admission only after the runner and forward were closed.
            inner.active = None;
            drop(inner);
            let _ = done_tx.send(true);
        });
        view
    }
    pub fn status_document(&self, id: DocumentId) -> SendStatus {
        let inner = self.inner.lock().unwrap();
        SendStatus {
            document_id: id,
            view: inner
                .current
                .get(&id)
                .and_then(|job| inner.views.get(job))
                .cloned(),
            busy: inner.active.as_ref().map(|job| job.document.clone()),
        }
    }
    pub async fn stop_document(
        &self,
        document: DocumentId,
        id: &str,
    ) -> Result<SendStatus, &'static str> {
        if !self
            .status(Some(id))
            .is_some_and(|view| view.document_id == document)
        {
            return Err("The Send does not belong to this agent.");
        }
        self.stop(id).await;
        Ok(self.status_document(document))
    }
    pub fn status(&self, id: Option<&str>) -> Option<SendView> {
        let inner = self.inner.lock().unwrap();
        let id = match id {
            Some(id) => SendId(parse(id)?),
            None => *inner.current.get(&DocumentId::SAMPLE)?,
        };
        inner.views.get(&id).cloned()
    }
    /// Signals before the first await, so a lost IPC waiter still stops the job.
    pub async fn stop(&self, id: &str) -> Option<SendView> {
        let id = SendId(parse(id)?);
        let done = {
            let inner = self.inner.lock().unwrap();
            inner.views.get(&id)?;
            inner
                .active
                .as_ref()
                .filter(|a| a.id == id)
                .map(Active::halt)
        };
        if let Some(done) = done {
            finished(done).await;
        }
        self.inner.lock().unwrap().views.get(&id).cloned()
    }
    /// App exit: refuse new admissions, stop the current job and await its cleanup.
    pub async fn shutdown(&self) {
        let done = {
            let mut inner = self.inner.lock().unwrap();
            inner.closed = true;
            inner.active.as_ref().map(Active::halt)
        };
        if let Some(done) = done {
            finished(done).await;
        }
        // Historical reads own their forward and durable writeback under this gate.
        let _history = self.history_gate.lock().await;
    }
    fn update(&self, id: SendId, change: impl FnOnce(&mut SendView)) {
        change(self.inner.lock().unwrap().views.get_mut(&id).unwrap());
    }
    async fn drive(
        &self,
        id: SendId,
        folder: Arc<AgentFolder>,
        admission: SendAccess,
        provider: Option<ProviderId>,
        expected_source: &SnapshotId,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<RunReport, Stage> {
        // Freeze the pair once, before any network wait; later edits never reach this attempt.
        let pair = until_stop(stop, Stage::Capture, async move {
            tokio::task::spawn_blocking(move || capture(&folder))
                .await
                .map_err(drop)?
        })
        .await?;
        if pair.snapshot() != expected_source {
            return Err(Stage::Capture);
        }
        self.update(id, |v| {
            v.source = Some(pair.snapshot().clone());
            v.model = Some(pair.model().into());
            v.stage = Stage::Access;
        });
        // Admission already froze this access before the first Running response.
        let (access, reader) = admission;
        if let Some(access) = &access {
            self.update(id, |v| {
                v.target_name = Some(access.binding().context_name.clone())
            });
        }
        let (access, provider) = access
            .zip(provider)
            .filter(|(a, p)| a.providers().iter().any(|v| v.id == *p))
            .ok_or(Stage::Access)?;
        let provider_name = access
            .providers()
            .iter()
            .find(|v| v.id == provider)
            .map(|v| v.name.clone())
            .ok_or(Stage::Access)?;
        let kubeconfig =
            until_stop(stop, Stage::Access, self.session.read_configuration(reader)).await?;
        let fresh = access.fresh_client(kubeconfig);
        self.update(id, |v| v.stage = Stage::Preflight);
        // Revalidates the fresh kubeconfig binding. Not raced with Stop, so the
        // one opened forward is always explicitly closed (and joined) below,
        // including when Stop drops a recheck GET on it.
        let transport = access
            .transport(&fresh, provider)
            .await
            .map_err(|_| Stage::Preflight)?;
        let checked = until_stop(stop, Stage::Preflight, async {
            Ok::<_, ()>(access.check(&transport, provider, &pair).await)
        })
        .await;
        let runner = NativeRunner::with_transport(transport);
        let binding = access.binding();
        let seed = AttemptSeed {
            source: pair.snapshot().clone(),
            target_display: format!("{} · {}", binding.context_name, binding.watched_namespace),
            target_uids: TargetUids {
                service_namespace: binding.service_namespace.clone(),
                service_uid: binding.service_uid.clone(),
                deployment_namespace: binding.deployment_namespace.clone(),
                deployment_uid: binding.deployment_uid.clone(),
                watched_namespace: binding.watched_namespace.clone(),
            },
            provider: provider_name,
            time: unix_seconds(),
        };
        let result = match checked {
            Ok(rows) if rows.iter().all(CheckRow::permits) => {
                self.run(id, &runner, pair, seed, stop).await
            }
            Ok(_) => Err(Stage::Preflight),
            Err(stage) => Err(stage),
        };
        runner.close().await;
        result
    }
    async fn run(
        &self,
        id: SendId,
        runner: &NativeRunner<'_>,
        pair: ParsedPair,
        seed: AttemptSeed,
        stop: &watch::Receiver<bool>,
    ) -> Result<RunReport, Stage> {
        if *stop.borrow() {
            return Err(Stage::Stopped);
        }
        let mut details = AttemptDetails {
            document_id: Some(
                self.inner.lock().unwrap().views[&id]
                    .document_id
                    .0
                    .to_string(),
            ),
            model: Some(pair.model().into()),
            captured_at: seed.time,
            cases: pair.cases().to_vec(),
            resources: Vec::new(),
            report: None,
            detail_state: "saved".into(),
            availability: None,
        };
        let prepared = runner.prepare(pair).map_err(|_| Stage::Submit)?;
        let planned_agent_name = prepared.agent_name().to_string();
        self.update(id, |v| {
            v.agent_name = Some(planned_agent_name.clone());
            v.stage = Stage::Submit;
        });
        let mut summary = AttemptSummary {
            id: id.0.to_string(),
            source: seed.source,
            target_display: seed.target_display,
            target_uids: seed.target_uids,
            provider: seed.provider,
            planned_agent_name,
            agent_uid: None,
            verdict: AttemptVerdict::Running,
            time: seed.time,
        };
        let initial = self.summary.checkpoint(summary.clone(), details.clone());
        if initial.is_err() {
            self.update(id, |v| v.storage_error = true);
            return Err(Stage::Storage);
        }
        let mut checkpoint = |event| {
            match event {
                RunCheckpoint::Before(resource) => details.resources.push(resource),
                RunCheckpoint::Acknowledged(resource) => {
                    if resource.kind == "Agent" {
                        summary.agent_uid = resource.uid.clone();
                    }
                    let old = details
                        .resources
                        .iter_mut()
                        .find(|r| r.name == resource.name)
                        .unwrap();
                    *old = resource;
                }
                RunCheckpoint::Case(index, report) => {
                    if let Some(task) = details
                        .resources
                        .iter_mut()
                        .filter(|r| r.kind == "Task")
                        .nth(index)
                    {
                        task.terminal = true;
                    }
                    details
                        .report
                        .get_or_insert(RunReport {
                            cases: Vec::new(),
                            verdict: Verdict::Unknown,
                        })
                        .cases
                        .push(report);
                }
            }
            details.captured_at = unix_seconds();
            if self
                .summary
                .checkpoint(summary.clone(), details.clone())
                .is_ok()
            {
                Ok(())
            } else {
                self.update(id, |v| v.storage_error = true);
                Err(ErrorKind::Storage)
            }
        };
        let handle = match runner
            .submit_checkpointed(prepared, stop.clone(), &mut checkpoint)
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                self.update(id, |v| {
                    v.acknowledged = receipts(&error.handle);
                    v.pending = error.pending.map(|p| PendingRef {
                        kind: p.kind,
                        name: p.name,
                    });
                });
                if error.kind == ErrorKind::Storage {
                    return Err(if error.handle.agent().is_some() {
                        Stage::Submit
                    } else {
                        Stage::Storage
                    });
                }
                summary.verdict = AttemptVerdict::Unknown;
                if self.summary.checkpoint(summary, details).is_err() {
                    self.update(id, |v| v.storage_error = true);
                }
                return Err(failed(error.kind, Stage::Submit));
            }
        };
        self.update(id, |v| {
            v.acknowledged = receipts(&handle);
            v.stage = Stage::Observe;
        });
        let observed = runner
            .observe_checkpointed(&handle, stop.clone(), &mut checkpoint)
            .await;
        let result = match observed {
            Ok(report) => {
                summary.verdict = match report.verdict {
                    Verdict::Passed => AttemptVerdict::Passed,
                    Verdict::Failed => AttemptVerdict::Failed,
                    Verdict::Unknown => AttemptVerdict::Unknown,
                };
                details.report = Some(report.clone());
                Ok(report)
            }
            Err(error) => {
                if error.kind == ErrorKind::Storage {
                    return Err(Stage::Observe);
                }
                summary.verdict = AttemptVerdict::Unknown;
                Err(failed(error.kind, Stage::Observe))
            }
        };
        details.captured_at = unix_seconds();
        if self.summary.checkpoint(summary, details).is_err() {
            self.update(id, |v| v.storage_error = true);
            return Err(Stage::Observe);
        }
        result
    }
}
