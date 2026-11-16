pub mod catalog;
mod client;
mod credentials;
#[cfg(test)]
mod tests;
pub use credentials::{Credential, ResolvedCreator};
pub mod commands;
mod destination;
mod forward;
pub(crate) use destination::valid_identity;
pub mod preflight;
use bridge_runtime_protocol::Uuid;
pub use client::{ClusterClient, RemoteAccess};
pub use forward::{OrkaStreamTransport, PodOrkaRead};
use serde::Serialize;
use serde_json::Value;
use std::{future::Future, pin::Pin};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub struct ContextId(pub(crate) Uuid);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub struct TargetId(pub(crate) Uuid);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
pub struct ProviderId(pub(crate) Uuid);
#[derive(Clone, Debug, Serialize)]
pub struct ContextView {
    pub id: ContextId,
    pub name: String,
    pub current: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct TargetView {
    pub id: TargetId,
    pub context_id: ContextId,
    pub context_name: String,
    pub service_name: String,
    pub service_namespace: String,
    pub watched_namespace: String,
    pub image: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ProviderView {
    pub id: ProviderId,
    pub name: String,
    pub ready: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetBinding {
    pub cluster_identity: String,
    pub context_id: ContextId,
    pub context_name: String,
    pub server_address: String,
    pub service_name: String,
    pub service_namespace: String,
    pub service_uid: String,
    pub deployment_name: String,
    pub deployment_namespace: String,
    pub deployment_uid: String,
    pub watched_namespace: String,
    pub image: String,
    pub api_port: u16,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForwardPeer {
    pub pod_name: String,
    pub pod_namespace: String,
    pub pod_uid: String,
    pub port: u16,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClusterError {
    Unavailable,
    DestinationMismatch,
    Denied,
    UnsupportedCredential,
    MissingTool,
}
/// The first cluster check that failed. Finite labels only; never a native or server message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Check {
    Configuration,
    Credentials,
    ClusterApi,
    Compatibility,
    ServiceAccount,
    Permissions,
    OrkaIdentity,
    OrkaConnection,
    Agents,
    Providers,
}
/// Unknown covers timeouts and malformed, oversized or unclassifiable replies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckResult {
    Unknown,
    ConnectionFailed,
    AuthenticationRejected,
    PermissionDenied,
    UnsupportedCredential,
    Incompatible,
    IdentityRejected,
    NoProviders,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ClusterFailure {
    pub check: Check,
    pub result: CheckResult,
}
// Use stays in one task. Share its active check through nested read/operation
// timeouts so cancellation cannot erase whether a stream had opened yet.
tokio::task_local! { static ACTIVE_CHECK: std::cell::Cell<Check>; }
impl Check {
    pub(crate) fn begin(self) {
        let _ = ACTIVE_CHECK.try_with(|active| active.set(self));
    }
    pub(crate) fn current(self) -> Self {
        ACTIVE_CHECK.try_with(std::cell::Cell::get).unwrap_or(self)
    }
    pub(crate) fn fail(self, result: CheckResult) -> AccessOutcome {
        AccessOutcome::Failed(ClusterFailure {
            check: self,
            result,
        })
    }
    pub(crate) fn unknown(self) -> AccessOutcome {
        self.fail(CheckResult::Unknown)
    }
}
impl ClusterFailure {
    /// The broad categories Send and preflight already act on.
    pub fn legacy(self) -> ClusterError {
        match self.result {
            CheckResult::AuthenticationRejected
            | CheckResult::PermissionDenied
            | CheckResult::IdentityRejected => ClusterError::Denied,
            CheckResult::UnsupportedCredential => ClusterError::UnsupportedCredential,
            _ => ClusterError::Unavailable,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessOutcome {
    NeedServiceAccount,
    Denied,
    Unavailable,
    UnsupportedCredential,
    Failed(ClusterFailure),
}
impl AccessOutcome {
    /// Keeps a known origin. A broad failure without one is unknown at `check`, never a guessed cause.
    pub(crate) fn at(self, check: Check) -> Self {
        match self {
            Self::UnsupportedCredential => check.fail(CheckResult::UnsupportedCredential),
            Self::Denied | Self::Unavailable => check.unknown(),
            other => other,
        }
    }
    pub(crate) fn failure(self, check: Check) -> ClusterFailure {
        match self.at(check) {
            Self::Failed(failure) => failure,
            _ => ClusterFailure {
                check,
                result: CheckResult::Unknown,
            },
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrkaResource {
    WhoAmI,
    Agents,
    AgentInventory,
    Providers,
    Tools,
}
impl OrkaResource {
    pub(crate) fn check(self) -> Check {
        match self {
            Self::WhoAmI => Check::OrkaIdentity,
            // Tools are read only by Send, whose callers keep the broad category.
            Self::Agents | Self::AgentInventory | Self::Tools => Check::Agents,
            Self::Providers => Check::Providers,
        }
    }
}
/// Native GET-only seam. The implementation owns the selected pod stream, not an arbitrary URL.
pub trait OrkaRead: Send + Sync {
    fn get<'a>(
        &'a self,
        creator: &'a kube::Client,
        binding: &'a TargetBinding,
        bearer: &'a str,
        resource: OrkaResource,
        namespace: &'a str,
        after: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, AccessOutcome>> + Send + 'a>>;
}
