//! App-level custody of one delivered native session; not a second VM lifecycle.
use crate::workspace_upgrade::{UpgradeActionReply, UpgradeCancelOutcome, UpgradeStatusDto};
use crate::{
    engine::{types::*, EngineTransport, TransportFuture},
    runtime::{
        client::VmClient,
        types::{RuntimeError, WorkspaceFailure, WorkspaceOperation},
    },
};
use serde::Serialize;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::sync::{watch, Mutex as AsyncMutex};
#[path = "local_vm_native.rs"]
pub(crate) mod native;
pub use native::{configured, upgrade_target};
/// Opaque Rust-owned target. No serde constructor or renderer-selected paths.
pub struct UpgradeTarget {
    pub(crate) source: native::image::ImageSource,
    pub(crate) machine_id: Vec<u8>,
}
#[derive(Serialize)]
pub struct ExamplePair {
    pub agent: &'static str,
    pub tests: &'static str,
}
pub fn example() -> ExamplePair {
    ExamplePair {
        agent: include_str!("../example-agent/web-reader.agent.md"),
        tests: include_str!("../example-agent/web-reader.agent.test.md"),
    }
}

// Both supported platforms now use the paired model-capable guest images.
pub const MODEL_IMAGE_AVAILABLE: bool = crate::catalog::IMAGE_AVAILABLE;

