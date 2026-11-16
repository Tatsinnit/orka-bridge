//! App-level custody of one delivered native session; not a second VM lifecycle.
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
mod native;
pub use native::configured;
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
    vm.cancel()
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
    terminal: bool,
    cancelled: watch::Sender<bool>,
    completion: watch::Sender<Option<bool>>,
    progress: Option<ProgressClock>,
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
    }
    fn ready(&self) -> bool {
        let state = self.state.lock().unwrap();
        !state.closing && state.phase == LocalVmStatus::Ready
    }
}
pub struct LocalVm {
    inner: Arc<Inner>,
    factory: Option<Arc<dyn LocalFactory>>,
    choice: AsyncMutex<()>,
    authoring: Mutex<Arc<AsyncMutex<()>>>,
}
impl LocalVm {
    fn make(factory: Option<Arc<dyn LocalFactory>>) -> Arc<Self> {
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
                    terminal: false,
                    cancelled,
                    completion,
                    progress: None,
                }),
                session: Arc::new(AsyncMutex::new(None)),
            }),
            factory,
            choice: AsyncMutex::new(()),
            authoring: Mutex::new(Arc::new(AsyncMutex::new(()))),
        })
    }
    pub fn new(factory: Arc<dyn LocalFactory>) -> Arc<Self> {
        Self::make(Some(factory))
    }
    pub fn unavailable() -> Arc<Self> {
        Self::make(None)
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
        if state.terminal
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
        state.operation = WorkspaceOperation::Verify;
        state.close_started = false;
        state.stopped = None;
        state.progress = None;
        state.cancelled = watch::channel(false).0;
        state.completion = watch::channel(None).0;
        let cancelled = state.cancelled.subscribe();
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
            // Preflight refused before image/session work, so there is nothing to stop.
            let successful = result.is_ok()
                || cancelled_error
                || matches!(&result, Err(RuntimeError::Preflight(_)));
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
                    let operation = inner.state.lock().unwrap().operation;
                    inner.fail(
                        format!("Starting the local VM failed: {}", failure_reason(&error)),
                        &error,
                        operation,
                        matches!(error, RuntimeError::StopUnconfirmed(_)),
                    )
                }
            }
            successful
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
    }
    pub async fn cancel(&self) -> Result<(), RuntimeError> {
        self.stop(false).await
    }
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        self.request_close();
        self.stop(true).await
    }
    async fn stop(&self, terminal: bool) -> Result<(), RuntimeError> {
        let mut completion = {
            let mut state = self.inner.state.lock().unwrap();
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
                                    format!(
                                        "Stopping the local VM failed: {}",
                                        failure_reason(&error)
                                    ),
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
                    } else {
                        LocalVmStatus::Cancelled
                    };
                    state.stopped = Some(stopped);
                    state.closing = state.terminal || !stopped;
                    state.completion.send_replace(Some(stopped));
                });
            }
            state.completion.subscribe()
        };
        loop {
            if let Some(ok) = *completion.borrow_and_update() {
                return if ok {
                    Ok(())
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
