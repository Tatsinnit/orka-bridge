//! GET-only evidence shared by preview and Start; no model catalogue or Tool copying.
use super::{
    client::{continuation, items},
    *,
};
use crate::engine::run::{ParsedPair, ToolKind};
use bridge_runtime_protocol::SnapshotId;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Allowed,
    Blocked,
    CannotCheck,
    Unverified,
}
#[derive(Serialize)]
pub struct CheckRow {
    pub label: String,
    pub status: CheckStatus,
    pub message: String,
}
impl CheckRow {
    pub(crate) fn new(
        label: impl Into<String>,
        status: CheckStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            status,
            message: message.into(),
        }
    }
    pub(crate) fn permits(&self) -> bool {
        matches!(self.status, CheckStatus::Allowed | CheckStatus::Unverified)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SendAction {
    Create,
    Update,
    Unchanged,
    Blocked,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendCheck {
    pub document_id: crate::agent_folder::DocumentId,
    pub source: Option<SnapshotId>,
    pub target_id: String,
    pub provider_id: String,
    pub model: Option<String>,
    pub rows: Vec<CheckRow>,
    pub can_send: bool,
    pub action: SendAction,
    pub can_separate: bool,
}
impl SendCheck {
    pub(crate) fn finish(mut self) -> Self {
        self.can_send = !self.rows.is_empty() && self.rows.iter().all(CheckRow::permits);
        if !self.can_send {
            self.action = SendAction::Blocked;
        }
        self
    }
}
// Check the fields this native profile actually sends or reads, not an image pin
// or a full-schema equality. Optional source features are not made universal gates.
pub(super) fn supports_schema(resource: &str, schema: &Value) -> bool {
    if schema["type"] != "object" {
        return false;
    }
    let required: &[(&str, &str)] = match resource {
        "agents" => &[
            ("spec.model.name", "string"),
            ("spec.providerRef.name", "string"),
            ("spec.systemPrompt.inline", "string"),
            ("spec.tools[].name", "string"),
        ],
        "tasks" => &[
            ("spec.type", "string"),
            ("spec.agentRef.name", "string"),
            ("spec.prompt", "string"),
            ("spec.timeout", "string"),
            ("spec.retryPolicy.maxRetries", "integer"),
        ],
        "providers" => &[("status.ready", "boolean")],
        // Kubernetes validates ObjectMeta implicitly, even when omitted here.
        "tools" => &[],
        _ => return false,
    };
    required.iter().all(|(path, kind)| {
        let mut node = schema;
        for component in path.split('.') {
            if node["type"] != "object" {
                return false;
            }
            let name = component.strip_suffix("[]").unwrap_or(component);
            node = &node["properties"][name];
            if component.ends_with("[]") {
                if node["type"] != "array" {
                    return false;
                }
                node = &node["items"];
            }
        }
        node["type"] == *kind
    }) && (resource != "tasks"
        || schema["properties"]["spec"]["properties"]["type"]
            .get("enum")
            .is_none_or(|v| v.as_array().is_some_and(|a| a.iter().any(|v| v == "ai"))))
}

#[derive(Default)]
struct Catalogue {
    builtins: HashSet<String>,
    resources: HashSet<String>,
}
impl RemoteAccess {
    pub(crate) async fn check(
        &self,
        stream: &OrkaStreamTransport,
        provider: ProviderId,
        pair: &ParsedPair,
    ) -> Vec<CheckRow> {
        use CheckStatus::*;
        if self.recheck_reads(stream).await.is_err() {
            return vec![CheckRow::new(
                "Access",
                CannotCheck,
                "Bridge cannot check the selected access. Reconnect in Clusters.",
            )];
        }
        let mut rows = vec![CheckRow::new("Access", Allowed, "Read access confirmed.")];
        let selected = self.providers().iter().find(|p| p.id == provider);
        let (status, message) = match self.configured_providers(stream).await {
            Err(_) => (
                CannotCheck,
                "Bridge cannot check the selected AI connection.",
            ),
            Ok(current) => {
                match selected.and_then(|p| current.iter().find(|(name, _)| *name == p.name)) {
                    Some((_, true)) => (Allowed, "The selected AI connection is ready."),
                    Some((_, false)) => (Blocked, "The selected AI connection is not ready."),
                    None => (Blocked, "Select an AI connection in Clusters."),
                }
            }
        };
        rows.push(CheckRow::new("AI connection", status, message));
        rows.push(CheckRow::new(
            "Model",
            Unverified,
            "Model will be checked when tests run.",
        ));
        let mut catalog = Catalogue::default();
        let readable = self.catalogue(stream, pair, &mut catalog).await.is_ok();
        for tool in pair.declared_tools() {
            let builtin = catalog.builtins.contains(&tool.name);
            let resource = catalog.resources.contains(&tool.name);
            let (status, message) = match tool.kind {
                ToolKind::Builtin if builtin => (Allowed, "Built in to this cluster.".into()),
                _ if !readable => (
                    CannotCheck,
                    format!(
                        "Bridge cannot check {} on {}.",
                        tool.name,
                        self.binding().context_name
                    ),
                ),
                ToolKind::Builtin => (
                    Blocked,
                    "This built-in tool is not listed on this cluster.".into(),
                ),
                ToolKind::Resource if !resource => (
                    Blocked,
                    format!(
                        "Missing tool {}; create it on {}.",
                        tool.name,
                        self.binding().context_name
                    ),
                ),
                ToolKind::Resource if builtin => (
                    Unverified,
                    format!(
                        "{} has the same name as a built-in tool; which one runs is not checked.",
                        tool.name
                    ),
                ),
                ToolKind::Resource => (
                    Allowed,
                    "Tool found on this cluster; its endpoint and policy are not checked yet."
                        .into(),
                ),
            };
            rows.push(CheckRow::new(&tool.name, status, message));
        }
        rows
    }
    async fn catalogue(
        &self,
        stream: &OrkaStreamTransport,
        pair: &ParsedPair,
        catalog: &mut Catalogue,
    ) -> Result<(), AccessOutcome> {
        let required = pair.declared_tools();
        let resources = required.iter().any(|t| t.kind == ToolKind::Resource);
        if required.is_empty() {
            return Ok(());
        }
        let mut after = None;
        let mut seen = HashSet::new();
        loop {
            let page = self
                .read_via(stream, OrkaResource::Tools, after.as_deref())
                .await?;
            for item in items(&page)? {
                let name = item["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .ok_or(AccessOutcome::Unavailable)?;
                match item["builtin"].as_bool() {
                    Some(true) => {
                        catalog.builtins.insert(name.into());
                    }
                    Some(false) if !resources => {}
                    Some(false) if item["namespace"] == self.binding().watched_namespace => {
                        catalog.resources.insert(name.into());
                    }
                    _ => return Err(AccessOutcome::Unavailable),
                }
            }
            // Positive built-in evidence needs no unrelated resource pages.
            // Resources exhaust the catalogue, including later collision evidence.
            if !resources && required.iter().all(|t| catalog.builtins.contains(&t.name)) {
                return Ok(());
            }
            after = continuation(&page)?;
            if after.is_none() {
                return Ok(());
            }
            if !seen.insert(after.clone()) {
                return Err(AccessOutcome::Unavailable);
            }
        }
    }
}