pub async fn setup_status(
    store: Arc<crate::setup::SetupStore>,
    vm: &LocalVm,
) -> Result<crate::setup::SetupSnapshot, crate::setup::SetupError> {
    Ok(vm.snapshot(store.status_async().await?))
}
pub async fn select_local(
    store: Arc<crate::setup::SetupStore>,
    vm: &Arc<LocalVm>,
) -> Result<crate::setup::SetupSnapshot, crate::setup::SetupError> {
    let _choice = vm.choice.lock().await;
    let selected = store.select_local_async().await?;
    vm.start();
    Ok(vm.snapshot(selected))
}
pub async fn cancel_local(
    store: Arc<crate::setup::SetupStore>,
    vm: &Arc<LocalVm>,
) -> Result<crate::setup::SetupSnapshot, crate::setup::SetupError> {
    vm.cancel_setup()
        .await
        .map_err(|_| crate::setup::SetupError::model("modelSession"))?;
    setup_status(store, vm).await
}
pub async fn change_local(
    store: Arc<crate::setup::SetupStore>,
    vm: &Arc<LocalVm>,
) -> Result<crate::setup::SetupSnapshot, crate::setup::SetupError> {
    let vm = vm.clone();
    // The confirmed change owns stop + preference removal even if IPC is dropped.
    tokio::spawn(async move {
        let _choice = vm.choice.lock().await;
        vm.cancel()
            .await
            .map_err(|_| crate::setup::SetupError::model("modelSession"))?;
        Ok(vm.snapshot(store.clear_local_async().await?))
    })
    .await
    .map_err(|_| crate::setup::SetupError::model("modelSession"))?
}
pub async fn resume_selected(
    store: Arc<crate::setup::SetupStore>,
    vm: &Arc<LocalVm>,
) -> Result<(), crate::setup::SetupError> {
    let _choice = vm.choice.lock().await;
    if store.status_async().await?.selected.is_some() {
        vm.start();
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalVmStatus {
    NotIntegrated,
    Idle,
    Preparing,
    Installing,
    Starting,
    Ready,
    Closing,
    Closed,
    Cancelled,
    Failed,
}
impl LocalVmStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotIntegrated => "notIntegrated",
            Self::Idle => "idle",
            Self::Preparing => "preparing",
            Self::Installing => "installing",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Closing => "closing",
            Self::Closed => "closed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}
fn failure_reason(error: &RuntimeError) -> String {
    match error {
        RuntimeError::Preflight(failure) => failure.summary().to_owned(),
        RuntimeError::Io(error) if error.kind() == std::io::ErrorKind::StorageFull => {
            format!("NoSpace: {error}")
        }
        RuntimeError::Io(error) => error.to_string(),
        RuntimeError::StopUnconfirmed(detail) => format!("startupStopFailed: {detail}"),
        // Already formatted when retained, including any NoSpace prefix.
        RuntimeError::Startup(context) => context.diagnostic.clone(),
        _ => error.to_string(),
    }
}

pub use crate::setup::{WorkspacePhase, WorkspaceProgress};
#[derive(Clone, Copy, Debug)]
pub enum LocalProgress {
    Status(LocalVmStatus),
    Image(crate::runtime::image_types::ImageProgress),
}
pub type StartFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn LocalSession>, RuntimeError>> + Send + 'a>>;
pub trait LocalFactory: Send + Sync {
    fn launch(
        &self,
        closed: watch::Receiver<bool>,
        progress: Arc<dyn Fn(LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_>;
    /// Refuse invalid replacement before quiescence can discard live creation authority.
    /// Read-only; the upgrade backend must still validate after custody release.
    fn preflight_replacement(&self, _target: &UpgradeTarget) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Drop retained factory handles only after native stop and all queued work joined.
    fn release_stopped(&self) -> Result<(), RuntimeError> {
        Ok(())
    }
    /// Hand a just-created destination's live, new-only creation authority to the
    /// factory that will eventually launch it. Only the process that called
    /// `WorkspaceUpgrade::replace`/`retry` may call this; a later ordinary start
    /// must reuse this exact journal, never reopen the directory from scratch.
    /// Default: no native upgrade backend (debug/dev/unavailable factories).
    fn adopt_destination(
        &self,
        _journal: crate::runtime::journal::Journal,
    ) -> Result<(), RuntimeError> {
        Err(RuntimeError::Unavailable)
    }
}
pub trait LocalSession: Send {
    /// From the installed guest build metadata, never a probing guest request.
    fn run_cancel_supported(&self) -> bool {
        false
    }
    /// Only called under the whole-session blocking custody; no client transfer.
    fn with_client(
        &mut self,
        _work: &mut dyn FnMut(
            bridge_runtime_protocol::Uuid,
            &mut VmClient,
        ) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        Err(RuntimeError::Unavailable)
    }
    fn execute(&mut self, request: EngineRequest) -> TransportFuture<'_>;
    fn stop(&mut self) -> Result<(), RuntimeError>;
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum UpgradeCancellationSource {
    None,
    User,
    Terminal,
}
enum ForwardError {
    Cancellation(RuntimeError, Box<UpgradeStatusDto>),
    Failure(RuntimeError),
}
impl From<RuntimeError> for ForwardError {
    fn from(error: RuntimeError) -> Self {
        Self::Failure(error)
    }
}
impl ForwardError {
    fn from_forward(
        error: RuntimeError,
        owner: &crate::workspace_upgrade::WorkspaceUpgrade,
        source: Option<&native::image::ImageSource>,
    ) -> Self {
        match error {
            RuntimeError::Closed
            | RuntimeError::Image(crate::runtime::image_types::ImageError::Cancelled) => {
                // Local Journals have dropped; retain the worker's transaction lock.
                Self::Cancellation(error, Box::new(owner.status_dto(source)))
            }
            _ => Self::Failure(error),
        }
    }
}
struct UpgradeCancellation {
    signal: watch::Sender<bool>,
    source: UpgradeCancellationSource,
    startup: bool,
}
#[derive(Clone, Copy)]
struct StopOutcome {
    custody_safe: bool,
    upgrade_startup_failed: bool,
}
struct State {
    phase: LocalVmStatus,
    model_connected: bool,
    run_cancel_supported: bool,
    failure: Option<String>,
    preflight_failure: Option<crate::setup::preflight::Failure>,
    workspace_failure: Option<WorkspaceFailure>,
    /// Latest typed progress, captured before failure clears the progress clock.
    operation: WorkspaceOperation,
    closing: bool,
    close_started: bool,
    stopped: Option<bool>,
    startup: Option<tokio::task::JoinHandle<bool>>,
    /// A marked launch refusal is a failure even when no VM needs stopping.
    upgrade_startup_failed: bool,
    terminal: bool,
    cancelled: watch::Sender<bool>,
    completion: watch::Sender<Option<StopOutcome>>,
    progress: Option<ProgressClock>,
    /// One authority interval from forward admission through its marked startup.
    upgrade_cancel: Option<UpgradeCancellation>,
}
struct ProgressClock {
    started: std::time::Instant,
    phase: WorkspacePhase,
    bytes: Option<u64>,
    total: Option<u64>,
    sample: Option<(std::time::Instant, u64)>,
    remaining: Option<u64>,
}
impl ProgressClock {
    fn observe(&mut self, bytes: u64, total: u64, retained: bool, now: std::time::Instant) {
        let advanced = self.bytes != Some(bytes);
        self.bytes = Some(bytes);
        self.total = Some(total);
        self.remaining = None;
        if retained || self.sample.is_none() {
            self.sample = Some((now, bytes));
        } else if let Some((time, previous)) = self.sample {
            // Average fresh movement over the transfer, not a single 64 KiB chunk.
            // A quarter-second sample avoids sub-millisecond events erasing the ETA.
            let nanos = now.saturating_duration_since(time).as_nanos();
            if advanced && bytes > previous && nanos >= 250_000_000 && bytes < total {
                self.remaining = u64::try_from(
                    (total - bytes) as u128 * nanos / (bytes - previous) as u128 / 1_000_000,
                )
                .ok();
            }
        }
    }
    fn snapshot(&self) -> WorkspaceProgress {
        WorkspaceProgress {
            phase: self.phase,
            bytes: self.bytes,
            total_bytes: self.total,
            elapsed_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            remaining_ms: self.remaining,
        }
    }
}
#[cfg(test)]
mod progress_tests {
    use super::*;
    #[test]
    fn full_disk_diagnostic_keeps_the_actionable_category() {
        let error = RuntimeError::Io(std::io::ErrorKind::StorageFull.into());
        assert!(failure_reason(&error).starts_with("NoSpace: "));
    }
    #[test]
    fn startup_snapshot_has_native_elapsed_without_fake_bytes_or_eta() {
        let vm = LocalVm::unavailable();
        vm.inner
            .publish(LocalProgress::Status(LocalVmStatus::Starting));
        let mut state = vm.inner.state.lock().unwrap();
        let progress = state
            .progress
            .as_mut()
            .expect("startup has a monotonic clock");
        progress.started = std::time::Instant::now() - std::time::Duration::from_secs(42);
        let snapshot = serde_json::to_value(progress.snapshot()).unwrap();
        assert_eq!(snapshot["phase"], "starting");
        assert!(snapshot["elapsedMs"].as_u64().unwrap() >= 42000);
        assert!(
            snapshot["bytes"].is_null()
                && snapshot["totalBytes"].is_null()
                && snapshot["remainingMs"].is_null()
        );
    }
    #[test]
    fn eta_stays_useful_for_sub_millisecond_copy_events() {
        let now = std::time::Instant::now();
        let total = 3 * 1024 * 1024 * 1024u64;
        let mut clock = ProgressClock {
            started: now,
            phase: WorkspacePhase::Materializing,
            bytes: None,
            total: None,
            sample: None,
            remaining: None,
        };
        clock.observe(0, total, true, now);
        for i in 1..=20_000u64 {
            clock.observe(
                i * 65536,
                total,
                false,
                now + std::time::Duration::from_micros(i * 160 + 4000),
            );
        }
        let remaining = clock
            .remaining
            .expect("a sustained fast copy has a measured ETA");
        assert!((4600..=4800).contains(&remaining), "{remaining}");
    }
    #[test]
    fn eta_uses_fresh_movement_not_retained_bytes_or_stalled_samples() {
        let now = std::time::Instant::now();
        let mut clock = ProgressClock {
            started: now,
            phase: WorkspacePhase::Materializing,
            bytes: None,
            total: None,
            sample: None,
            remaining: None,
        };
        clock.observe(800, 1000, true, now);
        assert!(clock.remaining.is_none());
        clock.observe(
            850,
            1000,
            false,
            now + std::time::Duration::from_millis(500),
        );
        assert_eq!(clock.remaining, Some(1500));
        clock.observe(
            950,
            1000,
            true,
            now + std::time::Duration::from_millis(1000),
        );
        assert!(clock.remaining.is_none());
        clock.observe(
            975,
            1000,
            false,
            now + std::time::Duration::from_millis(1500),
        );
        assert_eq!(clock.remaining, Some(500));
        clock.observe(
            975,
            1000,
            false,
            now + std::time::Duration::from_millis(2000),
        );
        assert!(clock.remaining.is_none());
        clock.observe(
            1000,
            1000,
            false,
            now + std::time::Duration::from_millis(2500),
        );
        assert!(clock.remaining.is_none());
    }
}
#[cfg(test)]
#[path = "local_vm_model_tests.rs"]
mod model_tests;

struct Inner {
    state: Mutex<State>,
    session: Arc<AsyncMutex<Option<Box<dyn LocalSession>>>>,
}
impl Inner {
    fn model_reset_result(
        &self,
        result: &Result<crate::setup::ModelSetupState, crate::setup::SetupError>,
    ) {
        if result.is_ok() {
            let mut state = self.state.lock().unwrap();
            state.model_connected = false;
            state.failure = None;
        }
    }
    fn model_result(
        &self,
        result: &Result<crate::setup::ModelSetupState, crate::setup::SetupError>,
    ) {
        let mut state = self.state.lock().unwrap();
        if state.phase != LocalVmStatus::Ready || state.closing {
            return;
        }
        state.model_connected = result.as_ref().is_ok_and(|model| model.ready);
        match result {
            Ok(model) if model.ready => state.failure = None,
            Err(error) => {
                // A generic retry refusal must not erase the original guest diagnostic.
                if let Some(detail) = error.model_detail() {
                    state.failure = Some(detail);
                }
            }
            _ => {}
        }
    }
    fn publish(&self, progress: LocalProgress) {
        use crate::runtime::image_types::ImageProgress;
        let mut state = self.state.lock().unwrap();
        if state.closing {
            return;
        }
        match progress {
            LocalProgress::Status(phase) => {
                state.phase = phase;
                if phase == LocalVmStatus::Ready {
                    state.upgrade_cancel = None;
                }
                match phase {
                    LocalVmStatus::Installing => state.operation = WorkspaceOperation::Prepare,
                    LocalVmStatus::Starting => state.operation = WorkspaceOperation::Start,
                    _ => {}
                }
                if phase == LocalVmStatus::Starting {
                    state.progress = Some(ProgressClock {
                        started: std::time::Instant::now(),
                        phase: WorkspacePhase::Starting,
                        bytes: None,
                        total: None,
                        sample: None,
                        remaining: None,
                    });
                } else if !matches!(phase, LocalVmStatus::Preparing | LocalVmStatus::Installing) {
                    state.progress = None;
                }
            }
            LocalProgress::Image(event) => {
                let (phase, count) = match event {
                    ImageProgress::Verifying => (WorkspacePhase::Verifying, None),
                    ImageProgress::Materializing { bytes, total } => {
                        (WorkspacePhase::Materializing, Some((bytes, total, false)))
                    }
                    ImageProgress::Retained { bytes, total } => {
                        (WorkspacePhase::Materializing, Some((bytes, total, true)))
                    }
                    _ => return,
                };
                state.operation = match phase {
                    WorkspacePhase::Verifying => WorkspaceOperation::Verify,
                    _ => WorkspaceOperation::Prepare,
                };
                let now = std::time::Instant::now();
                let clock = state.progress.get_or_insert(ProgressClock {
                    started: now,
                    phase,
                    bytes: None,
                    total: None,
                    sample: None,
                    remaining: None,
                });
                if clock.phase != phase {
                    clock.phase = phase;
                    clock.sample = None;
                    clock.remaining = None;
                }
                if let Some((bytes, total, retained)) = count {
                    clock.observe(bytes, total, retained, now);
                }
            }
        }
    }
    fn fail(
        &self,
        reason: String,
        error: &RuntimeError,
        operation: WorkspaceOperation,
        stop_unconfirmed: bool,
    ) {
        let mut state = self.state.lock().unwrap();
        state.failure = Some(reason);
        state.preflight_failure = match error {
            RuntimeError::Preflight(failure) => Some(*failure),
            _ => None,
        };
        state.workspace_failure = Some(WorkspaceFailure::from_error(error, operation));
        state.progress = None;
        if !state.closing {
            state.phase = LocalVmStatus::Failed;
        }
        state.closing |= stop_unconfirmed;
        if state.upgrade_cancel.as_ref().is_some_and(|c| c.startup) {
            state.upgrade_cancel = None;
        }
    }
    fn ready(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.closing && state.phase == LocalVmStatus::Ready
    }
}
pub struct LocalVm {
    inner: Arc<Inner>,
    factory: Option<Arc<dyn LocalFactory>>,
    /// Only a fresh recovery-blocked owner proves factory custody was never acquired.
    recovery_without_factory: bool,
    recovery_blocked: std::sync::atomic::AtomicBool,
    choice: Arc<AsyncMutex<()>>,
    authoring: Mutex<Arc<AsyncMutex<()>>>,
    upgrading: std::sync::atomic::AtomicBool,
    /// Latches true once this process observes any upgrade-backed destination
    /// or an interrupted/restored transaction. Never cleared within a process:
    /// suppressing automatic first-test replay must survive a later successful
    /// status poll, a cleared failure or a later Restored transition.
    workspace_replaced: std::sync::atomic::AtomicBool,
    /// Last successfully computed status, so a concurrent poll during a busy
    /// mutation still shows real fields with `busy` overlaid, not a blank DTO.
    upgrade_cache: Mutex<Option<crate::workspace_upgrade::UpgradeStatusDto>>,
    /// Compared and advanced only under upgrade_cache; its mutex supplies ordering.
    upgrade_cache_revision: std::sync::atomic::AtomicU64,
    workspace_replaced_observer: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
/// Keep native launch admission and setup choice serialized through the switch.
pub(crate) struct UpgradeQuiescence<'a> {
    vm: &'a LocalVm,
    custody_released: bool,
    _choice: tokio::sync::OwnedMutexGuard<()>,
}
impl Drop for UpgradeQuiescence<'_> {
    fn drop(&mut self) {
        if self.custody_released {
            self.vm.clear_upgrade_busy();
        } else {
            self.vm.quarantine_upgrade();
        }
    }
}
impl LocalVm {
    fn make(factory: Option<Arc<dyn LocalFactory>>, recovery_without_factory: bool) -> Arc<Self> {
        let (cancelled, _) = watch::channel(false);
        let (completion, _) = watch::channel(None);
        Arc::new(Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    phase: if factory.is_some() {
                        LocalVmStatus::Idle
                    } else {
                        LocalVmStatus::NotIntegrated
                    },
                    model_connected: false,
                    run_cancel_supported: false,
                    failure: None,
                    preflight_failure: None,
                    workspace_failure: None,
                    operation: WorkspaceOperation::Verify,
                    closing: false,
                    close_started: false,
                    stopped: None,
                    startup: None,
                    upgrade_startup_failed: false,
                    terminal: false,
                    cancelled,
                    completion,
                    progress: None,
                    upgrade_cancel: None,
                }),
                session: Arc::new(AsyncMutex::new(None)),
            }),
            factory,
            recovery_without_factory,
            recovery_blocked: std::sync::atomic::AtomicBool::new(false),
            choice: Arc::new(AsyncMutex::new(())),
            authoring: Mutex::new(Arc::new(AsyncMutex::new(()))),
            upgrading: std::sync::atomic::AtomicBool::new(false),
            workspace_replaced: std::sync::atomic::AtomicBool::new(false),
            upgrade_cache: Mutex::new(None),
            upgrade_cache_revision: std::sync::atomic::AtomicU64::new(0),
            workspace_replaced_observer: Mutex::new(None),
        })
    }
    pub fn new(factory: Arc<dyn LocalFactory>) -> Arc<Self> {
        Self::make(Some(factory), false)
    }
    pub fn unavailable() -> Arc<Self> {
        Self::make(None, false)
    }
    pub fn recovery_blocked() -> Arc<Self> {
        let vm = Self::make(None, true);
        vm.block_for_recovery();
        vm
    }
    /// Preserve configured custody for explicit retry; never resume ordinary boot.
    pub fn for_recovery(self: Arc<Self>) -> Arc<Self> {
        if self.factory.is_none() {
            Self::recovery_blocked()
        } else {
            self.block_for_recovery();
            self
        }
    }
    /// Retain the configured factory for explicit recovery without resuming startup.
    pub fn block_for_recovery(&self) {
        self.recovery_blocked
            .store(true, std::sync::atomic::Ordering::Release);
        self.set_workspace_replaced(true);
        self.inner.fail(
            "Workspace recovery is required before starting the local cluster.".into(),
            &RuntimeError::RetainedState,
            WorkspaceOperation::Verify,
            false,
        );
    }
    #[cfg(test)]
    pub(crate) async fn quiesce_for_upgrade(&self) -> Result<UpgradeQuiescence<'_>, RuntimeError> {
        let choice = self.choice.clone().lock_owned().await;
        self.quiesce_with_choice(choice).await
    }
    /// Reserve choice before preflight, then retain it through stop and handoff.
    /// A timeout keeps the gate and journal custody quarantined until process exit.
    async fn quiesce_with_choice(
        &self,
        choice: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<UpgradeQuiescence<'_>, RuntimeError> {
        {
            let mut cache = self.upgrade_cache.lock().unwrap();
            self.upgrading
                .store(true, std::sync::atomic::Ordering::Release);
            self.upgrade_cache_revision
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Some(dto) = cache.as_mut() {
                dto.busy = true;
                dto.error = None;
            }
        }
        let mut guard = UpgradeQuiescence {
            vm: self,
            custody_released: false,
            _choice: choice,
        };
        let stopped = tokio::time::timeout(std::time::Duration::from_secs(30), self.stop(false))
            .await
            .unwrap_or_else(|_| {
                Err(RuntimeError::StopUnconfirmed(
                    "Workspace stop did not complete.".into(),
                ))
            });
        stopped?;
        let released = match self.factory.as_ref() {
            Some(factory) => factory.release_stopped(),
            // Only this constructor proves no factory ever acquired handles.
            None if self.recovery_without_factory => Ok(()),
            None => Err(RuntimeError::Unavailable),
        };
        released?;
        guard.custody_released = true;
        Ok(guard)
    }
    fn upgrade_cancellation(&self) -> watch::Receiver<bool> {
        let mut state = self.inner.state.lock().unwrap();
        let (signal, receiver) = watch::channel(state.terminal);
        state.upgrade_cancel = Some(UpgradeCancellation {
            signal,
            source: if state.terminal {
                UpgradeCancellationSource::Terminal
            } else {
                UpgradeCancellationSource::None
            },
            startup: false,
        });
        receiver
    }
    fn with_upgrade_capability(&self, mut dto: UpgradeStatusDto) -> UpgradeStatusDto {
        let state = self.inner.state.lock().unwrap();
        dto.can_cancel = !state.terminal
            && state.stopped != Some(false)
            && state
                .upgrade_cancel
                .as_ref()
                .is_some_and(|c| !c.startup || !state.closing);
        dto
    }
    /// Ordinary setup keeps its generic startup/Ready stop contract. Upgrade
    /// cancellation itself never acquires authority over an unrelated session.
    async fn cancel_setup(&self) -> Result<(), RuntimeError> {
        match self.cancel_workspace_upgrade().await? {
            UpgradeCancelOutcome::NotCancellable => {
                let _choice = self.reserve_upgrade_choice()?;
                self.cancel().await
            }
            UpgradeCancelOutcome::Requested | UpgradeCancelOutcome::Completed => Ok(()),
        }
    }
    fn finish_forward(
        self: &Arc<Self>,
        data: &std::path::Path,
        local: &std::path::Path,
        result: Result<UpgradeStatusDto, ForwardError>,
    ) -> Result<UpgradeActionReply, RuntimeError> {
        // Only cancellation-class errors may be normalized. A bool watch is
        // cooperative transport, not evidence that the user requested Cancel.
        let cancellation_error = matches!(&result, Err(ForwardError::Cancellation(..)));
        let user_cancelled = self
            .inner
            .state
            .lock()
            .unwrap()
            .upgrade_cancel
            .as_ref()
            .is_some_and(|c| c.source == UpgradeCancellationSource::User);
        let result = match result {
            Err(ForwardError::Cancellation(_, status)) if user_cancelled => Ok(*status),
            Err(ForwardError::Cancellation(error, _) | ForwardError::Failure(error)) => Err(error),
            Ok(status) => Ok(status),
        };
        let mut dto = match self.finish_upgrade_status(data, local, result) {
            Ok(dto) => dto,
            Err(error) => {
                self.inner.state.lock().unwrap().upgrade_cancel = None;
                return Err(error);
            }
        };
        if !cancellation_error {
            // Observers run unlocked, while Cancel still owns the same watch.
            self.retire_workspace_state();
        }
        self.clear_upgrade_busy();
        let mut state = self.inner.state.lock().unwrap();
        if state.terminal {
            state.upgrade_cancel = None;
            return Err(RuntimeError::Closed);
        }
        let cancelled = state
            .upgrade_cancel
            .as_ref()
            .is_some_and(|c| c.source == UpgradeCancellationSource::User);
        state.phase = LocalVmStatus::Cancelled;
        state.progress = None;
        if !cancellation_error {
            self.recovery_blocked
                .store(false, std::sync::atomic::Ordering::Release);
        }
        if cancelled {
            state.upgrade_cancel = None;
            return Ok(UpgradeActionReply::Cancelled { status: dto });
        }
        if let Some(cancel) = state.upgrade_cancel.as_mut() {
            cancel.startup = true;
        }
        self.start_locked(&mut state);
        dto.can_cancel = state.upgrade_cancel.is_some();
        Ok(UpgradeActionReply::Completed { status: dto })
    }
    /// Admission and preflight share one reservation, including before spawn.
    fn reserve_upgrade_choice(&self) -> Result<tokio::sync::OwnedMutexGuard<()>, RuntimeError> {
        let choice = self
            .choice
            .clone()
            .try_lock_owned()
            .map_err(|_| RuntimeError::Busy)?;
        if self.inner.state.lock().unwrap().terminal {
            return Err(RuntimeError::Closed);
        }
        Ok(choice)
    }
    /// Denial-only preflight; full mutation authority remains in WorkspaceUpgrade.
    fn validate_upgrade_transaction(
        &self,
        data: &std::path::Path,
        local: &std::path::Path,
        transaction: bridge_runtime_protocol::Uuid,
    ) -> Result<crate::workspace_upgrade::UpgradeStatus, RuntimeError> {
        self.upgrade_idle()?;
        crate::workspace_upgrade::validate_transaction(data, local, transaction)
    }
    fn upgrade_idle(&self) -> Result<(), RuntimeError> {
        if self.upgrading.load(std::sync::atomic::Ordering::Acquire)
            || matches!(
                self.status(),
                LocalVmStatus::Preparing | LocalVmStatus::Installing | LocalVmStatus::Starting
            )
        {
            Err(RuntimeError::Busy)
        } else {
            Ok(())
        }
    }
    /// Recovery never starts A in this app, even after an explicit restore.
    pub async fn restore_workspace(
        &self,
        data: std::path::PathBuf,
        local: std::path::PathBuf,
        transaction: bridge_runtime_protocol::Uuid,
    ) -> Result<crate::workspace_upgrade::UpgradeStatusDto, RuntimeError> {
        let choice = self.reserve_upgrade_choice()?;
        if self.validate_upgrade_transaction(&data, &local, transaction)?
            == crate::workspace_upgrade::UpgradeStatus::Restored
        {
            return Err(RuntimeError::RetainedState);
        }
        let _quiesced = self.quiesce_with_choice(choice).await?;
        let result = (|| {
            let mut owner = crate::workspace_upgrade::WorkspaceUpgrade::open(&data, &local)?;
            owner.restore(transaction)?;
            self.retire_workspace_state();
            self.block_for_recovery();
            Ok(owner.status_dto(native::upgrade_source().as_ref()))
        })();
        self.finish_upgrade_status(&data, &local, result)
    }
    /// Explicit association consent only; restart validated B, never Connect.
    pub async fn transfer_workspace_credential(
        self: &Arc<Self>,
        data: std::path::PathBuf,
        local: std::path::PathBuf,
        transaction: bridge_runtime_protocol::Uuid,
    ) -> Result<crate::workspace_upgrade::UpgradeStatusDto, RuntimeError> {
        let choice = self.reserve_upgrade_choice()?;
        if self.validate_upgrade_transaction(&data, &local, transaction)?
            != crate::workspace_upgrade::UpgradeStatus::Consumed
        {
            return Err(RuntimeError::RetainedState);
        }
        crate::workspace_upgrade::WorkspaceUpgrade::open(&data, &local)?
            .validate_transfer(transaction)?;
        let quiesced = self.quiesce_with_choice(choice).await?;
        let result = (|| {
            let mut owner = crate::workspace_upgrade::WorkspaceUpgrade::open(&data, &local)?;
            owner.transfer_credential(transaction)?;
            Ok(owner.status_dto(native::upgrade_source().as_ref()))
        })();
        let dto = self.finish_upgrade_status(&data, &local, result)?;
        self.clear_upgrade_busy();
        if matches!(dto.state, "fresh" | "consumed") {
            self.start();
        }
        drop(quiesced);
        Ok(dto)
    }
    /// The slot is always native-derived, never a renderer argument.
    pub async fn remove_workspace_credential(
        self: &Arc<Self>,
        data: std::path::PathBuf,
        local: std::path::PathBuf,
        transaction: bridge_runtime_protocol::Uuid,
    ) -> Result<crate::workspace_upgrade::UpgradeStatusDto, RuntimeError> {
        let choice = self.reserve_upgrade_choice()?;
        if !matches!(
            self.validate_upgrade_transaction(&data, &local, transaction)?,
            crate::workspace_upgrade::UpgradeStatus::Consumed
                | crate::workspace_upgrade::UpgradeStatus::Restored
        ) {
            return Err(RuntimeError::RetainedState);
        }
        let slot = crate::workspace_upgrade::WorkspaceUpgrade::open(&data, &local)?
            .resolve_removal_slot(transaction)?;
        let quiesced = self.quiesce_with_choice(choice).await?;
        let result = (|| {
            let mut owner = crate::workspace_upgrade::WorkspaceUpgrade::open(&data, &local)?;
            owner.remove_credential(
                transaction,
                slot,
                &crate::runtime::credential_store::OsCredentialStore,
            )?;
            Ok(owner.status_dto(native::upgrade_source().as_ref()))
        })();
        let dto = self.finish_upgrade_status(&data, &local, result)?;
        self.clear_upgrade_busy();
        if matches!(dto.state, "fresh" | "consumed") {
            self.start();
        }
        drop(quiesced);
        Ok(dto)
    }
    /// Installed once by the native shell; never a renderer-supplied callback.
    pub fn on_workspace_replaced(&self, observer: Arc<dyn Fn() + Send + Sync>) {
        *self.workspace_replaced_observer.lock().unwrap() = Some(observer);
    }
    fn retire_workspace_state(&self) {
        let observer = self.workspace_replaced_observer.lock().unwrap().clone();
        if let Some(observer) = observer {
            observer();
        }
    }
    /// Observe the durable transaction before ordinary startup takes its journal.
    pub fn seed_upgrade_status(&self, data: &std::path::Path, local: &std::path::Path) {
        let dto = crate::workspace_upgrade::upgrade_status(
            data,
            local,
            native::upgrade_source().as_ref(),
        );
        self.set_workspace_replaced(dto.transaction.is_some());
        self.cache_upgrade_status(dto);
    }
    fn quarantine_upgrade(&self) {
        let mut dto = self
            .upgrade_cache
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(crate::workspace_upgrade::UpgradeStatusDto::none);
        dto.state = "conflict";
        dto.error = Some("stopUnconfirmed");
        dto.busy = false;
        dto.can_replace = false;
        dto.can_restore = false;
        self.cache_upgrade_status(dto);
        self.inner.fail(
            "Workspace shutdown or custody release could not be confirmed.".into(),
            &RuntimeError::StopUnconfirmed("Workspace recovery is required.".into()),
            WorkspaceOperation::Stop,
            false,
        );
        // Keep upgrading=true: only process restart may release this quarantine.
    }
    fn cache_upgrade_status(&self, dto: crate::workspace_upgrade::UpgradeStatusDto) {
        let mut cache = self.upgrade_cache.lock().unwrap();
        *cache = Some(dto);
        self.upgrade_cache_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn clear_upgrade_busy(&self) {
        let mut cache = self.upgrade_cache.lock().unwrap();
        self.upgrading
            .store(false, std::sync::atomic::Ordering::Release);
        if let Some(dto) = cache.as_mut() {
            dto.busy = false;
        }
        self.upgrade_cache_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    /// The comparison and publication share the writer's lock. A lifecycle
    /// change or another completed read wins over any earlier observation.
    fn publish_upgrade_status(
        &self,
        observed_revision: u64,
        dto: crate::workspace_upgrade::UpgradeStatusDto,
    ) -> crate::workspace_upgrade::UpgradeStatusDto {
        let mut cache = self.upgrade_cache.lock().unwrap();
        if self
            .upgrade_cache_revision
            .load(std::sync::atomic::Ordering::Relaxed)
            != observed_revision
        {
            let mut current = cache.clone().unwrap_or_else(|| {
                let mut dto = crate::workspace_upgrade::UpgradeStatusDto::none();
                dto.state = "conflict";
                dto
            });
            current.busy = current.error != Some("stopUnconfirmed")
                && self.upgrading.load(std::sync::atomic::Ordering::Acquire);
            return current;
        }
        *cache = Some(dto.clone());
        self.upgrade_cache_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        dto
    }
    fn finish_upgrade_status(
        &self,
        data: &std::path::Path,
        local: &std::path::Path,
        result: Result<crate::workspace_upgrade::UpgradeStatusDto, RuntimeError>,
    ) -> Result<crate::workspace_upgrade::UpgradeStatusDto, RuntimeError> {
        match result {
            Ok(dto) => {
                self.cache_upgrade_status(dto.clone());
                Ok(dto)
            }
            Err(error) => {
                self.inner.fail(
                    failure_reason(&error),
                    &error,
                    WorkspaceOperation::Prepare,
                    false,
                );
                let mut dto = crate::workspace_upgrade::upgrade_status(
                    data,
                    local,
                    native::upgrade_source().as_ref(),
                );
                if dto.busy {
                    dto = self
                        .upgrade_cache
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or_else(crate::workspace_upgrade::UpgradeStatusDto::none);
                }
                dto.busy = false;
                dto.error = crate::workspace_upgrade::classify_error(&error);
                self.cache_upgrade_status(dto);
                Err(error)
            }
        }
    }
    /// Projection is not mutation authority. Our running journal is expected
    /// to exclude Journal::open; it is not evidence of an upgrade in progress.
    pub async fn upgrade_status(
        &self,
        data: std::path::PathBuf,
        local: std::path::PathBuf,
    ) -> crate::workspace_upgrade::UpgradeStatusDto {
        // Snapshot revision and cache together before observing phase or awaiting
        // disk work. No cache guard is retained while acquiring VM state.
        let (revision, cached) = {
            let cache = self.upgrade_cache.lock().unwrap();
            (
                self.upgrade_cache_revision
                    .load(std::sync::atomic::Ordering::Relaxed),
                cache.clone(),
            )
        };
        if self.upgrading.load(std::sync::atomic::Ordering::Acquire) {
            let mut dto = cached.unwrap_or_else(crate::workspace_upgrade::UpgradeStatusDto::none);
            dto.busy = dto.error != Some("stopUnconfirmed");
            return self.with_upgrade_capability(dto);
        }
        let phase = self.status();
        if matches!(
            phase,
            LocalVmStatus::Preparing
                | LocalVmStatus::Installing
                | LocalVmStatus::Starting
                | LocalVmStatus::Ready
        ) {
            let mut dto = cached.unwrap_or_else(crate::workspace_upgrade::UpgradeStatusDto::none);
            dto.busy = dto.state == "fresh" && phase != LocalVmStatus::Ready;
            if phase == LocalVmStatus::Ready && dto.state == "fresh" {
                dto.state = "consumed";
                return self.with_upgrade_capability(self.publish_upgrade_status(revision, dto));
            }
            return self.with_upgrade_capability(dto);
        }
        let source = native::upgrade_source();
        let dto = match tokio::task::spawn_blocking(move || {
            crate::workspace_upgrade::upgrade_status(&data, &local, source.as_ref())
        })
        .await
        {
            Ok(mut dto) => {
                // A failed native launch can still retain our journal.
                if dto.busy {
                    dto = cached.clone().unwrap_or(dto);
                }
                dto.busy = false;
                if let Some(error) = cached.as_ref().and_then(|c| c.error) {
                    dto.error = Some(error);
                }
                dto
            }
            Err(_) => {
                let mut dto =
                    cached.unwrap_or_else(crate::workspace_upgrade::UpgradeStatusDto::none);
                dto.busy = false;
                dto
            }
        };
        self.with_upgrade_capability(self.publish_upgrade_status(revision, dto))
    }
    /// The owned task holds quiescence through the blocking worker AND handoff.
    /// Dropping an IPC waiter cannot detach filesystem work from lifecycle custody.
    pub async fn replace_workspace(
        self: &Arc<Self>,
        data: std::path::PathBuf,
        local: std::path::PathBuf,
        target: UpgradeTarget,
    ) -> Result<UpgradeActionReply, RuntimeError> {
        let choice = self.reserve_upgrade_choice()?;
        self.upgrade_idle()?;
        if self.status() == LocalVmStatus::Ready {
            return Err(RuntimeError::RetainedState);
        }
        crate::workspace_upgrade::validate_new_replacement(&data, &local)?;
        let factory = self.factory.clone().ok_or(RuntimeError::Unavailable)?;
        factory.preflight_replacement(&target)?;
        let rx = self.upgrade_cancellation();
        let vm = self.clone();
        tokio::spawn(async move {
            let quiesced = match vm.quiesce_with_choice(choice).await {
                Ok(quiesced) => quiesced,
                Err(error) => {
                    vm.inner.state.lock().unwrap().upgrade_cancel = None;
                    return Err(error);
                }
            };
            vm.set_workspace_replaced(true);
            let runtime = tokio::runtime::Handle::current();
            let inner = vm.inner.clone();
            inner.publish(LocalProgress::Status(LocalVmStatus::Preparing));
            let (worker_data, worker_local) = (data.clone(), local.clone());
            // Keep a receiver through adoption so an acknowledged Cancel survives handoff.
            let worker_rx = rx.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut owner =
                    crate::workspace_upgrade::WorkspaceUpgrade::open(&worker_data, &worker_local)?;
                owner.set_progress(Arc::new(move |event| {
                    inner.publish(LocalProgress::Image(event))
                }));
                owner
                    .replace(&target.source, target.machine_id, &worker_rx, &runtime)
                    .map_err(|error| {
                        ForwardError::from_forward(error, &owner, Some(&target.source))
                    })?;
                // Status must inspect the original creator Journal before handoff.
                let dto = owner.status_dto(Some(&target.source));
                let journal = owner
                    .take_destination()
                    .ok_or(RuntimeError::RetainedState)?;
                factory.adopt_destination(journal)?;
                Ok(dto)
            })
            .await
            .unwrap_or(Err(ForwardError::Failure(RuntimeError::Unavailable)));
            let reply = vm.finish_forward(&data, &local, result);
            drop(quiesced);
            reply
        })
        .await
        .map_err(|_| RuntimeError::Unavailable)?
    }
    /// Retry creates a new destination, never reopens new-only authority.
    pub async fn retry_workspace(
        self: &Arc<Self>,
        data: std::path::PathBuf,
        local: std::path::PathBuf,
        target: UpgradeTarget,
        transaction: bridge_runtime_protocol::Uuid,
    ) -> Result<UpgradeActionReply, RuntimeError> {
        let choice = self.reserve_upgrade_choice()?;
        if !matches!(
            self.validate_upgrade_transaction(&data, &local, transaction)?,
            crate::workspace_upgrade::UpgradeStatus::SourceSelected
                | crate::workspace_upgrade::UpgradeStatus::SourceRetained
                | crate::workspace_upgrade::UpgradeStatus::Incomplete
        ) {
            return Err(RuntimeError::RetainedState);
        }
        let factory = self.factory.clone().ok_or(RuntimeError::Unavailable)?;
        let rx = self.upgrade_cancellation();
        let vm = self.clone();
        tokio::spawn(async move {
            let quiesced = match vm.quiesce_with_choice(choice).await {
                Ok(quiesced) => quiesced,
                Err(error) => {
                    vm.inner.state.lock().unwrap().upgrade_cancel = None;
                    return Err(error);
                }
            };
            vm.set_workspace_replaced(true);
            let runtime = tokio::runtime::Handle::current();
            let inner = vm.inner.clone();
            inner.publish(LocalProgress::Status(LocalVmStatus::Preparing));
            let (worker_data, worker_local) = (data.clone(), local.clone());
            let worker_rx = rx.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut owner =
                    crate::workspace_upgrade::WorkspaceUpgrade::open(&worker_data, &worker_local)?;
                owner.set_progress(Arc::new(move |event| {
                    inner.publish(LocalProgress::Image(event))
                }));
                owner
                    .retry(
                        transaction,
                        &target.source,
                        target.machine_id,
                        &worker_rx,
                        &runtime,
                    )
                    .map_err(|error| {
                        ForwardError::from_forward(error, &owner, Some(&target.source))
                    })?;
                let dto = owner.status_dto(Some(&target.source));
                let journal = owner
                    .take_destination()
                    .ok_or(RuntimeError::RetainedState)?;
                factory.adopt_destination(journal)?;
                Ok(dto)
            })
            .await
            .unwrap_or(Err(ForwardError::Failure(RuntimeError::Unavailable)));
            let reply = vm.finish_forward(&data, &local, result);
            drop(quiesced);
            reply
        })
        .await
        .map_err(|_| RuntimeError::Unavailable)?
    }
    /// Select authority and mark startup Closing atomically with Ready publication.
    pub async fn cancel_workspace_upgrade(&self) -> Result<UpgradeCancelOutcome, RuntimeError> {
        let completion = {
            let mut state = self.inner.state.lock().unwrap();
            if state.terminal
                || state.stopped == Some(false)
                || (state.closing && state.upgrade_cancel.as_ref().is_some_and(|c| c.startup))
            {
                return Ok(UpgradeCancelOutcome::NotCancellable);
            }
            let Some(cancel) = state.upgrade_cancel.as_mut() else {
                return Ok(UpgradeCancelOutcome::NotCancellable);
            };
            cancel.source = UpgradeCancellationSource::User;
            cancel.signal.send_replace(true);
            if !cancel.startup {
                return Ok(UpgradeCancelOutcome::Requested);
            }
            self.begin_stop(&mut state, false)
        };
        let outcome = Self::join_stop(completion).await?;
        let state = self.inner.state.lock().unwrap();
        if state.terminal {
            return Err(RuntimeError::Closed);
        }
        if outcome.upgrade_startup_failed {
            return Err(RuntimeError::Unavailable);
        }
        Ok(UpgradeCancelOutcome::Completed)
    }
    /// Atomic Run/Chat admission, separate from each bounded VM request.
    pub fn try_authoring(&self) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        self.authoring.lock().unwrap().clone().try_lock_owned().ok()
    }
    /// Reject a captured operation whose admission belongs to an earlier boot.
    pub fn authoring_current(&self, lease: &tokio::sync::OwnedMutexGuard<()>) -> bool {
        Arc::ptr_eq(
            &self.authoring.lock().unwrap(),
            tokio::sync::OwnedMutexGuard::mutex(lease),
        )
    }
    pub fn status(&self) -> LocalVmStatus {
        self.inner.state.lock().unwrap().phase
    }
    /// Native-set only; latches true and never clears within this process.
    /// Main sets this once from startup recovery; later replace/retry/restore
    /// completions in this same process must call it again on success.
    pub fn set_workspace_replaced(&self, replaced: bool) {
        if replaced {
            self.workspace_replaced
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    pub fn workspace_replaced(&self) -> bool {
        self.workspace_replaced
            .load(std::sync::atomic::Ordering::Acquire)
    }
    /// Acknowledgment retained by the native session, independent of renderer reloads.
    pub fn model_connected(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        !state.closing && state.phase == LocalVmStatus::Ready && state.model_connected
    }
    pub fn run_cancel_supported(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        !state.closing && state.phase == LocalVmStatus::Ready && state.run_cancel_supported
    }
    /// A successful guest observation can invalidate an earlier Connect acknowledgment.
    pub fn clear_model_acknowledgment(&self) {
        self.inner.state.lock().unwrap().model_connected = false;
    }
    fn snapshot(&self, mut snapshot: crate::setup::SetupSnapshot) -> crate::setup::SetupSnapshot {
        let state = self.inner.state.lock().unwrap();
        snapshot.workspace_progress = state.progress.as_ref().map(ProgressClock::snapshot);
        snapshot.preflight_failure = if state.phase == LocalVmStatus::Failed {
            state.preflight_failure
        } else {
            None
        };
        snapshot.workspace_failure = if state.phase == LocalVmStatus::Failed {
            state.workspace_failure.clone()
        } else {
            None
        };
        // Publish phase, failure text and model availability from the same observation.
        snapshot
            .with_model_support(MODEL_IMAGE_AVAILABLE)
            .with_local_vm(state.phase.as_str(), state.failure.clone())
            .with_model(
                if state.phase == LocalVmStatus::Ready && MODEL_IMAGE_AVAILABLE {
                    "available"
                } else {
                    "locked"
                },
            )
    }
    pub fn start(self: &Arc<Self>) {
        let mut state = self.inner.state.lock().unwrap();
        self.start_locked(&mut state);
    }
    fn start_locked(self: &Arc<Self>, state: &mut State) {
        if state.terminal
            || self
                .recovery_blocked
                .load(std::sync::atomic::Ordering::Acquire)
            || self.upgrading.load(std::sync::atomic::Ordering::Acquire)
            || state.closing
            || !matches!(
                state.phase,
                LocalVmStatus::Idle | LocalVmStatus::Cancelled | LocalVmStatus::Failed
            )
        {
            return;
        }
        let Some(factory) = self.factory.clone() else {
            return;
        };
        // Unknown work on an old boot must not own a new workspace's admission.
        *self.authoring.lock().unwrap() = Arc::new(AsyncMutex::new(()));
        state.phase = LocalVmStatus::Preparing;
        state.model_connected = false;
        state.run_cancel_supported = false;
        state.failure = None;
        state.preflight_failure = None;
        state.workspace_failure = None;
        state.upgrade_startup_failed = false;
        state.operation = WorkspaceOperation::Verify;
        state.close_started = false;
        state.stopped = None;
        state.progress = None;
        state.cancelled = watch::channel(false).0;
        state.completion = watch::channel(None).0;
        let cancelled = state.cancelled.subscribe();
        let upgrade_startup = state.upgrade_cancel.as_ref().is_some_and(|c| c.startup);
        let inner = self.inner.clone();
        // Stored before releasing the admission lock. Dropping an IPC waiter does
        // not drop this task or any installation/handshake job it awaits.
        state.startup = Some(tokio::spawn(async move {
            let progress = {
                let i = inner.clone();
                Arc::new(move |phase| i.publish(phase))
            };
            let result = factory.launch(cancelled.clone(), progress).await;
            let cancelled_error = *cancelled.borrow()
                && matches!(
                    &result,
                    Err(RuntimeError::Closed
                        | RuntimeError::Image(crate::runtime::image_types::ImageError::Cancelled))
                );
            // Native preflight/version policy refuses before creating a session.
            // Its retained journal still needs explicit release for replacement.
            let custody_safe = result.is_ok()
                || cancelled_error
                || matches!(&result, Err(RuntimeError::Preflight(_)
                    | RuntimeError::Image(crate::runtime::image_types::ImageError::WorkspaceUpdateRequired
                        | crate::runtime::image_types::ImageError::WorkspaceVersionUnsupported)));
            let mut session = inner.session.lock().await;
            match result {
                Ok(live) => {
                    inner.state.lock().unwrap().run_cancel_supported = live.run_cancel_supported();
                    *session = Some(live);
                    inner.publish(LocalProgress::Status(LocalVmStatus::Ready));
                }
                Err(
                    RuntimeError::Closed
                    | RuntimeError::Image(crate::runtime::image_types::ImageError::Cancelled),
                ) if cancelled_error => {}
                Err(error) => {
                    let operation = {
                        let mut state = inner.state.lock().unwrap();
                        state.upgrade_startup_failed = upgrade_startup;
                        state.operation
                    };
                    inner.fail(
                        format!("Starting the local VM failed: {}", failure_reason(&error)),
                        &error,
                        operation,
                        matches!(error, RuntimeError::StopUnconfirmed(_)),
                    )
                }
            }
            custody_safe
        }));
    }
    /// Native callers keep typed results local; renderer commands wrap fixed work only.
    /// Closing during admitted work returns Closed after completion; effects may have applied.
    pub async fn with_client<T: Send + 'static>(
        &self,
        work: impl FnOnce(bridge_runtime_protocol::Uuid, &mut VmClient) -> T + Send + 'static,
    ) -> Result<T, RuntimeError> {
        if !self.inner.ready() {
            return Err(RuntimeError::Unavailable);
        }
        let mut session = self
            .inner
            .session
            .clone()
            .try_lock_owned()
            .map_err(|_| RuntimeError::Busy)?;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            if !inner.ready() {
                return Err(RuntimeError::Closed);
            }
            let mut work = Some(work);
            let mut result = None;
            session
                .as_mut()
                .ok_or(RuntimeError::Unavailable)?
                .with_client(&mut |id, client| {
                    let work = work.take().ok_or(RuntimeError::Unavailable)?;
                    result = Some(work(id, client));
                    Ok(())
                })?;
            if !inner.ready() {
                return Err(RuntimeError::Closed);
            }
            result.ok_or(RuntimeError::Unavailable)
        })
        .await
        .map_err(|_| RuntimeError::Unavailable)?
    }
    pub async fn workspace_catalog(
        &self,
        setup: Arc<crate::setup::SetupStore>,
    ) -> Result<crate::catalog::CatalogReply, crate::catalog::CatalogError> {
        use crate::catalog::CatalogError;
        if !crate::catalog::IMAGE_AVAILABLE {
            return Err(CatalogError::CatalogUnavailable);
        }
        let failure = CatalogError::CatalogUnavailable;
        if !self.model_connected() {
            return Err(failure);
        }
        let result = self
            .with_client(move |id, client| setup.workspace_catalog(id, client))
            .await
            .map_err(CatalogError::from)?
            .map_err(|_| CatalogError::CatalogUnavailable)?;
        if !self.model_connected() {
            return Err(failure);
        }
        Ok(result)
    }
    pub async fn connect_model(
        &self,
        setup: Arc<crate::setup::SetupStore>,
    ) -> Result<crate::setup::ModelSetupState, crate::setup::SetupError> {
        self.connect_model_choice(setup, None).await
    }
    pub async fn connect_saved_key_model(
        &self,
        setup: Arc<crate::setup::SetupStore>,
        vendor: bridge_runtime_protocol::ModelVendor,
    ) -> Result<crate::setup::ModelSetupState, crate::setup::SetupError> {
        self.connect_model_choice(setup, Some(vendor)).await
    }
    async fn connect_model_choice(
        &self,
        setup: Arc<crate::setup::SetupStore>,
        vendor: Option<bridge_runtime_protocol::ModelVendor>,
    ) -> Result<crate::setup::ModelSetupState, crate::setup::SetupError> {
        if !MODEL_IMAGE_AVAILABLE {
            return Err(crate::setup::SetupError::model("modelGuest"));
        }
        let runtime = tokio::runtime::Handle::current();
        let inner = self.inner.clone();
        self.with_client(move |id, client| {
            let result =
                if let Some(vendor) = vendor {
                    setup.connect_saved_key_model(
                        id,
                        vendor,
                        client,
                        &crate::runtime::credential_store::OsCredentialStore,
                    )
                } else {
                    setup.connect_stored_model(
                    id, client, &crate::runtime::credential_store::OsCredentialStore,
                    || runtime.block_on(crate::runtime::credentials::capture_stored_github_token()),
                )
                };
            // The owned worker retains details even if its IPC waiter is dropped.
            inner.model_result(&result);
            result
        })
        .await
        .map_err(|error| {
            crate::setup::SetupError::model(if matches!(error, RuntimeError::Busy) {
                "modelBusy"
            } else {
                "modelSession"
            })
        })?
    }
    pub async fn connect_key_model(
        &self,
        setup: Arc<crate::setup::SetupStore>,
        vendor: bridge_runtime_protocol::ModelVendor,
        key: String,
    ) -> Result<crate::setup::ModelSetupState, crate::setup::SetupError> {
        if !MODEL_IMAGE_AVAILABLE {
            return Err(crate::setup::SetupError::model("modelGuest"));
        }
        let inner = self.inner.clone();
        self.with_client(move |id, client| {
            let result = setup.connect_key_model(
                id,
                vendor,
                &key,
                client,
                &crate::runtime::credential_store::OsCredentialStore,
            );
            // As with Copilot, the owned worker outlives a dropped IPC waiter.
            inner.model_result(&result);
            result
        })
        .await
        .map_err(|error| {
            crate::setup::SetupError::model(if matches!(error, RuntimeError::Busy) {
                "modelBusy"
            } else {
                "modelSession"
            })
        })?
    }
    /// Setup-only preparation shares VM custody with Connect/Remove, never general reconnect.
    pub async fn prepare_example(
        &self,
        setup: Arc<crate::setup::SetupStore>,
        folder: Arc<crate::agent_folder::AgentFolder>,
    ) -> Result<(String, &'static str), &'static str> {
        if !self.model_connected() {
            return Err("Connect the AI before preparing the example.");
        }
        self.with_client(move |_, _| {
            let binding = setup
                .model_binding_status()
                .map_err(|_| "The AI connection is unavailable.")?;
            if binding.unknown {
                return Err("The AI connection is unavailable.");
            }
            let connection = binding
                .connection
                .ok_or("The AI connection is unavailable.")?;
            let pair = crate::agent_folder::example_for_model(&connection.selection.model)
                .ok_or("The example model is unavailable.")?;
            folder.prepare_example_model(&connection.selection.model)?;
            Ok(pair)
        })
        .await
        .map_err(|error| {
            if matches!(error, RuntimeError::Busy) {
                "exampleBusy"
            } else {
                "The local cluster is unavailable."
            }
        })?
    }
    pub async fn reset_model(
        &self,
        setup: Arc<crate::setup::SetupStore>,
    ) -> Result<crate::setup::ModelSetupState, crate::setup::SetupError> {
        let inner = self.inner.clone();
        self.with_client(move |id, _client| {
            let result = setup
                .reset_mismatched_model(id, &crate::runtime::credential_store::OsCredentialStore);
            // A refused reset must not invalidate a newer, current-install connection.
            inner.model_reset_result(&result);
            result
        })
        .await
        .map_err(|error| {
            crate::setup::SetupError::model(if matches!(error, RuntimeError::Busy) {
                "modelBusy"
            } else {
                "modelSession"
            })
        })?
    }
    pub async fn remove_model(
        &self,
        setup: Arc<crate::setup::SetupStore>,
    ) -> Result<crate::setup::ModelSetupState, crate::setup::SetupError> {
        let inner = self.inner.clone();
        self.with_client(move |id, client| {
            let result = setup.sign_out_model(
                id,
                Some(client),
                &crate::runtime::credential_store::OsCredentialStore,
            );
            // Even a failed remove may already have stopped the guest model.
            inner.model_result(&result);
            result
        })
        .await
        .map_err(|error| {
            crate::setup::SetupError::model(if matches!(error, RuntimeError::Busy) {
                "modelBusy"
            } else {
                "modelSession"
            })
        })?
    }
    pub fn request_close(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.terminal = true;
        if state.stopped == Some(true) {
            state.phase = LocalVmStatus::Closed;
        } else if state.stopped.is_none() {
            state.phase = LocalVmStatus::Closing;
        }
        state.closing = true;
        state.progress = None;
        state.cancelled.send_replace(true);
        if let Some(cancel) = state.upgrade_cancel.as_mut() {
            cancel.source = UpgradeCancellationSource::Terminal;
            cancel.signal.send_replace(true);
        }
    }
    pub async fn cancel(&self) -> Result<(), RuntimeError> {
        self.stop(false).await
    }
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        self.request_close();
        // Owned forward work retains choice through filework, adoption and retirement.
        let _choice = self.choice.lock().await;
        self.stop(true).await
    }
    async fn stop(&self, terminal: bool) -> Result<(), RuntimeError> {
        let completion = {
            let mut state = self.inner.state.lock().unwrap();
            self.begin_stop(&mut state, terminal)
        };
        Self::join_stop(completion).await.map(|_| ())
    }
    fn begin_stop(
        &self,
        state: &mut State,
        terminal: bool,
    ) -> watch::Receiver<Option<StopOutcome>> {
        state.terminal |= terminal;
        if !state.close_started {
            state.closing = true;
            state.phase = LocalVmStatus::Closing;
            state.progress = None;
            state.cancelled.send_replace(true);
            state.close_started = true;
            let startup = state.startup.take();
            let inner = self.inner.clone();
            // Custody survives dropped cancel, change and shutdown IPC waiters.
            tokio::spawn(async move {
                let started = match startup {
                    Some(task) => matches!(task.await, Ok(true)),
                    None => true,
                };
                let mut session = inner.session.clone().lock_owned().await;
                let stop_inner = inner.clone();
                let stopped = tokio::task::spawn_blocking(move || {
                    match session.as_mut().map_or(Ok(()), |s| s.stop()) {
                        Ok(()) => {
                            session.take();
                            true
                        }
                        Err(error) => {
                            // Never replace a late session whose native stop failed.
                            stop_inner.fail(
                                format!("Stopping the local VM failed: {}", failure_reason(&error)),
                                &error,
                                WorkspaceOperation::Stop,
                                true,
                            );
                            false
                        }
                    }
                })
                .await
                .unwrap_or(false)
                    && started;
                let mut state = inner.state.lock().unwrap();
                state.phase = if !stopped {
                    LocalVmStatus::Failed
                } else if state.terminal {
                    LocalVmStatus::Closed
                } else if state.upgrade_startup_failed {
                    LocalVmStatus::Failed
                } else {
                    LocalVmStatus::Cancelled
                };
                state.stopped = Some(stopped);
                if state.upgrade_cancel.as_ref().is_some_and(|c| c.startup) {
                    state.upgrade_cancel = None;
                }
                state.closing = state.terminal || !stopped;
                let outcome = StopOutcome {
                    custody_safe: stopped,
                    upgrade_startup_failed: state.upgrade_startup_failed,
                };
                state.completion.send_replace(Some(outcome));
            });
        }
        state.completion.subscribe()
    }
    async fn join_stop(
        mut completion: watch::Receiver<Option<StopOutcome>>,
    ) -> Result<StopOutcome, RuntimeError> {
        loop {
            if let Some(outcome) = *completion.borrow_and_update() {
                return if outcome.custody_safe {
                    Ok(outcome)
                } else {
                    Err(RuntimeError::Unavailable)
                };
            }
            completion
                .changed()
                .await
                .map_err(|_| RuntimeError::Unavailable)?;
        }
    }
}
#[cfg(test)]
#[test]
fn facade_cancel_without_authority_is_not_cancellable_even_with_reserved_choice() {
    let vm = LocalVm::unavailable();
    let choice = vm.reserve_upgrade_choice().unwrap();
    let status = vm.status();
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(vm.cancel_workspace_upgrade());
    drop(choice);
    assert!(matches!(result, Ok(UpgradeCancelOutcome::NotCancellable)));
    assert_eq!(vm.status(), status);
}

