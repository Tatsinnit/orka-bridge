#[cfg(debug_assertions)]
pub mod process;
pub mod types;
pub mod vm;

use serde::Deserialize;
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::watch;
use types::*;

pub type TransportFuture<'a> =
    Pin<Box<dyn Future<Output = Result<EngineOutput, EngineError>> + Send + 'a>>;
pub trait EngineTransport: Send + Sync {
    fn execute(&self, request: EngineRequest) -> TransportFuture<'_>;
}

pub struct EngineState {
    transport: Arc<dyn EngineTransport>,
    closed: watch::Sender<bool>,
}
impl EngineState {
    pub fn configured() -> Self {
        let (closed, _receiver) = watch::channel(false);
        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("BRIDGE_DEV_KMX_PATH") {
            return Self {
                transport: Arc::new(process::ProcessTransport::new(path.into(), _receiver)),
                closed,
            };
        }
        Self {
            transport: Arc::new(vm::VmTransport::default()),
            closed,
        }
    }
    pub fn shutdown(&self) {
        self.closed.send_replace(true);
    }
    pub async fn call(&self, request: EngineRequest) -> Result<EngineReply, EngineError> {
        let action = request.action.clone();
        let output = self.transport.execute(request).await?;
        decode(&action, output)
    }
}

#[derive(Deserialize)]
struct AgentList {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    metadata: Option<ListMetadata>,
    items: Vec<Agent>,
}
#[derive(Deserialize)]
struct ListMetadata {
    #[serde(rename = "continue")]
    continuation: Option<String>,
    #[serde(rename = "remainingItemCount")]
    remaining: Option<u64>,
}
#[derive(Deserialize)]
struct Agent {
    #[serde(rename = "apiVersion")]
    api_version: String,
    kind: String,
    metadata: Identity,
}
#[derive(Deserialize)]
struct Identity {
    name: String,
    namespace: String,
    uid: String,
}

/// Only a finite projection crosses IPC. Child diagnostics remain in EngineOutput,
/// never in a renderer error or a capability/readiness claim.
pub fn decode(action: &EngineAction, output: EngineOutput) -> Result<EngineReply, EngineError> {
    let invalid = || EngineError::new(ErrorKind::InvalidResponse);
    if output.exit != 0 {
        return Err(EngineError::new(ErrorKind::Execution));
    }
    if output.stdout.len() > MAX_OUTPUT || output.stderr.len() > MAX_OUTPUT {
        return Err(invalid());
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|_| invalid())?;
    let has_diagnostics = !output.stderr.is_empty();
    match action {
        EngineAction::Version { .. } => {
            let mut lines = text.lines();
            let first = lines.next().ok_or_else(invalid)?;
            let mut fields = first
                .strip_prefix("kmx ")
                .ok_or_else(invalid)?
                .split_whitespace();
            let version = fields.next().ok_or_else(invalid)?;
            if version != "dev"
                && semver::Version::parse(version.strip_prefix('v').unwrap_or(version)).is_err()
            {
                return Err(invalid());
            }
            Ok(EngineReply::Version {
                version: version.into(),
                has_diagnostics: has_diagnostics
                    || fields.next().is_some()
                    || lines.next().is_some(),
            })
        }
        EngineAction::ListAgents { .. } => {
            let list: AgentList = serde_json::from_str(text).map_err(|_| invalid())?;
            let typed = list.kind == "AgentList" && list.api_version == "core.orka.ai/v1alpha1";
            // kubectl flattens resource results into the generic v1 List envelope.
            let kubectl = list.kind == "List" && list.api_version == "v1";
            if !typed && !kubectl {
                return Err(EngineError::new(ErrorKind::Unsupported));
            }
            if list.metadata.is_some_and(|m| {
                m.continuation.is_some_and(|s| !s.is_empty()) || m.remaining.unwrap_or(0) > 0
            }) {
                return Err(invalid());
            }
            let mut agents = Vec::with_capacity(list.items.len());
            for item in list.items {
                if item.kind != "Agent"
                    || item.api_version != "core.orka.ai/v1alpha1"
                    || item.metadata.name.is_empty()
                    || item.metadata.namespace.is_empty()
                    || item.metadata.uid.is_empty()
                {
                    return Err(invalid());
                }
                agents.push(AgentSummary {
                    name: item.metadata.name,
                    namespace: item.metadata.namespace,
                    uid: item.metadata.uid,
                });
            }
            Ok(EngineReply::Agents {
                agents,
                has_diagnostics,
            })
        }
    }
}
