//! Fixed example Run on the original VM client. Credentials remain in the guest.
use crate::{
    agent_folder::{example_for_model, AgentFolder, DocumentId, DocumentRef},
    engine::run::{
        ErrorKind, NativeRunner, ParsedPair, RunCheckpoint, RunFuture, RunHandle, RunLimits,
        RunReport, RunTransport,
    },
    local_vm::{LocalVm, LocalVmStatus},
    overview::{LocalRunObservation, ReportSummary},
    pair_source::CapturedPair,
    runtime::client::VmClient,
};
use bridge_runtime_protocol::{ModelPhase, OrkaRequest, SnapshotId, Uuid, MAX_ORKA_BODY};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{sync::watch, time::Instant};
#[path = "example_run_cancel.rs"]
mod cancel;
pub use cancel::Cancellation;
use cancel::Recovery;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Idle,
    Running,
    Complete,
    Unknown,
}
#[derive(Clone, Serialize)]
pub struct Receipt {
    pub kind: &'static str,
    pub name: String,
    pub uid: String,
}
#[derive(Clone, Serialize)]
pub struct PendingRef {
    pub kind: &'static str,
    pub name: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub document_id: DocumentId,
    pub busy: Option<DocumentRef>,
    pub state: RunState,
    pub can_run: bool,
    pub can_cancel: bool,
    pub can_retry_cancellation: bool,
    pub can_check_cancellation: bool,
    pub cancellation: Cancellation,
    pub model_connected: bool,
    pub source: Option<SnapshotId>,
    pub stage: Option<&'static str>,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<bridge_runtime_protocol::ModelVendor>,
    pub report: Option<RunReport>,
    pub acknowledged: Vec<Receipt>,
    pub pending: Option<PendingRef>,
    pub error: Option<&'static str>,
    /// Renderer correlation, recorded only by the admission that accepted it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<Uuid>,
}
impl RunSnapshot {
    fn claim_end(&mut self, stop: &watch::Receiver<bool>, phase: RunState) -> bool {
        let cancelled = *stop.borrow();
        if !cancelled {
            self.state = phase;
        }
        cancelled
    }
    fn idle(document_id: DocumentId) -> Self {
        Self {
            document_id,
            busy: None,
            state: RunState::Idle,
            can_run: false,
            can_cancel: false,
            can_retry_cancellation: false,
            can_check_cancellation: false,
            cancellation: Cancellation::None,
            model_connected: false,
            source: None,
            stage: None,
            model: None,
            vendor: None,
            report: None,
            acknowledged: Vec::new(),
            pending: None,
            error: None,
            attempt_id: None,
        }
    }
}
#[cfg(test)]
mod terminal_claim_tests {
    use super::*;
    #[tokio::test]
    async fn validation_failure_before_submission_keeps_cancelled_before_start() {
        let run = ExampleRun::new(
            tokio::runtime::Handle::current(),
            true,
            Arc::new(AgentFolder::new(None)),
        );
        let mut snapshot = RunSnapshot::idle(DocumentId::SAMPLE);
        snapshot.state = RunState::Running;
        snapshot.stage = Some("model");
        snapshot.cancellation = Cancellation::Cancelling;
        run.0
            .lock()
            .unwrap()
            .snapshots
            .insert(DocumentId::SAMPLE, snapshot);
        run.fail(DocumentId::SAMPLE, "submit");
        let state = run.0.lock().unwrap();
        let done = &state.snapshots[&DocumentId::SAMPLE];
        assert_eq!(done.state, RunState::Complete);
        assert!(done.cancellation == Cancellation::BeforeStart);
        assert!(done.error.is_none());
        assert!(done.acknowledged.is_empty() && done.pending.is_none());
        let report = done.report.as_ref().unwrap();
        assert!(report.cases.is_empty());
        assert_eq!(report.verdict, crate::engine::run::Verdict::Cancelled);
    }
    #[test]
    fn stop_and_terminal_claim_have_only_two_ordered_outcomes() {
        for phase in [RunState::Unknown, RunState::Complete] {
            let (sender, stop) = watch::channel(false);
            let mut snapshot = RunSnapshot::idle(DocumentId::SAMPLE);
            snapshot.state = RunState::Running;
            assert!(!snapshot.claim_end(&stop, phase));
            assert_ne!(
                snapshot.state,
                RunState::Running,
                "late Cancel admission is retired before history I/O"
            );
            sender.send(true).unwrap();
            assert_eq!(snapshot.state, phase);
            let mut snapshot = RunSnapshot::idle(DocumentId::SAMPLE);
            snapshot.state = RunState::Running;
            assert!(snapshot.claim_end(&stop, phase));
            assert_eq!(
                snapshot.state,
                RunState::Running,
                "accepted Stop must reach cancellation proof instead of failure/completion"
            );
        }
    }
}
#[derive(Default)]
struct State {
    retired: bool,
    snapshots: HashMap<DocumentId, RunSnapshot>,
    active: Option<DocumentRef>,
    observation: Option<LocalRunObservation>,
    recorded_current: bool,
    stop: Option<watch::Sender<bool>>,
    recovery: HashMap<DocumentId, Recovery>,
    cleanup_cursor: usize,
}
pub struct ExampleRun(
    Mutex<State>,
    tokio::runtime::Handle,
    bool,
    Arc<AgentFolder>,
    crate::local_results::LocalResults,
);
impl ExampleRun {
    /// The native shell supplies its runtime; a blocking IPC thread need not enter it.
    pub fn new(
        runtime: tokio::runtime::Handle,
        model_image_available: bool,
        folder: Arc<AgentFolder>,
    ) -> Arc<Self> {
        Self::with_history_root(runtime, model_image_available, folder, None)
    }
    pub fn with_history_root(
        runtime: tokio::runtime::Handle,
        model_image_available: bool,
        folder: Arc<AgentFolder>,
        app_local_data: Option<std::path::PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self(
            Mutex::default(),
            runtime,
            model_image_available,
            folder,
            crate::local_results::LocalResults::new(app_local_data),
        ))
    }
    pub fn local_results(&self) -> std::io::Result<crate::local_results::LocalResultsSnapshot> {
        self.4.list()
    }
    pub fn is_active(&self) -> bool {
        self.0.lock().unwrap().active.is_some()
    }
    pub fn overview_observation(&self) -> Option<LocalRunObservation> {
        let state = self.0.lock().unwrap();
        (!state.retired)
            .then(|| state.observation.clone())
            .flatten()
    }
    /// Retires every in-memory run observation after a confirmed
    /// workspace replacement or restore: a prior pass against the old (or a
    /// different) installation must never paint as current on this one.
    /// Saved `LocalResults` history is untouched; it is already installation-scoped.
    pub fn retire_for_workspace_replacement(&self) {
        let mut state = self.0.lock().unwrap();
        state.retired = true;
        // In-flight work still owns these entries. Let it finish (and save its
        // installation-scoped history), but suppress all of its live projections.
        if state.active.is_none() {
            state.snapshots.clear();
            state.recovery.clear();
            state.observation = None;
            state.recorded_current = false;
        }
    }
    /// Sample-only compatibility for native setup callers.
    pub fn status(&self, vm: &LocalVm) -> RunSnapshot {
        self.status_document(vm, DocumentId::SAMPLE)
    }
    pub fn status_document(&self, vm: &LocalVm, id: DocumentId) -> RunSnapshot {
        let state = self.0.lock().unwrap();
        let mut snapshot = if state.retired {
            RunSnapshot::idle(id)
        } else {
            state
                .snapshots
                .get(&id)
                .cloned()
                .unwrap_or_else(|| RunSnapshot::idle(id))
        };
        snapshot.busy = state.active.clone();
        let cancellable = snapshot.state == RunState::Running;
        // Terminal means admission is released, not merely that the report arrived.
        if !state.retired
            && state
                .active
                .as_ref()
                .is_some_and(|job| job.document_id == id)
        {
            snapshot.state = RunState::Running;
        } else if matches!(snapshot.state, RunState::Complete | RunState::Unknown) {
            snapshot.stage = Some("done");
        }
        let supported = vm.run_cancel_supported();
        snapshot.can_cancel = supported
            && cancellable
            && state
                .active
                .as_ref()
                .is_some_and(|job| job.document_id == id)
            && snapshot.cancellation == Cancellation::None;
        snapshot.can_check_cancellation = supported
            && !state.retired
            && state.active.is_none()
            && state.recovery.contains_key(&id)
            && snapshot.cancellation == Cancellation::Unconfirmed
            && snapshot.error != Some("session");
        snapshot.can_retry_cancellation &= snapshot.can_check_cancellation;
        snapshot.model_connected = vm.model_connected();
        snapshot.can_run = self.2
            && state.active.is_none()
            && vm.status() == LocalVmStatus::Ready
            && vm.try_authoring().is_some();
        snapshot
    }
    pub fn cancel(self: &Arc<Self>, vm: Arc<LocalVm>, id: DocumentId) -> RunSnapshot {
        self.request_cancellation(vm, id, true)
    }
    pub fn check_cancellation(self: &Arc<Self>, vm: Arc<LocalVm>, id: DocumentId) -> RunSnapshot {
        self.request_cancellation(vm, id, false)
    }
    fn request_cancellation(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
        retry: bool,
    ) -> RunSnapshot {
        if !vm.run_cancel_supported() {
            return self.status_document(&vm, id);
        }
        {
            let mut state = self.0.lock().unwrap();
            if state.retired {
                drop(state);
                return self.status_document(&vm, id);
            }
            if state
                .active
                .as_ref()
                .is_some_and(|job| job.document_id == id)
            {
                if retry
                    && state.snapshots[&id].state == RunState::Running
                    && state.snapshots[&id].cancellation == Cancellation::None
                {
                    state.stop.as_ref().unwrap().send_replace(true);
                    state.snapshots.get_mut(&id).unwrap().cancellation = Cancellation::Cancelling;
                }
                drop(state);
                return self.status_document(&vm, id);
            }
        }
        let Some(admission) = vm.try_authoring() else {
            return self.status_document(&vm, id);
        };
        let recovery = {
            let mut state = self.0.lock().unwrap();
            let recovery = state.recovery.get(&id).cloned().filter(|_| {
                !state.retired
                    && state.active.is_none()
                    && state.snapshots[&id].cancellation == Cancellation::Unconfirmed
                    && (!retry || state.snapshots[&id].can_retry_cancellation)
            });
            let Some(recovery) = recovery else {
                drop(state);
                return self.status_document(&vm, id);
            };
            state.active = Some(recovery.document.clone());
            let snapshot = state.snapshots.get_mut(&id).unwrap();
            snapshot.cancellation = Cancellation::Cancelling;
            snapshot.stage = Some("cancel");
            recovery
        };
        let owner = self.clone();
        let target = vm.clone();
        self.1.spawn(async move {
            let runtime = tokio::runtime::Handle::current();
            let worker = owner.clone();
            let observing = target.clone();
            let result = target
                .with_client(move |installation, client| {
                    if !observing.authoring_current(&admission)
                        || installation != recovery.installation
                        || client.boot_nonce() != recovery.boot
                    {
                        worker.recovery_failed(id);
                        return;
                    }
                    let runner = NativeRunner::resume_with_transport(
                        VmRunTransport {
                            client: Mutex::new(client),
                            provider: recovery.provider.clone(),
                            vm: observing,
                        },
                        &recovery.handle,
                    );
                    runtime.block_on(worker.finish_cancellation(id, &runner, recovery, retry));
                })
                .await;
            if result.is_err() && !matches!(&result, Err(crate::runtime::types::RuntimeError::Busy))
            {
                owner.recovery_failed(id);
            }
            let mut state = owner.0.lock().unwrap();
            if state.snapshots[&id].error.is_some()
                || matches!(&result, Err(crate::runtime::types::RuntimeError::Busy))
            {
                state.snapshots.get_mut(&id).unwrap().cancellation = Cancellation::Unconfirmed;
            }
            state.active = None;
        });
        self.status_document(&vm, id)
    }
    fn recovery_failed(&self, id: DocumentId) {
        // Recovery owns one document's snapshot, not the latest run's global
        // observation or previously saved history (possibly another document).
        let mut state = self.0.lock().unwrap();
        let snapshot = state.snapshots.get_mut(&id).unwrap();
        snapshot.error = Some("session");
        snapshot.cancellation = Cancellation::Unconfirmed;
    }
    fn cancelled_before_start(&self, id: DocumentId) {
        let mut state = self.0.lock().unwrap();
        let snapshot = state.snapshots.get_mut(&id).unwrap();
        snapshot.cancellation = Cancellation::BeforeStart;
        snapshot.state = RunState::Complete;
        snapshot.error = None;
        snapshot.acknowledged.clear();
        snapshot.pending = None;
        snapshot.report = Some(RunReport {
            cases: Vec::new(),
            verdict: crate::engine::run::Verdict::Cancelled,
        });
    }
    pub fn start_first_test(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
    ) -> Result<RunSnapshot, &'static str> {
        self.start_first_test_with_model(vm, id, "gpt-5.6-terra".into(), None)
    }
    pub fn start_first_test_with_model(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        id: DocumentId,
        model: String,
        attempt_id: Option<Uuid>,
    ) -> Result<RunSnapshot, &'static str> {
        if id != DocumentId::SAMPLE
            || example_for_model(&model).is_none()
            || attempt_id.is_some_and(|attempt| attempt.is_nil())
        {
            return Err("The automatic first test requires the connected bundled example.");
        }
        Ok(self.start_checked(vm, self.3.clone(), Some(model), attempt_id))
    }
    pub fn start(self: &Arc<Self>, vm: Arc<LocalVm>) -> RunSnapshot {
        self.start_document(vm, self.3.clone())
    }
    pub fn start_document(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        folder: Arc<AgentFolder>,
    ) -> RunSnapshot {
        self.start_checked(vm, folder, None, None)
    }
    fn start_checked(
        self: &Arc<Self>,
        vm: Arc<LocalVm>,
        folder: Arc<AgentFolder>,
        expected_model: Option<String>,
        attempt_id: Option<Uuid>,
    ) -> RunSnapshot {
        let id = folder.document_id();
        if !self.2 {
            return self.status_document(&vm, id);
        }
        let (sender, stop) = watch::channel(false);
        let admission = {
            let mut state = self.0.lock().unwrap();
            // Acquire admission under the retirement lock, never carry an old
            // workspace's lease into a newly unretired live snapshot.
            let admission = vm.try_authoring();
            if state.active.is_some()
                || vm.status() != LocalVmStatus::Ready
                || admission
                    .as_ref()
                    .is_none_or(|lease| !vm.authoring_current(lease))
            {
                drop(state);
                return self.status_document(&vm, id);
            }
            if state.retired {
                state.snapshots.clear();
                state.recovery.clear();
                state.observation = None;
                state.recorded_current = false;
                state.retired = false;
            }
            state.active = Some(folder.document());
            state.recorded_current = false;
            state.stop = Some(sender);
            state.recovery.remove(&id);
            let snapshot = state
                .snapshots
                .entry(id)
                .or_insert_with(|| RunSnapshot::idle(id));
            snapshot.state = RunState::Running;
            snapshot.can_run = false;
            snapshot.cancellation = Cancellation::None;
            snapshot.can_retry_cancellation = false;
            snapshot.error = None;
            snapshot.report = None;
            snapshot.source = None;
            snapshot.stage = Some("capture");
            snapshot.model = None;
            snapshot.vendor = None;
            // Only this admission may answer for the request; denials keep the old one.
            snapshot.attempt_id = attempt_id;
            // Keep this document's receipts through preflight failures.
            admission.unwrap()
        };
        let owner = self.clone();
        let target = vm.clone();
        self.1.spawn(async move {
            let admission = Arc::new(admission);
            let bundled_example = expected_model.is_some();
            if *stop.borrow() {
                owner.cancelled_before_start(id);
                owner.0.lock().unwrap().active = None;
                return;
            }
            let pair = tokio::task::spawn_blocking(move || {
                let snapshot = folder.capture().map_err(|_| ())?;
                if let Some(model) = expected_model {
                    let (agent, tests) = example_for_model(&model).ok_or(())?;
                    for (slot, expected) in [
                        (bridge_runtime_protocol::FileSlot::Agent, agent.as_str()),
                        (bridge_runtime_protocol::FileSlot::Tests, tests),
                    ] {
                        let actual = std::str::from_utf8(snapshot.file(slot)).map_err(|_| ())?;
                        if actual.replace("\r\n", "\n") != expected.replace("\r\n", "\n") {
                            return Err(());
                        }
                    }
                }
                // Parse and execute this held capture, never a later file read.
                CapturedPair::from_snapshot(snapshot).map_err(|_| ())
            })
            .await;
            if *stop.borrow() {
                owner.cancelled_before_start(id);
            } else if let Ok(Ok(pair)) = pair {
                let pair = pair.into_pair();
                {
                    let mut state = owner.0.lock().unwrap();
                    let snapshot = state.snapshots.get_mut(&id).unwrap();
                    snapshot.source = Some(pair.snapshot().clone());
                    snapshot.model = Some(pair.model().into());
                    snapshot.stage = Some("model");
                }
                let runtime = tokio::runtime::Handle::current();
                let worker = owner.clone();
                let observing = target.clone();
                if target
                    .with_client(move |installation, client| {
                        if !observing.authoring_current(&admission) {
                            worker.fail(id, "session");
                            return;
                        }
                        runtime.block_on(worker.execute(
                            id,
                            installation,
                            client,
                            observing,
                            (pair, bundled_example),
                            stop,
                        ));
                        // Observation is over, but admission and the original client
                        // are still held. Retained results keep their own Agents.
                        worker.cleanup(installation, client);
                    })
                    .await
                    .is_err()
                {
                    owner.fail(id, "session");
                }
            } else {
                owner.fail(id, "source");
            }
            // One global admission, released only after the client work returns.
            let mut state = owner.0.lock().unwrap();
            state.active = None;
            state.stop = None;
        });
        self.status_document(&vm, id)
    }
    fn cleanup(&self, installation: Uuid, client: &mut VmClient) {
        let Ok(mut candidates) = self.4.cleanup_candidates(installation) else {
            return;
        };
        if candidates.is_empty() {
            return;
        }
        // Finalizers and unfinished remote work may block an Agent indefinitely.
        // Continue beyond them on later sweeps rather than starving the backlog.
        let offset = self.0.lock().unwrap().cleanup_cursor % candidates.len();
        candidates.rotate_left(offset);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        for (index, owner) in candidates.into_iter().take(8).enumerate() {
            if std::time::Instant::now() >= deadline {
                break;
            }
            self.0.lock().unwrap().cleanup_cursor = offset + index + 1;
            match client.cleanup_run_agent(&owner.agent_uid) {
                Ok(reply) if reply.removed => {
                    let _ = self.4.forget_removed(&owner);
                }
                Ok(_) => (),
                // No retry on an uncertain mutation or an unsupported peer.
                Err(_) => break,
            }
        }
    }
    fn fail(&self, id: DocumentId, message: &'static str) {
        let mut state = self.0.lock().unwrap();
        if state.snapshots[&id].cancellation == Cancellation::Cancelling
            && matches!(state.snapshots[&id].stage, Some("capture" | "model"))
        {
            drop(state);
            self.cancelled_before_start(id);
            return;
        }
        if state.recorded_current {
            if let Some(observation) = &mut state.observation {
                observation.state = RunState::Unknown;
                observation.report = None;
            }
        }
        let snapshot = state.snapshots.get_mut(&id).unwrap();
        snapshot.state = RunState::Unknown;
        if snapshot.cancellation == Cancellation::Cancelling {
            snapshot.cancellation = Cancellation::Unconfirmed;
        }
        snapshot.error = Some(message);
        let observation = state
            .recorded_current
            .then(|| state.observation.clone())
            .flatten();
        drop(state);
        if let Some(observation) = observation {
            // Storage failure affects history availability, never local test admission.
            let _ = self.4.record(observation);
        }
    }
    async fn execute(
        &self,
        id: DocumentId,
        installation: Uuid,
        client: &mut VmClient,
        vm: Arc<LocalVm>,
        pair: (ParsedPair, bool),
        stop: watch::Receiver<bool>,
    ) {
        let (pair, bundled_example) = pair;
        if *stop.borrow() {
            self.cancelled_before_start(id);
            return;
        }
        if pair.agent().has_unknown_local_builtin() {
            self.fail(id, "source");
            return;
        }
        if pair.agent().has_unsupported_local_tool() {
            self.fail(id, "unsupportedTool");
            return;
        }
        let observed = client.model_status();
        if *stop.borrow() {
            self.cancelled_before_start(id);
            return;
        }
        let model = match observed {
            Ok(model) if model.phase == ModelPhase::Ready => model,
            Ok(_) => {
                vm.clear_model_acknowledgment();
                self.fail(id, "model");
                return;
            }
            Err(_) => {
                self.fail(id, "session");
                return;
            }
        };
        let Some(connection) = model.connection else {
            self.fail(id, "model");
            return;
        };
        if bundled_example && connection.selection.model != pair.model() {
            self.fail(id, "model");
            return;
        }
        self.0
            .lock()
            .unwrap()
            .snapshots
            .get_mut(&id)
            .unwrap()
            .vendor = connection.selection.vendor;
        let boot = client.boot_nonce();
        let provider = connection.selection.provider;
        let runner = NativeRunner::with_transport(VmRunTransport {
            client: Mutex::new(client),
            provider: provider.clone(),
            vm,
        });
        let prepared = match runner.prepare(pair) {
            Ok(prepared) => prepared,
            Err(_) => {
                self.fail(id, "submit");
                return;
            }
        };
        self.0.lock().unwrap().snapshots.get_mut(&id).unwrap().stage = Some("submit");
        let handle = match runner.submit_prepared(prepared, stop.clone()).await {
            Ok(handle) => handle,
            Err(error)
                if self
                    .0
                    .lock()
                    .unwrap()
                    .snapshots
                    .get_mut(&id)
                    .unwrap()
                    .claim_end(&stop, RunState::Unknown) =>
            {
                let recovery = Recovery {
                    document: self.0.lock().unwrap().active.clone().unwrap(),
                    installation,
                    boot,
                    provider,
                    handle: *error.handle,
                    pending: error.pending.map(|p| PendingRef {
                        kind: p.kind,
                        name: p.name,
                    }),
                };
                self.finish_cancellation(id, &runner, recovery, true).await;
                return;
            }
            Err(error) => {
                self.record(
                    id,
                    installation,
                    &error.handle,
                    error.pending.map(|p| PendingRef {
                        kind: p.kind,
                        name: p.name,
                    }),
                    None,
                    RunState::Unknown,
                );
                self.fail(id, "submit");
                return;
            }
        };
        self.record(id, installation, &handle, None, None, RunState::Running);
        self.0.lock().unwrap().snapshots.get_mut(&id).unwrap().stage = Some("observe");
        let observed = runner
            .observe_checkpointed(&handle, stop.clone(), &mut |event| {
                if let RunCheckpoint::Case(_, case) | RunCheckpoint::Evidence(case) = event {
                    let mut state = self.0.lock().unwrap();
                    let report = state
                        .snapshots
                        .get_mut(&id)
                        .unwrap()
                        .report
                        .get_or_insert_with(|| RunReport {
                            cases: Vec::new(),
                            verdict: crate::engine::run::Verdict::Unknown,
                        });
                    handle.retain_evidence(
                        report,
                        RunReport {
                            cases: vec![case],
                            verdict: crate::engine::run::Verdict::Unknown,
                        },
                    );
                }
                Ok(())
            })
            .await;
        // Cancel and normal completion compete under the same lock. Once the
        // worker crosses its final Stop check, no request may claim to cancel it.
        let cancelled = self
            .0
            .lock()
            .unwrap()
            .snapshots
            .get_mut(&id)
            .unwrap()
            .claim_end(
                &stop,
                if observed.is_ok() {
                    RunState::Complete
                } else {
                    RunState::Unknown
                },
            );
        if cancelled {
            let recovery = Recovery {
                document: self.0.lock().unwrap().active.clone().unwrap(),
                installation,
                boot,
                provider,
                handle,
                pending: None,
            };
            self.finish_cancellation(id, &runner, recovery, true).await;
            return;
        }
        match observed {
            Ok(report) => {
                self.record(
                    id,
                    installation,
                    &handle,
                    None,
                    Some(report),
                    RunState::Complete,
                );
                self.0.lock().unwrap().snapshots.get_mut(&id).unwrap().state = RunState::Complete;
            }
            Err(_) => {
                self.record(id, installation, &handle, None, None, RunState::Unknown);
                self.fail(id, "observe");
            }
        }
    }
    fn record(
        &self,
        id: DocumentId,
        installation: Uuid,
        handle: &RunHandle,
        pending: Option<PendingRef>,
        report: Option<RunReport>,
        phase: RunState,
    ) {
        let mut state = self.0.lock().unwrap();
        // Only this operation's acknowledged handle supplies both source and UID.
        // A pre-ack retry failure must leave the previous coherent tuple untouched.
        if let Some(agent) = handle.agent() {
            state.recorded_current = true;
            state.observation = Some(LocalRunObservation {
                installation,
                document_id: id,
                source: handle.snapshot().clone(),
                namespace: agent.namespace.clone(),
                agent_name: agent.name.clone(),
                agent_uid: agent.uid.clone(),
                observed_at: crate::overview::now(),
                state: phase,
                report: report.as_ref().map(ReportSummary::from),
            });
        }
        let snapshot = state.snapshots.get_mut(&id).unwrap();
        snapshot.acknowledged = receipts(handle);
        snapshot.pending = pending;
        snapshot.report = report;
        let observation = handle.agent().and_then(|_| state.observation.clone());
        drop(state);
        if let Some(observation) = observation {
            let _ = self.4.record(observation);
        }
    }
}
/// Acknowledged identities only, shared by local Run and remote Send.
pub fn receipts(handle: &RunHandle) -> Vec<Receipt> {
    handle
        .agent()
        .map(|r| Receipt {
            kind: "Agent",
            name: r.name.clone(),
            uid: r.uid.clone(),
        })
        .into_iter()
        .chain(handle.tasks().iter().map(|r| Receipt {
            kind: "Task",
            name: r.name.clone(),
            uid: r.uid.clone(),
        }))
        .collect()
}
struct VmRunTransport<'a> {
    client: Mutex<&'a mut VmClient>,
    provider: String,
    vm: Arc<LocalVm>,
}
impl RunTransport for VmRunTransport<'_> {
    fn namespace(&self) -> &str {
        "orka-system"
    }
    fn provider(&self) -> &str {
        &self.provider
    }
    fn limits(&self) -> RunLimits {
        RunLimits::default()
    }
    fn max_create_body_bytes(&self) -> Option<usize> {
        Some(MAX_ORKA_BODY)
    }
    fn run_worker<'a>(
        &'a self,
        name: &'a str,
        uid: &'a str,
        deadline: Instant,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<bridge_runtime_protocol::RunWorkerReply, ErrorKind>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if self.vm.status() != LocalVmStatus::Ready {
                return Err(ErrorKind::Transport);
            }
            if Instant::now() >= deadline {
                return Err(ErrorKind::Deadline);
            }
            let reply = self
                .client
                .lock()
                .map_err(|_| ErrorKind::Transport)?
                .run_worker(name, uid)
                .map_err(|_| ErrorKind::Transport)?;
            if Instant::now() >= deadline {
                return Err(ErrorKind::Deadline);
            }
            Ok(reply)
        })
    }
    fn request<'a>(
        &'a self,
        path: &'a str,
        after: Option<u64>,
        body: Option<&'a Value>,
        deadline: Instant,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            if self.vm.status() != LocalVmStatus::Ready {
                return Err(ErrorKind::Transport);
            }
            if Instant::now() >= deadline {
                return Err(ErrorKind::Deadline);
            }
            let reply = self
                .client
                .lock()
                .map_err(|_| ErrorKind::Transport)?
                .orka_request(OrkaRequest {
                    path: path.into(),
                    after,
                    body: body.cloned(),
                })
                .map_err(|_| ErrorKind::Transport)?;
            // A successful in-flight create is authority, even after its budget:
            // retain its UID before Stop/error handling; later requests stay fenced.
            let created =
                body.is_some() && matches!(path, "agents" | "tasks") && reply.status == 201;
            if Instant::now() >= deadline && !created {
                return Err(ErrorKind::Deadline);
            }
            if path.ends_with("/cancel") {
                match reply.status {
                    200 => {}
                    409 => return Err(ErrorKind::Conflict),
                    401 => return Err(ErrorKind::Authentication),
                    403 => return Err(ErrorKind::Permission),
                    _ => return Err(ErrorKind::Response),
                }
            } else if reply.status != if body.is_some() { 201 } else { 200 } {
                return Err(ErrorKind::Response);
            }
            Ok(reply.body)
        })
    }
}