#[cfg(test)]
#[test]
fn facade_upgrade_admission_does_not_trust_incomplete_or_absent_cache() {
    for phase in [
        LocalVmStatus::Preparing,
        LocalVmStatus::Installing,
        LocalVmStatus::Starting,
    ] {
        for cached in [false, true] {
            let vm = LocalVm::unavailable();
            vm.inner.state.lock().unwrap().phase = phase;
            if cached {
                let mut dto = crate::workspace_upgrade::UpgradeStatusDto::none();
                dto.state = "incomplete";
                vm.cache_upgrade_status(dto);
            }
            assert!(
                matches!(vm.upgrade_idle(), Err(RuntimeError::Busy)),
                "{phase:?}, cached={cached}"
            );
            assert_eq!(vm.status(), phase);
        }
    }
}

#[cfg(test)]
#[test]
fn facade_stale_status_publication_keeps_winning_fresh_projection() {
    let vm = LocalVm::unavailable();
    let observed = vm
        .upgrade_cache_revision
        .load(std::sync::atomic::Ordering::Relaxed);
    let mut fresh = crate::workspace_upgrade::UpgradeStatusDto::none();
    fresh.state = "fresh";
    vm.cache_upgrade_status(fresh);
    let mut old = crate::workspace_upgrade::UpgradeStatusDto::none();
    old.state = "incomplete";
    let returned = vm.publish_upgrade_status(observed, old);
    assert_eq!(returned.state, "fresh");
    assert_eq!(
        vm.upgrade_cache.lock().unwrap().as_ref().unwrap().state,
        "fresh"
    );
}

