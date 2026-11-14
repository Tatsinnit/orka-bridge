//! ID-only desktop boundary. Credentials and confirmed access stay in this native owner.
use super::credentials::ExecOwner;
use super::*;
use kube::config::Kubeconfig;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;

const READ_LIMIT: Duration = Duration::from_secs(10);
const UNKNOWN_CONFIGURATION: ClusterFailure = ClusterFailure {
    check: Check::Configuration,
    result: CheckResult::Unknown,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccessStatus {
    Connected,
    NeedServiceAccount,
    Denied,
    Unavailable,
    UnsupportedCredential,
}
#[derive(Serialize)]
pub struct AccessView {
    pub status: AccessStatus,
    pub providers: Vec<ProviderView>,
    /// Null exactly when connected or a ServiceAccount name is needed.
    pub failure: Option<ClusterFailure>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterPlaceSnapshot {
    pub places: Vec<crate::overview_places::Place>,
    pub selected_key: Option<String>,
}
impl From<AccessOutcome> for AccessView {
    fn from(outcome: AccessOutcome) -> Self {
        // Connect tags every failure at its origin; the fallback check is never reached.
        let failure = (outcome != AccessOutcome::NeedServiceAccount)
            .then(|| outcome.failure(Check::ClusterApi));
        Self {
            status: match failure.map(ClusterFailure::legacy) {
                None => AccessStatus::NeedServiceAccount,
                Some(ClusterError::Denied) => AccessStatus::Denied,
                Some(ClusterError::UnsupportedCredential) => AccessStatus::UnsupportedCredential,
                Some(_) => AccessStatus::Unavailable,
            },
            providers: Vec::new(),
            failure,
        }
    }
}
#[derive(Default)]
struct Session {
    client: Option<ClusterClient>,
}
impl Session {
    /// Lazily reads configuration off the executor. A read that misses its budget is dropped,
    /// never published later; the plugin owner and Orka reader outlive every reload.
    async fn client(
        &mut self,
        owner: &ClusterSession,
    ) -> Result<&mut ClusterClient, ClusterFailure> {
        let client = match self.client.take() {
            Some(client) => client,
            None => {
                let kubeconfig = owner.read_configuration(owner.1.clone()).await?;
                ClusterClient::new(kubeconfig, owner.3.clone()).with_exec_owner(owner.2.clone())
            }
        };
        Ok(self.client.insert(client))
    }
}
/// Reads the current kubeconfig for each Send; tests inject fixtures here.
pub type KubeconfigReader = Arc<dyn Fn() -> Result<Kubeconfig, ClusterError> + Send + Sync>;
type ConfirmedAccess = Option<(TargetId, Arc<RemoteAccess>)>;
pub(crate) type SendAccess = (Option<Arc<RemoteAccess>>, KubeconfigReader);
#[derive(Clone)]
pub struct ClusterSession(
    Arc<Mutex<Session>>,
    KubeconfigReader,
    ExecOwner,
    Arc<dyn OrkaRead>,
    // Memory-only admission snapshot, independent of bounded discovery/Use waits.
    Arc<std::sync::Mutex<ConfirmedAccess>>,
    Arc<AtomicBool>,
    Arc<crate::overview_places::Inventory>,
);
impl Default for ClusterSession {
    fn default() -> Self {
        Self(
            Arc::default(),
            Arc::new(read_kubeconfig),
            ExecOwner::default(),
            Arc::new(PodOrkaRead),
            Arc::default(),
            Arc::default(),
            Arc::default(),
        )
    }
}
fn read_kubeconfig() -> Result<Kubeconfig, ClusterError> {
    Kubeconfig::read().map_err(|_| ClusterError::Unavailable)
}
impl ClusterSession {
    /// All configuration reads share one slot, including reads abandoned by Stop or timeout.
    pub(crate) async fn read_configuration(
        &self,
        reader: KubeconfigReader,
    ) -> Result<Kubeconfig, ClusterFailure> {
        if self.5.swap(true, Ordering::SeqCst) {
            return Err(UNKNOWN_CONFIGURATION);
        }
        let running = self.5.clone();
        let (reply, read) = tokio::sync::oneshot::channel();
        // A stuck file read cannot be cancelled. Admit at most one, outside
        // Tokio's blocking pool so it cannot hold up runtime shutdown.
        if std::thread::Builder::new()
            .name("bridge-kubeconfig".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| reader()))
                    .unwrap_or(Err(ClusterError::Unavailable));
                running.store(false, Ordering::SeqCst);
                let _ = reply.send(result);
            })
            .is_err()
        {
            self.5.store(false, Ordering::SeqCst);
            return Err(UNKNOWN_CONFIGURATION);
        }
        // A late completion only releases the slot; it cannot publish configuration.
        tokio::time::timeout(READ_LIMIT, read)
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(Result::ok)
            .ok_or(UNKNOWN_CONFIGURATION)
    }
    pub fn new(client: ClusterClient) -> Self {
        Self::with_reader(client, Arc::new(read_kubeconfig))
    }
    pub fn with_reader(client: ClusterClient, reader: KubeconfigReader) -> Self {
        let (exec, orka) = (client.exec_owner(), client.orka());
        Self(
            Arc::new(Mutex::new(Session {
                client: Some(client),
            })),
            reader,
            exec,
            orka,
            Arc::default(),
            Arc::default(),
            Arc::default(),
        )
    }
    /// Independent of the discovery/Use lock, so close can interrupt a running plugin.
    pub async fn shutdown(&self) {
        self.6.invalidate(None);
        self.2.shutdown().await;
    }
    pub async fn overview(&self) -> Vec<crate::overview_places::Place> {
        self.6.refresh().await
    }
    /// A display-only snapshot: never reads kubeconfig, refreshes inventory or renews access.
    pub fn places(&self) -> ClusterPlaceSnapshot {
        // Do not hold the admission lock while reading the independent inventory.
        let binding = self
            .4
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, access)| access.binding().clone());
        ClusterPlaceSnapshot {
            places: self.6.snapshot(),
            selected_key: binding.map(|binding| crate::overview_places::Place::new(binding).key),
        }
    }
    /// Memory-only lookup; the caller rechecks a fresh kubeconfig before writing.
    pub(crate) fn send_access(&self, target: &str) -> SendAccess {
        let access = self
            .4
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|(id, access)| (id.0.to_string() == target).then(|| access.clone()));
        (access, self.1.clone())
    }
    pub(crate) fn history_access(&self) -> SendAccess {
        (
            self.4
                .lock()
                .unwrap()
                .as_ref()
                .map(|(_, access)| access.clone()),
            self.1.clone(),
        )
    }
    pub async fn contexts(&self) -> Result<Vec<ContextView>, ClusterFailure> {
        let mut session = self.0.lock().await;
        session
            .client(self)
            .await?
            .contexts()
            .map_err(|_| UNKNOWN_CONFIGURATION)
    }
    /// Rereads configuration offline, queued behind any discovery or Use. Old IDs and
    /// access are invalid even if the read fails; a running Send keeps its frozen access.
    pub async fn reload(&self) -> Result<Vec<ContextView>, ClusterFailure> {
        let mut session = self.0.lock().await;
        *session = Session::default();
        self.6.invalidate(None);
        *self.4.lock().unwrap() = None;
        session
            .client(self)
            .await?
            .contexts()
            .map_err(|_| UNKNOWN_CONFIGURATION)
    }
    pub async fn discover(&self, context_id: &str) -> Result<Vec<TargetView>, ClusterFailure> {
        let mut session = self.0.lock().await;
        *self.4.lock().unwrap() = None;
        let client = session.client(self).await?;
        let context = client
            .contexts()
            .map_err(|_| UNKNOWN_CONFIGURATION)?
            .into_iter()
            .find(|c| c.id.0.to_string() == context_id)
            .ok_or(UNKNOWN_CONFIGURATION)?;
        client.discover(context.id).await
    }
    pub async fn use_target(&self, target_id: &str, sa_name: Option<String>) -> AccessView {
        let mut session = self.0.lock().await;
        *self.4.lock().unwrap() = None;
        let id = Uuid::parse_str(target_id).ok().map(TargetId);
        let result = async {
            let client = session.client(self).await.map_err(AccessOutcome::Failed)?;
            let id = id.ok_or(AccessOutcome::Failed(UNKNOWN_CONFIGURATION))?;
            if let Some(binding) = client.target_binding(id) {
                self.6.invalidate(Some(binding));
            }
            client.connect(id, sa_name).await
        }
        .await;
        match result {
            Ok(access) => {
                let view = AccessView {
                    status: AccessStatus::Connected,
                    providers: access.providers().to_vec(),
                    failure: None,
                };
                let access = Arc::new(access);
                self.6.remember(access.clone());
                *self.4.lock().unwrap() = id.map(|id| (id, access));
                view
            }
            Err(outcome) => outcome.into(),
        }
    }
}
