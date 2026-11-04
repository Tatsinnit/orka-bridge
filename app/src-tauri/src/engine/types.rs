use serde::{Deserialize, Serialize};
use std::ffi::OsString;

pub const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngineRequest {
    pub action: EngineAction,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum EngineAction {
    // An empty struct, not a unit variant: serde then enforces unknown-field rejection.
    Version {},
    ListAgents {
        #[serde(deserialize_with = "explicit_scope")]
        context: String,
        #[serde(deserialize_with = "explicit_scope")]
        namespace: String,
    },
}

// An empty selector must not silently become the helper's default scope.
fn explicit_scope<'de, D: serde::Deserializer<'de>>(input: D) -> Result<String, D::Error> {
    let value = String::deserialize(input)?;
    if value.trim().is_empty() {
        return Err(serde::de::Error::custom(
            "An explicit nonempty scope is required.",
        ));
    }
    Ok(value)
}

impl EngineAction {
    pub fn argv(&self) -> Vec<OsString> {
        match self {
            Self::Version { .. } => vec!["version".into()],
            Self::ListAgents { context, namespace } => [
                "cluster",
                "orka",
                "list",
                "--context",
                context,
                "--namespace",
                namespace,
                "-o",
                "json",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
        }
    }
}

#[derive(Debug)]
pub struct EngineOutput {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EngineReply {
    Version {
        version: String,
        has_diagnostics: bool,
    },
    Agents {
        agents: Vec<AgentSummary>,
        has_diagnostics: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AgentSummary {
    pub name: String,
    pub namespace: String,
    pub uid: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    Unavailable,
    Unsupported,
    Execution,
    InvalidResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EngineError {
    pub kind: ErrorKind,
    pub message: &'static str,
}
impl EngineError {
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            message: match kind {
                ErrorKind::Unavailable => "The engine is unavailable in this build.",
                ErrorKind::Unsupported => "This engine response is not supported.",
                ErrorKind::Execution => "The engine request failed.",
                ErrorKind::InvalidResponse => "The engine returned an invalid response.",
            },
        }
    }
    pub fn cancelled() -> Self {
        Self {
            kind: ErrorKind::Execution,
            message: "The engine request was cancelled.",
        }
    }
}