#[cfg(test)]
#[test]
fn facade_older_concurrent_status_reader_cannot_overwrite_first_publication() {
    let vm = LocalVm::unavailable();
    let observed = vm
        .upgrade_cache_revision
        .load(std::sync::atomic::Ordering::Relaxed);
    let mut fresh = crate::workspace_upgrade::UpgradeStatusDto::none();
    fresh.state = "fresh";
    assert_eq!(vm.publish_upgrade_status(observed, fresh).state, "fresh");
    let mut old = crate::workspace_upgrade::UpgradeStatusDto::none();
    old.state = "incomplete";
    assert_eq!(vm.publish_upgrade_status(observed, old).state, "fresh");
}

#[cfg(test)]
#[test]
fn facade_status_disk_read_cannot_publish_across_a_cache_mutation() {
    let vm = LocalVm::unavailable();
    let data = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            // Occupy the blocking pool so the real status future must suspend
            // after its snapshot. Channels, not timing, order the mutation.
            let (entered, entered_rx) = tokio::sync::oneshot::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                entered.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            entered_rx.await.unwrap();
            let mut read = Box::pin(vm.upgrade_status(data.path().into(), local.path().into()));
            let pending = std::future::poll_fn(|cx| {
                std::task::Poll::Ready(read.as_mut().poll(cx).is_pending())
            })
            .await;
            let mut fresh = crate::workspace_upgrade::UpgradeStatusDto::none();
            fresh.state = "fresh";
            vm.cache_upgrade_status(fresh);
            release.send(()).unwrap();
            blocker.await.unwrap();
            // Release/join before asserting, including if a regression made
            // the first poll Ready. Never poll that completed future again.
            assert!(
                pending,
                "status read must suspend behind the blocking worker"
            );
            assert_eq!(read.await.state, "fresh");
            assert_eq!(
                vm.upgrade_cache.lock().unwrap().as_ref().unwrap().state,
                "fresh"
            );

            for phase in [
                LocalVmStatus::Preparing,
                LocalVmStatus::Installing,
                LocalVmStatus::Starting,
            ] {
                vm.inner.state.lock().unwrap().phase = phase;
                let dto = vm
                    .upgrade_status(data.path().into(), local.path().into())
                    .await;
                assert_eq!(dto.state, "fresh", "{phase:?}");
                assert!(dto.busy, "{phase:?}");
            }
            // The synchronous Ready projection must use the same publisher.
            vm.inner.state.lock().unwrap().phase = LocalVmStatus::Ready;
            let revision = vm
                .upgrade_cache_revision
                .load(std::sync::atomic::Ordering::Relaxed);
            assert_eq!(
                vm.upgrade_status(data.path().into(), local.path().into())
                    .await
                    .state,
                "consumed"
            );
            assert_ne!(
                vm.upgrade_cache_revision
                    .load(std::sync::atomic::Ordering::Relaxed),
                revision
            );
            assert_eq!(
                vm.upgrade_cache.lock().unwrap().as_ref().unwrap().state,
                "consumed"
            );
        });
}

