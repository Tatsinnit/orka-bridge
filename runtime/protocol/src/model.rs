use crate::{CredentialState, Uuid, CREDENTIAL_PROFILE, MODEL_API_KEY_PROFILE};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<ModelVendor>,
    pub provider: String,
    pub model: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelVendor {
    Openai,
    Anthropic,
}
impl ModelVendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
        }
    }
}
impl ModelSelection {
    pub fn credential_profile(&self) -> u16 {
        if self.vendor.is_some() {
            MODEL_API_KEY_PROFILE
        } else {
            CREDENTIAL_PROFILE
        }
    }
    pub fn valid(&self) -> bool {
        let p = self.provider.as_bytes();
        !p.is_empty()
            && p.len() <= 45
            && p.first().is_some_and(u8::is_ascii_alphanumeric)
            && p.last().is_some_and(u8::is_ascii_alphanumeric)
            && p.iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            && !self.model.is_empty()
            && self.model.len() <= 256
            && self.model.bytes().all(|b| (b'!'..=b'~').contains(&b))
            && !self.model.eq_ignore_ascii_case("auto")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelPhase {
    Idle,
    Ready,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConnection {
    pub id: Uuid,
    pub selection: ModelSelection,
    pub generation: Uuid,
}
// Only closed diagnostic vocabulary crosses the guest boundary; raw kmx output never does.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelDiagnosticClass {
    Process,
    Timeout,
    OutputLimit,
    InvalidReply,
    Exit,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStage {
    Preflight,
    Gateway,
    Provider,
    Model,
    Finalize,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorCode {
    InvalidArguments,
    ConfigurationInvalid,
    Unsupported,
    ConfirmationRequired,
    CredentialUnavailable,
    CredentialRejected,
    ProviderUnreachable,
    ModelUnavailable,
    PermissionDenied,
    DependencyUnavailable,
    ExecutionUnknown,
    Cancelled,
    DeadlineExceeded,
    OutputFailed,
    OperationFailed,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ModelStderr {
    #[serde(rename = "The command arguments are invalid.")]
    InvalidArguments,
    #[serde(rename = "The configuration is invalid.")]
    ConfigurationInvalid,
    #[serde(rename = "The requested operation is unsupported.")]
    Unsupported,
    #[serde(rename = "Target confirmation is required.")]
    ConfirmationRequired,
    #[serde(rename = "A required credential is unavailable.")]
    CredentialUnavailable,
    #[serde(rename = "The model company rejected the credential.")]
    CredentialRejected,
    #[serde(rename = "The workspace could not reach the model company.")]
    ProviderUnreachable,
    #[serde(rename = "The selected model is not advertised for this credential.")]
    ModelUnavailable,
    #[serde(rename = "The operation was not permitted.")]
    PermissionDenied,
    #[serde(rename = "A required dependency is unavailable.")]
    DependencyUnavailable,
    #[serde(rename = "The execution outcome is unknown.")]
    ExecutionUnknown,
    #[serde(rename = "The operation was cancelled.")]
    Cancelled,
    #[serde(rename = "The operation deadline was exceeded.")]
    DeadlineExceeded,
    #[serde(rename = "Cannot write machine output.")]
    OutputFailed,
    #[serde(rename = "The operation failed.")]
    OperationFailed,
    #[serde(rename = "Stderr withheld.")]
    Withheld,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelDiagnostic {
    pub classification: ModelDiagnosticClass,
    pub elapsed_ms: u64,
    pub exit_code: Option<i32>,
    pub stage: Option<ModelStage>,
    pub error_code: Option<ModelErrorCode>,
    pub stderr: Option<ModelStderr>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelState {
    /// Absence on older images means the retained-cleanup opcode is unsupported.
    #[serde(default, skip_serializing_if = "is_false")]
    pub retained_cleanup: bool,
    pub phase: ModelPhase,
    pub connection: Option<ModelConnection>,
    pub credential: CredentialState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<ModelDiagnostic>,
}
fn is_false(value: &bool) -> bool {
    !*value
}
impl ModelState {
    pub(crate) fn valid(&self) -> bool {
        self.credential.valid()
            && self.connection.as_ref().is_none_or(|c| {
                !c.id.is_nil()
                    && !c.generation.is_nil()
                    && c.selection.valid()
                    && self.credential.generation == Some(c.generation)
            })
            && match self.phase {
                ModelPhase::Idle => self.connection.is_none(),
                ModelPhase::Ready => {
                    self.credential.installed
                        && self.connection.as_ref().is_some_and(|c| {
                            c.selection.credential_profile() == self.credential.profile
                        })
                }
                ModelPhase::Unknown => true,
            }
    }
}
