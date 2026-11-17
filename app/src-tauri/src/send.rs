//! Native remote Send owner. One admitted attempt at a time; credentials, the
//! runner and its handles stay here beyond any IPC waiter. Shutdown stops local
//! observation; only explicit cancellation intent opens a control transport.
use crate::clusters::preflight::SendAction;
pub use crate::clusters::preflight::SendCheck;
#[path = "send_agent.rs"]
mod agent;
#[path = "send_binding.rs"]
mod binding;
use crate::{
    agent_folder::{AgentFolder, DocumentId, DocumentRef},
    clusters::{
        commands::{ClusterSession, SendAccess},
        preflight::{CheckRow, CheckStatus},
        CheckResult, ClusterError, ProviderId,
    },
    engine::run::{
        Cancellation, CaseReport, ErrorKind, NativeRunner, ParsedPair, RunCheckpoint, RunReport,
        RunTransport, Verdict,
    },
    example_run::{receipts, PendingRef, Receipt},
    pair_source::CapturedPair,
    send_summary::{
        AttemptDetails, AttemptSummary, AttemptVerdict, DetailView, SummaryError, SummaryStore,
        TargetUids,
    },
};
pub use agent::{AgentCheck, AgentFocus};
use binding::Binding;
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
    SubmitRejected,
    Storage,
    Observe,
    Stopped,
    Done,
    UpdateConflict,
    UpdateUnsupported,
    UpdateMissing,
    UpdateDenied,
    UpdateInvalid,
    UpdateUnconfirmed,
    EarlierWork,
    LegacyDestination,
    DestinationMismatch,
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
    pub agent_action: Option<SendAction>,
    pub state: SendState,
    pub stage: Stage,
    pub acknowledged: Vec<Receipt>,
    pub pending: Option<PendingRef>,
    pub report: Option<RunReport>,
    pub error: Option<Stage>,
    pub storage_error: bool,
    pub cancellation: Option<Cancellation>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendStatus {
    pub document_id: DocumentId,
    pub view: Option<SendView>,
    pub busy: Option<DocumentRef>,
}
struct Active {
    cancel_requested: bool,
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
impl Inner {
    /// Called only while holding the same mutex used to accept cancellation.
    fn finish(
        &mut self,
        id: SendId,
        result: &mut Option<Result<RunReport, Stage>>,
        control_finished: bool,
    ) -> bool {
        if !control_finished
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.id == id && active.cancel_requested)
        {
            return false;
        }
        let view = self.views.get_mut(&id).unwrap();
        match result.take().unwrap() {
            Ok(report) => {
                if view.storage_error {
                    (view.state, view.stage) = (SendState::Unknown, Stage::Done);
                    // Keep actual failed/unknown check details in memory;
                    // a storage error must never become a passing result.
                    view.report = (report.verdict != Verdict::Passed).then_some(report);
                } else {
                    (view.state, view.stage) = (
                        if view
                            .cancellation
                            .is_some_and(|c| c != Cancellation::Confirmed)
                        {
                            SendState::Unknown
                        } else {
                            SendState::Complete
                        },
                        Stage::Done,
                    );
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
        self.active = None;

        true
    }
}
#[cfg(test)]
#[path = "send_finalize_tests.rs"]
mod finalize_tests;
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
fn deployment_row(stage: Stage) -> CheckRow {
    let message = match stage {
        Stage::UpdateConflict => "This agent changed outside Bridge. Your files are unchanged.",
        Stage::UpdateUnsupported => "This Orka can't safely update this agent",
        Stage::UpdateMissing => "The linked agent is missing from this cluster.",
        Stage::UpdateDenied => "This cluster identity cannot update this agent.",
        Stage::UpdateInvalid => "Orka rejected this agent spec. Your files are unchanged.",
        Stage::EarlierWork => {
            "Earlier work for this agent is unfinished. Check its status or cancel its tests."
        }
        Stage::DestinationMismatch => "The Orka destination could not be verified. Bridge did not send the Orka credential.",
        Stage::LegacyDestination => "Saved ownership predates destination verification. Bridge cannot reuse it or create a duplicate agent.",
        Stage::Storage => "Bridge cannot read or save the deployment binding.",
        _ => "The agent update outcome is not confirmed. Check status before sending again.",
    };
    CheckRow::new("Agent", CheckStatus::Blocked, message)
}
fn cancel_access_error(error: ClusterError) -> Cancellation {
    if error == ClusterError::Denied {
        Cancellation::Denied
    } else {
        Cancellation::Unconfirmed
    }
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
    pub fn overview(
        &self,
        places: Vec<crate::overview_places::Place>,
    ) -> crate::overview_places::Places {
        let active = self
            .inner
            .lock()
            .unwrap()
            .active
            .as_ref()
            .map(|active| active.id.0.to_string());
        self.summary.overview(places, active.as_deref())
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
            view.can_cancel = false;
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
        let (summary, details) = self.summary.record(id)?;
        let details = details
            .filter(|d| d.view(id).can_check)
            .ok_or("notRecoverable")?;
        self.refresh_saved(id, false, self.session.history_access(), summary, details)
            .await
    }
    async fn refresh_saved(
        &self,
        id: &str,
        cancel: bool,
        admission: SendAccess,
        mut summary: AttemptSummary,
        mut details: AttemptDetails,
    ) -> Result<DetailView, &'static str> {
        if cancel {
            details.cancellation = Some(Cancellation::Cancelling);
        }
        // A saved cancel intent proves submission ended, even if control access failed.
        if details.cancellation.is_some() {
            let task_count = details
                .resources
                .iter()
                .filter(|r| r.kind == "Task")
                .count();
            let report = details.report.get_or_insert(RunReport {
                cases: Vec::new(),
                verdict: Verdict::Unknown,
            });
            for case in details.cases.iter().skip(task_count) {
                if !report.cases.iter().any(|r| r.id == case.id) {
                    report.cases.push(CaseReport {
                        id: case.id.clone(),
                        title: case.title.clone(),
                        question: case.input().into(),
                        phase: "NotRun".into(),
                        answer: None,
                        completed_tools: Vec::new(),
                        assertions: case.unevaluated(),
                        verdict: Verdict::Unknown,
                    });
                }
            }
        }
        if cancel {
            self.summary.checkpoint(summary.clone(), details.clone())?;
        }
        let read = async {
            let (access, reader) = admission;
            let access = access.ok_or(Cancellation::Unconfirmed)?;
            let binding = access.binding();
            let saved = &summary.target_uids;
            if !saved.pinned_matches(&binding::target(binding)) {
                return Err(Cancellation::Unconfirmed);
            }
            let kubeconfig = self
                .session
                .read_configuration(reader)
                .await
                .map_err(|error| match error.result {
                    CheckResult::AuthenticationRejected | CheckResult::PermissionDenied => {
                        Cancellation::Denied
                    }
                    _ => Cancellation::Unconfirmed,
                })?;
            let fresh = access.fresh_client(kubeconfig);
            if cancel {
                let control = access
                    .cancel_transport(&fresh)
                    .await
                    .map_err(cancel_access_error)?;
                let runner = NativeRunner::with_transport(control);
                let mut saved = details.clone();
                let result = runner
                    .cancel_saved(&mut details.resources, &details.cases, &mut |resources| {
                        saved.resources = resources.to_vec();
                        self.summary
                            .checkpoint(summary.clone(), saved.clone())
                            .map(|_| ())
                            .map_err(|_| ErrorKind::Storage)
                    })
                    .await;
                runner.close().await;
                details.cancellation = Some(result.map_err(|_| Cancellation::Unconfirmed)?);
            }
            let transport = access
                .read_transport(&fresh)
                .await
                .map_err(cancel_access_error)?;
            if let Some(mut bound) = self
                .summary
                .attempt_binding(id)
                .map_err(|_| Cancellation::Unconfirmed)?
            {
                if bound.intent.is_some()
                    || details
                        .resources
                        .iter()
                        .find(|r| r.kind == "Agent")
                        .is_none_or(|r| r.uid.is_none())
                {
                    let recovered = async {
                        let value = transport
                            .request(
                                &format!("agents/{}", bound.name),
                                None,
                                None,
                                transport.deadline(),
                            )
                            .await
                            .map_err(|_| Cancellation::Unconfirmed)?;
                        if bound.intent.is_some() {
                            let receipt = bound
                                .reconciled(&value)
                                .map_err(|_| Cancellation::Unconfirmed)?;
                            bound
                                .acknowledge(receipt.value())
                                .map_err(|_| Cancellation::Unconfirmed)?;
                            self.summary
                                .save_binding(bound.clone())
                                .map_err(|_| Cancellation::Unconfirmed)?;
                        }
                        let receipt = bound.receipt().map_err(|_| Cancellation::Unconfirmed)?;
                        receipt
                            .unchanged(&value)
                            .map_err(|_| Cancellation::Unconfirmed)?;
                        if bound
                            .repair_agent(&mut details)
                            .map_err(|_| Cancellation::Unconfirmed)?
                        {
                            summary.agent_uid = Some(receipt.uid);
                        }
                        Ok::<_, Cancellation>(())
                    }
                    .await;
                    if let Err(error) = recovered {
                        transport.close().await;
                        return Err(error);
                    }
                }
            }
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
                // Shared cancellation may be partial. Keep a freshly verified
                // failure; otherwise require complete cluster outcomes below.
                if let Some(report) = &mut details.report {
                    if report.verdict == Verdict::Cancelled {
                        report.verdict = if read.verified_failure {
                            Verdict::Failed
                        } else {
                            Verdict::Unknown
                        };
                    }
                }
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
                    Some(Verdict::Cancelled) if !details.unresolved() => AttemptVerdict::Cancelled,
                    _ => AttemptVerdict::Unknown,
                };
            }
            Err(cancellation) => {
                details.availability = Some("unavailable".into());
                if cancel {
                    details.cancellation = Some(cancellation);
                }
            }
        }
        if details.cancellation.is_some() {
            if details.unresolved() && details.cancellation == Some(Cancellation::Confirmed) {
                details.cancellation = Some(Cancellation::Unconfirmed);
            } else if !details.unresolved() && details.resources.iter().any(|r| r.kind == "Task") {
                details.cancellation = Some(Cancellation::Confirmed);
            }
        }
        // NotRun is incomplete work, except when an actual cancelled Task explains it.
        if let Some(report) = &mut details.report {
            if !details
                .resources
                .iter()
                .any(|r| r.kind == "Task" && !r.terminal)
                && report.cases.iter().any(|r| r.verdict == Verdict::Cancelled)
                && details.cases.len() == report.cases.len()
                && details
                    .cases
                    .iter()
                    .all(|case| report.cases.iter().any(|row| row.id == case.id))
                && report
                    .cases
                    .iter()
                    .all(|r| r.phase == "NotRun" || r.verdict != Verdict::Unknown)
            {
                report.verdict = if report.cases.iter().any(|r| r.verdict == Verdict::Failed) {
                    Verdict::Failed
                } else {
                    Verdict::Cancelled
                };
                summary.verdict = if report.verdict == Verdict::Failed {
                    AttemptVerdict::Failed
                } else {
                    AttemptVerdict::Cancelled
                };
            }
        }
        if details.cancellation.is_some() && details.unresolved() {
            if let Some(report) = &mut details.report {
                report.verdict = Verdict::Unknown;
            }
            summary.verdict = AttemptVerdict::Unknown;
        }
        if !details.unresolved() && !details.resources.iter().any(|r| r.kind == "Task") {
            details.cancellation = None;
        }
        if let Some(report) = &mut details.report {
            report.cases.sort_by_key(|r| {
                details
                    .cases
                    .iter()
                    .position(|c| c.id == r.id)
                    .unwrap_or(usize::MAX)
            });
        }
        details.report = details.report.filter(|report| !report.cases.is_empty());
        self.summary.checkpoint(summary, details.clone())?;
        self.summary.complete_binding(id, &details)?;
        let view = details.view(id);
        let mut inner = self.inner.lock().unwrap();
        let active = inner.active.as_ref().map(|a| a.id);
        if let Some(id) = parse(id).map(SendId) {
            if let Some(send) = inner.views.get_mut(&id) {
                send.report = view.report.clone();
                send.cancellation = view.cancellation;
                send.error = None;
                if !details.unresolved() {
                    send.pending = None;
                }
                if active != Some(id) {
                    send.state = if view
                        .report
                        .as_ref()
                        .is_some_and(|r| r.verdict != Verdict::Unknown)
                        && !details.unresolved()
                    {
                        SendState::Complete
                    } else {
                        SendState::Unknown
                    };
                    send.stage = Stage::Done;
                }
            }
        }
        drop(inner);
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
        let _history = self.history_gate.lock().await;
        let document = folder.document_id();
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
            action: SendAction::Create,
            can_separate: false,
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
            let transport = access.transport(&fresh, provider).await.map_err(|error| {
                if error == ClusterError::DestinationMismatch {
                    deployment_row(Stage::DestinationMismatch)
                } else {
                    CheckRow::new(
                        "Installation",
                        CannotCheck,
                        "Bridge cannot check the selected cluster. Reconnect in Clusters.",
                    )
                }
            })?;
            let mut rows = vec![CheckRow::new(
                "Installation",
                Allowed,
                "The selected cluster and required resources were checked.",
            )];
            rows.extend(access.check(&transport, provider, &pair).await);
            if rows.iter().all(CheckRow::permits) {
                let target = binding::target(access.binding());
                let desired = pair.agent().spec(transport.provider());
                check.can_separate = self
                    .summary
                    .binding(document, &target)
                    .is_ok_and(|b| b.is_some());
                let decision = if self.inner.lock().unwrap().active.is_some() {
                    Err(Stage::EarlierWork)
                } else {
                    self.deployment(document, &target, &transport).await
                };
                match decision {
                    Ok(Some(bound)) => {
                        check.action = bound.action(&desired);
                        if check.action == SendAction::Update
                            && access
                                .check_agent_patch(&fresh, &transport, &bound.name)
                                .await
                                .is_err()
                        {
                            rows.push(deployment_row(Stage::UpdateDenied));
                        }
                    }
                    Ok(None) => (),
                    Err(stage) => rows.push(deployment_row(stage)),
                }
            }
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
        self.start_folder(
            self.folder.clone(),
            target_id,
            provider_id,
            expected_source,
            false,
        )
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
        self.start_folder(folder, target, provider, expected, false)
            .await;
        self.status_document(id)
    }
    pub async fn start_document_separate(
        self: &Arc<Self>,
        folder: Arc<AgentFolder>,
        target: &str,
        provider: &str,
        expected: SnapshotId,
        separate_agent: bool,
    ) -> SendStatus {
        let id = folder.document_id();
        self.start_folder(folder, target, provider, expected, separate_agent)
            .await;
        self.status_document(id)
    }
    async fn start_folder(
        self: &Arc<Self>,
        folder: Arc<AgentFolder>,
        target_id: &str,
        provider_id: &str,
        expected_source: SnapshotId,
        separate_agent: bool,
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
            agent_action: None,
            state: SendState::Running,
            stage: Stage::Capture,
            acknowledged: Vec::new(),
            pending: None,
            report: None,
            error: None,
            storage_error: false,
            cancellation: None,
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
            cancel_requested: false,
            document: folder.document(),
            id,
            stop,
            done,
        });
        drop(inner);
        let owner = self.clone();
        tokio::spawn(async move {
            let cancellation_access = admission.clone();
            let mut result = Some(
                owner
                    .drive(
                        id,
                        folder,
                        admission,
                        provider,
                        (&expected_source, separate_agent),
                        &mut stop_rx,
                    )
                    .await,
            );
            let finalized = {
                let mut inner = owner.inner.lock().unwrap();
                // Accepting an intent and choosing finalization are one atomic decision.
                inner.finish(id, &mut result, false)
            };
            if !finalized {
                match owner.summary.record(&id.0.to_string()) {
                    Ok((summary, Some(details))) => {
                        match owner
                            .refresh_saved(
                                &id.0.to_string(),
                                true,
                                cancellation_access,
                                summary,
                                details,
                            )
                            .await
                        {
                            Ok(view) => {
                                if let Some(report) = view.report {
                                    result = Some(Ok(report));
                                }
                            }
                            Err(_) => owner.update(id, |v| {
                                v.storage_error = true;
                                v.cancellation = Some(Cancellation::Unconfirmed);
                            }),
                        }
                    }
                    Ok((_, None)) => owner.update(id, |v| v.cancellation = None),
                    Err(error) => owner.update(id, |v| {
                        // Before submission there may be no record and no write intent.
                        if error == "notFound" && v.acknowledged.is_empty() && v.pending.is_none() {
                            v.cancellation = None;
                        } else {
                            v.storage_error = true;
                            v.cancellation = Some(Cancellation::Unconfirmed);
                        }
                    }),
                }
                owner.inner.lock().unwrap().finish(id, &mut result, true);
            }
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
    /// Latch intent before returning. The admitted native job owns all remote cleanup.
    pub fn cancel_document(
        &self,
        document: DocumentId,
        id: &str,
    ) -> Result<SendStatus, &'static str> {
        let id = SendId(parse(id).ok_or("invalidId")?);
        {
            let mut inner = self.inner.lock().unwrap();
            let view = inner
                .views
                .get(&id)
                .filter(|v| v.document_id == document)
                .ok_or("The Send does not belong to this agent.")?;
            if view.state == SendState::Running
                && inner
                    .active
                    .as_ref()
                    .is_some_and(|a| a.id == id && !a.cancel_requested)
            {
                let active = inner.active.as_mut().unwrap();
                active.cancel_requested = true;
                active.halt();
                inner.views.get_mut(&id).unwrap().cancellation = Some(Cancellation::Cancelling);
            }
        }
        Ok(self.status_document(document))
    }
    pub async fn history_cancel(self: &Arc<Self>, id: &str) -> Result<DetailView, &'static str> {
        let owner = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _gate = owner.history_gate.lock().await;
            {
                let inner = owner.inner.lock().unwrap();
                if inner.active.is_some() || inner.closed {
                    return Err("busy");
                }
            }
            let (summary, details) = owner.summary.record(&id)?;
            let details = details
                .filter(|d| d.view(&id).can_cancel)
                .ok_or("notRecoverable")?;
            owner
                .refresh_saved(&id, true, owner.session.history_access(), summary, details)
                .await
        })
        .await
        .map_err(|_| "storage")?
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
        request: (&SnapshotId, bool),
        stop: &mut watch::Receiver<bool>,
    ) -> Result<RunReport, Stage> {
        let (expected_source, separate_agent) = request;
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
        let transport = access.transport(&fresh, provider).await.map_err(|error| {
            if error == ClusterError::DestinationMismatch {
                Stage::DestinationMismatch
            } else {
                Stage::Preflight
            }
        })?;
        let checked = until_stop(stop, Stage::Preflight, async {
            Ok::<_, ()>(access.check(&transport, provider, &pair).await)
        })
        .await;
        let document = self.inner.lock().unwrap().views[&id].document_id;
        let destination = binding::target(access.binding());
        let desired = pair.agent().spec(transport.provider());
        let deployment = async {
            if separate_agent {
                // An explicit recovery still validates the store, source and destination.
                self.summary
                    .binding(document, &destination)
                    .map_err(binding::lookup_stage)?
                    .ok_or(Stage::Preflight)?;
                return Ok(None);
            }
            let bound = self.deployment(document, &destination, &transport).await?;
            if let Some(bound) = &bound {
                if bound.action(&desired) == SendAction::Update {
                    access
                        .check_agent_patch(&fresh, &transport, &bound.name)
                        .await
                        .map_err(|e| {
                            if e == ClusterError::Denied {
                                Stage::UpdateDenied
                            } else {
                                Stage::UpdateUnconfirmed
                            }
                        })?;
                }
            }
            Ok(bound)
        }
        .await;
        let runner = NativeRunner::with_transport(transport);
        let binding = access.binding();
        let seed = AttemptSeed {
            source: pair.snapshot().clone(),
            target_display: format!("{} · {}", binding.context_name, binding.watched_namespace),
            target_uids: binding::target(binding),
            provider: provider_name,
            time: unix_seconds(),
        };
        let result = match checked {
            Ok(rows) if rows.iter().all(CheckRow::permits) => match deployment {
                Ok(bound) => self.run(id, &runner, pair, seed, bound, stop).await,
                Err(stage) => Err(stage),
            },
            Ok(_) => Err(Stage::Preflight),
            Err(stage) => Err(stage),
        };
        runner.close().await;
        if matches!(result, Err(Stage::Storage)) {
            self.update(id, |v| v.storage_error = true);
        }
        result
    }
    async fn run(
        &self,
        id: SendId,
        runner: &NativeRunner<'_>,
        pair: ParsedPair,
        seed: AttemptSeed,
        bound: Option<Binding>,
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
            cancellation: None,
        };
        let desired = pair.agent().spec(runner.transport().provider());
        let action = bound
            .as_ref()
            .map(|b| b.action(&desired))
            .unwrap_or(SendAction::Create);
        self.update(id, |v| v.agent_action = Some(action));
        let previous = bound.clone();
        let mut bound = match bound {
            Some(mut bound) => {
                bound.attempt = id.0.to_string();
                bound.unfinished = true;
                bound
            }
            None => {
                let planned = runner.prepare(pair.clone()).map_err(|_| Stage::Submit)?;
                Binding {
                    document: self.inner.lock().unwrap().views[&id].document_id,
                    target: seed.target_uids.clone(),
                    current: true,
                    name: planned.agent_name().into(),
                    desired: desired.clone(),
                    acknowledged: None,
                    intent: Some(desired.clone()),
                    attempt: id.0.to_string(),
                    unfinished: true,
                }
            }
        };
        if action == SendAction::Update {
            bound.intent = Some(desired.clone());
        }
        // Mutation intent precedes history. An unchanged run has no mutation:
        // publish its history first, then reserve it at AgentAcknowledged, before Tasks.
        if action != SendAction::Unchanged {
            self.summary
                .save_binding(bound.clone())
                .map_err(|_| Stage::Storage)?;
        }
        let planned_agent_name = bound.name.clone();
        details.resources.push(if action == SendAction::Unchanged {
            crate::engine::run::SavedResource::acknowledged(&bound.receipt()?)
        } else {
            crate::engine::run::SavedResource::planned(
                "Agent",
                &bound.name,
                &bound.target.watched_namespace,
                &desired,
            )
        });
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
            // Ownership is already known for reuse/update, even if the new revision
            // is not acknowledged. Its unknown attempt must supersede the old pass.
            agent_uid: bound
                .acknowledged
                .as_ref()
                .and_then(|agent| agent["metadata"]["uid"].as_str())
                .map(str::to_owned),
            verdict: AttemptVerdict::Running,
            time: seed.time,
        };
        let initial = self.summary.checkpoint(summary.clone(), details.clone());
        if initial.is_err() {
            // No mutation was polled: undo only this attempt's local reservation.
            if let Some(previous) = previous {
                self.summary.save_binding(previous)
            } else {
                self.summary.reject_creation(&bound)
            }
            .map_err(|_| Stage::Storage)?;
            self.update(id, |v| v.storage_error = true);
            return Err(Stage::Storage);
        }
        let prepared = if bound.acknowledged.is_some() {
            let receipt = if action == SendAction::Update {
                match self
                    .update_deployment(
                        &mut bound,
                        runner.transport(),
                        desired,
                        stop,
                        previous.as_ref().ok_or(Stage::Storage)?,
                    )
                    .await
                {
                    Ok(receipt) => receipt,
                    Err(stage) => {
                        summary.verdict = AttemptVerdict::Unknown;
                        if bound.intent.is_none() {
                            details.resources[0] =
                                crate::engine::run::SavedResource::acknowledged(&bound.receipt()?);
                        }
                        self.summary
                            .checkpoint(summary, details)
                            .map_err(|_| Stage::Storage)?;
                        return Err(stage);
                    }
                }
            } else {
                bound.receipt()?
            };
            runner.prepare_reuse(pair, receipt)
        } else {
            runner.prepare_named(pair, &bound.name)
        }
        .map_err(|_| Stage::Submit)?;
        let mut checkpoint = |event| {
            let task_barrier = matches!(&event, RunCheckpoint::Before(r) if r.kind == "Task");
            let terminal = matches!(&event, RunCheckpoint::Case(..));
            match event {
                RunCheckpoint::AgentAcknowledged(resource) => {
                    bound
                        .acknowledge(resource.value())
                        .map_err(|_| ErrorKind::Identity)?;
                    self.summary
                        .save_binding(bound.clone())
                        .map_err(|_| ErrorKind::Storage)?;
                    return Ok(());
                }
                RunCheckpoint::Before(resource) => {
                    if let Some(old) = details
                        .resources
                        .iter_mut()
                        .find(|r| r.name == resource.name)
                    {
                        *old = resource;
                    } else {
                        details.resources.push(resource);
                    }
                }
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
                RunCheckpoint::Evidence(_) => return Ok(()),
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
            self.update(id, |v| v.report = details.report.clone());
            details.captured_at = unix_seconds();
            if self
                .summary
                .checkpoint(summary.clone(), details.clone())
                .and_then(|_| {
                    if terminal {
                        self.summary.complete_binding(&summary.id, &details)?;
                    }
                    if task_barrier {
                        // Preserve the pending identity first, then reserve the
                        // binding durably before the runner may poll Task POST.
                        bound.unfinished = true;
                        self.summary.save_binding(bound.clone())?;
                    }
                    Ok(())
                })
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
                let rejected = matches!(
                    error.kind,
                    ErrorKind::Authentication
                        | ErrorKind::Permission
                        | ErrorKind::Invalid
                        | ErrorKind::Conflict
                        | ErrorKind::NotFound
                        | ErrorKind::Unsupported
                );
                if rejected {
                    if let Some(pending) = &error.pending {
                        details.resources.retain(|resource| {
                            resource.kind != pending.kind || resource.name != pending.name
                        });
                        if pending.kind == "Agent"
                            && error.handle.agent().is_none()
                            && self.summary.reject_creation(&bound).is_err()
                        {
                            self.update(id, |v| v.storage_error = true);
                            return Err(Stage::Storage);
                        }
                    }
                }
                self.update(id, |v| {
                    v.acknowledged = receipts(&error.handle);
                    v.pending = error.pending.filter(|_| !rejected).map(|p| PendingRef {
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
                // Preserve the rejected Task's absence before releasing its barrier.
                if self
                    .summary
                    .checkpoint(summary.clone(), details.clone())
                    .is_err()
                {
                    self.update(id, |v| v.storage_error = true);
                } else {
                    self.summary
                        .complete_binding(&summary.id, &details)
                        .map_err(|_| Stage::Storage)?;
                }
                return Err(failed(
                    error.kind,
                    if rejected {
                        Stage::SubmitRejected
                    } else {
                        Stage::Submit
                    },
                ));
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
            Ok(mut report) => {
                if report.verdict == Verdict::Cancelled {
                    report.verdict = if report
                        .cases
                        .iter()
                        .any(|row| row.verdict == Verdict::Failed)
                    {
                        Verdict::Failed
                    } else if report
                        .cases
                        .iter()
                        .any(|row| row.verdict == Verdict::Unknown)
                    {
                        Verdict::Unknown
                    } else {
                        Verdict::Cancelled
                    };
                }
                summary.verdict = match report.verdict {
                    Verdict::Passed => AttemptVerdict::Passed,
                    Verdict::Failed => AttemptVerdict::Failed,
                    Verdict::Unknown => AttemptVerdict::Unknown,
                    Verdict::Cancelled => AttemptVerdict::Cancelled,
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
        if self
            .summary
            .checkpoint(summary.clone(), details.clone())
            .is_err()
        {
            self.update(id, |v| v.storage_error = true);
            return Err(Stage::Observe);
        }
        self.summary
            .complete_binding(&summary.id, &details)
            .map_err(|_| Stage::Storage)?;
        result
    }
}