#[cfg(test)]
#[test]
fn facade_quiescence_invalidates_readers_even_without_a_cached_status() {
    let vm = LocalVm::recovery_blocked();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let before = vm
                .upgrade_cache_revision
                .load(std::sync::atomic::Ordering::Relaxed);
            let guard = vm.quiesce_for_upgrade().await.unwrap();
            let stale = vm
                .publish_upgrade_status(before, crate::workspace_upgrade::UpgradeStatusDto::none());
            assert_eq!(stale.state, "conflict");
            assert!(stale.busy);
            assert!(vm.upgrade_cache.lock().unwrap().is_none());
            let during = vm
                .upgrade_cache_revision
                .load(std::sync::atomic::Ordering::Relaxed);
            drop(guard);
            let stale = vm
                .publish_upgrade_status(during, crate::workspace_upgrade::UpgradeStatusDto::none());
            assert_eq!(stale.state, "conflict");
            assert!(!stale.busy);
            assert!(vm.upgrade_cache.lock().unwrap().is_none());

            let guard = vm.quiesce_for_upgrade().await.unwrap();
            // A mutation may cache its completed DTO before native handoff.
            let mut fresh = crate::workspace_upgrade::UpgradeStatusDto::none();
            fresh.state = "fresh";
            vm.cache_upgrade_status(fresh);
            let stale = vm
                .publish_upgrade_status(before, crate::workspace_upgrade::UpgradeStatusDto::none());
            assert_eq!(stale.state, "fresh");
            assert!(stale.busy);
            drop(guard);
            assert!(!vm.upgrade_cache.lock().unwrap().as_ref().unwrap().busy);

            // Missing factory custody quarantines instead of staying busy forever.
            let quarantined = LocalVm::unavailable();
            assert!(quarantined.quiesce_for_upgrade().await.is_err());
            let stale = quarantined
                .publish_upgrade_status(0, crate::workspace_upgrade::UpgradeStatusDto::none());
            assert_eq!(stale.error, Some("stopUnconfirmed"));
            assert!(!stale.busy);
        });
}

#[cfg(test)]
#[test]
fn facade_forward_cancellation_errors_require_user_provenance() {
    for image_error in [false, true] {
        for source in [
            UpgradeCancellationSource::None,
            UpgradeCancellationSource::User,
            UpgradeCancellationSource::Terminal,
        ] {
            let vm = LocalVm::unavailable();
            let rx = vm.upgrade_cancellation();
            match source {
                UpgradeCancellationSource::None => {}
                UpgradeCancellationSource::User => {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async {
                            assert_eq!(
                                vm.cancel_workspace_upgrade().await.unwrap(),
                                UpgradeCancelOutcome::Requested
                            );
                        });
                }
                UpgradeCancellationSource::Terminal => vm.request_close(),
            }
            assert_eq!(*rx.borrow(), source != UpgradeCancellationSource::None);
            let data = tempfile::tempdir().unwrap();
            let local = tempfile::tempdir().unwrap();
            let error = if image_error {
                RuntimeError::Image(crate::runtime::image_types::ImageError::Cancelled)
            } else {
                RuntimeError::Closed
            };
            let result = vm.finish_forward(
                data.path(),
                local.path(),
                Err(ForwardError::Cancellation(
                    error,
                    Box::new(UpgradeStatusDto::none()),
                )),
            );
            if source == UpgradeCancellationSource::User {
                assert!(
                    matches!(result, Ok(UpgradeActionReply::Cancelled { status }) if !status.can_cancel && status.error.is_none())
                );
            } else {
                assert!(result.is_err());
            }
            assert!(vm.inner.state.lock().unwrap().upgrade_cancel.is_none());
        }
    }
}

#[cfg(test)]
#[test]
fn facade_upgrade_cancel_does_not_acknowledge_failed_startup_or_join() {
    for panics in [false, true] {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let vm = LocalVm::unavailable();
                let _rx = vm.upgrade_cancellation();
                {
                    let mut state = vm.inner.state.lock().unwrap();
                    state.phase = LocalVmStatus::Preparing;
                    state.upgrade_cancel.as_mut().unwrap().startup = true;
                    state.startup = Some(tokio::spawn(async move {
                        assert!(!panics, "startup task failed to join");
                        false
                    }));
                }
                assert!(vm.cancel_workspace_upgrade().await.is_err());
                assert_eq!(vm.status(), LocalVmStatus::Failed);
                assert!(
                    !vm.with_upgrade_capability(UpgradeStatusDto::none())
                        .can_cancel
                );
            });
    }
}

#[cfg(test)]
#[test]
fn facade_cancel_pending_launch_preserves_safe_refusal() {
    struct RefusalFactory {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        version: bool,
    }
    impl LocalFactory for RefusalFactory {
        fn launch(
            &self,
            closed: watch::Receiver<bool>,
            _: Arc<dyn Fn(LocalProgress) + Send + Sync>,
        ) -> StartFuture<'_> {
            Box::pin(async move {
                // The native early cancellation check has already succeeded.
                assert!(!*closed.borrow());
                self.entered.notify_one();
                self.release.notified().await;
                Err(if self.version {
                    RuntimeError::Image(
                        crate::runtime::image_types::ImageError::WorkspaceVersionUnsupported,
                    )
                } else {
                    RuntimeError::Preflight(crate::setup::preflight::Failure::HypervisorUnavailable)
                })
            })
        }
    }
    for version in [false, true] {
        for terminal in [false, true] {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let factory = Arc::new(RefusalFactory {
                        entered: tokio::sync::Notify::new(),
                        release: tokio::sync::Notify::new(),
                        version,
                    });
                    let vm = LocalVm::new(factory.clone());
                    let _rx = vm.upgrade_cancellation();
                    vm.inner
                        .state
                        .lock()
                        .unwrap()
                        .upgrade_cancel
                        .as_mut()
                        .unwrap()
                        .startup = true;
                    vm.start();
                    factory.entered.notified().await;
                    let mut cancel = Box::pin(vm.cancel_workspace_upgrade());
                    let pending = std::future::poll_fn(|cx| {
                        std::task::Poll::Ready(cancel.as_mut().poll(cx).is_pending())
                    })
                    .await;
                    if terminal {
                        vm.request_close();
                    }
                    factory.release.notify_one();
                    let result = if pending {
                        cancel.await
                    } else {
                        panic!("Cancel must join pending launch")
                    };
                    // Failure must not mean a live VM or quarantined custody.
                    let safe = vm.inner.state.lock().unwrap().stopped;
                    let phase = vm.status();
                    let setup_dir = tempfile::tempdir().unwrap();
                    let snapshot = vm.snapshot(
                        crate::setup::SetupStore::new(Some(setup_dir.path().into()))
                            .status()
                            .unwrap(),
                    );
                    vm.shutdown().await.unwrap();
                    assert!(result.is_err(), "launch refusal must not report Completed");
                    assert_eq!(safe, Some(true));
                    if terminal {
                        assert_eq!(phase, LocalVmStatus::Closed);
                    } else {
                        assert_eq!(phase, LocalVmStatus::Failed);
                        assert!(snapshot.workspace_failure.is_some());
                        if !version {
                            assert_eq!(
                                snapshot.preflight_failure,
                                Some(crate::setup::preflight::Failure::HypervisorUnavailable)
                            );
                        }
                    }
                });
        }
    }
}

#[cfg(test)]
#[test]
fn facade_cancelled_forward_keeps_worker_status_under_poll_lock() {
    let data = tempfile::tempdir().unwrap();
    let local = tempfile::tempdir().unwrap();
    let vm = LocalVm::unavailable();
    let _rx = vm.upgrade_cancellation();
    let owner =
        crate::workspace_upgrade::WorkspaceUpgrade::open(data.path(), local.path()).unwrap();
    let result = Err(ForwardError::from_forward(
        RuntimeError::Closed,
        &owner,
        None,
    ));
    vm.inner
        .state
        .lock()
        .unwrap()
        .upgrade_cancel
        .as_mut()
        .unwrap()
        .source = UpgradeCancellationSource::User;
    drop(owner);
    // The operation has ended, but a native poll now owns the transaction lock.
    // No amount of facade cache retention repairs the returned busy placeholder.
    let poll_owner =
        crate::workspace_upgrade::WorkspaceUpgrade::open(data.path(), local.path()).unwrap();
    assert!(crate::workspace_upgrade::upgrade_status(data.path(), local.path(), None).busy);
    let reply = vm
        .finish_forward(data.path(), local.path(), result)
        .unwrap();
    drop(poll_owner);
    let UpgradeActionReply::Cancelled { status } = reply else {
        panic!("expected cancellation")
    };
    assert!(
        !status.busy,
        "reply must carry the worker's native status, not reread under contention"
    );
    assert_eq!(status.state, "none");
}

#[cfg(test)]
mod cancellation_progress_test {
    use super::*;
    use crate::runtime as fixture_runtime;
    mod fixture {
        include!("../tests/support/runtime_image_fixture.rs");
    }
    #[test]
    fn facade_cancelled_forward_clears_copying_clock() {
        let vm = LocalVm::unavailable();
        let _rx = vm.upgrade_cancellation();
        let cache = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let verified = fixture::verified(
            cache.path(),
            crate::runtime::image_types::ImageTarget::WindowsAmd64,
        );
        let compatibility = verified.image().identity().state_compatibility_id.clone();
        let journal = crate::runtime::journal::Journal::create(
            &local.path().join("copy"),
            crate::runtime::types::BackendKind::Qemu,
            vec![1],
        )
        .unwrap();
        let (cancel, closed) = watch::channel(false);
        let copied = std::sync::atomic::AtomicBool::new(false);
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let result = crate::runtime::image_install::PreparedImage::from_verified(verified).unwrap().materialize(
                &journal.image_target(&compatibility).unwrap(), closed, |event| {
                    vm.inner.publish(LocalProgress::Image(event));
                    if matches!(event, crate::runtime::image_types::ImageProgress::Materializing { bytes, .. } if bytes > 0) {
                        copied.store(true, std::sync::atomic::Ordering::SeqCst);
                        cancel.send_replace(true);
                    }
                }).await;
            assert!(matches!(result, Err(crate::runtime::image_types::ImageError::Cancelled)));
        });
        assert!(copied.load(std::sync::atomic::Ordering::SeqCst));
        let store = crate::setup::SetupStore::new(Some(data.path().into()));
        assert!(vm
            .snapshot(store.status().unwrap())
            .workspace_progress
            .is_some());
        vm.inner
            .state
            .lock()
            .unwrap()
            .upgrade_cancel
            .as_mut()
            .unwrap()
            .source = UpgradeCancellationSource::User;
        vm.finish_forward(data.path(), local.path(), Ok(UpgradeStatusDto::none()))
            .unwrap();
        assert_eq!(vm.status(), LocalVmStatus::Cancelled);
        assert!(vm
            .snapshot(store.status().unwrap())
            .workspace_progress
            .is_none());
        assert!(
            vm.inner.state.lock().unwrap().progress.is_none(),
            "no clock remains to accrue elapsed time"
        );
    }
}

#[cfg(test)]
#[test]
fn facade_request_close_cancels_upgrade_before_async_shutdown() {
    let vm = LocalVm::unavailable();
    let (cancel, receiver) = watch::channel(false);
    vm.inner.state.lock().unwrap().upgrade_cancel = Some(UpgradeCancellation {
        signal: cancel,
        source: UpgradeCancellationSource::None,
        startup: false,
    });
    vm.request_close();
    assert!(
        *receiver.borrow(),
        "close must cancel acquisition synchronously"
    );
}

impl EngineTransport for LocalVm {
    fn execute(&self, request: EngineRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            if !self.inner.ready() {
                return Err(EngineError::new(ErrorKind::Unavailable));
            }
            let mut session = self
                .inner
                .session
                .clone()
                .try_lock_owned()
                .map_err(|_| EngineError::new(ErrorKind::Execution))?;
            let inner = self.inner.clone();
            let runtime = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                if !inner.ready() {
                    return Err(EngineError::new(ErrorKind::Unavailable));
                }
                let session = session
                    .as_mut()
                    .ok_or_else(|| EngineError::new(ErrorKind::Unavailable))?;
                // Keep the owned session guard through Mac's existing inner job,
                // even if the renderer cancels its outer request.
                let result = runtime.block_on(session.execute(request));
                if inner.ready() {
                    result
                } else {
                    Err(EngineError::new(ErrorKind::Unavailable))
                }
            })
            .await
            .map_err(|_| EngineError::new(ErrorKind::Execution))?
        })
    }
}
