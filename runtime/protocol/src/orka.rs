use crate::ProtocolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ORKA_NAMESPACE: &str = "orka-system";
/// Serialized create body; a maximal Execute frame stays below MAX_FRAME.
pub const MAX_ORKA_BODY: usize = 64 * 1024;
/// Raw HTTP response body accepted by the guest.
pub const MAX_ORKA_REPLY: usize = 1024 * 1024;

/// One native Run call against the fixed Orka API. The guest owns the
/// endpoint, namespace and credentials; callers name only a Run resource.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrkaRequest {
    pub path: String,
    pub after: Option<u64>,
    pub body: Option<Value>,
}
/// UID-fenced Task status and count of worker pods not yet Succeeded/Failed.
/// Cancellation proof requires both `finished` and `pods == 0`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunWorkerReply {
    pub finished: bool,
    pub pods: u64,
}
pub fn valid_run_worker(name: &str, uid: &str) -> bool {
    generated(name, "bridge-task-") && valid_run_uid(uid)
}
fn valid_run_uid(uid: &str) -> bool {
    !uid.is_empty() && uid.len() <= 64
}
/// Exact HTTP status and bounded JSON body (`null` when absent or not JSON).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OrkaReply {
    pub status: u16,
    pub body: Value,
}
pub(super) fn generated(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|id| {
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
impl OrkaRequest {
    /// Returns whether this is a POST (create or cancel); everything else is a GET.
    pub fn validate(&self) -> Result<bool, ProtocolError> {
        if self.path.len() > 64 {
            return Err(ProtocolError::Invalid);
        }
        let parts: Vec<&str> = self.path.split('/').collect();
        let (post, events) = match parts.as_slice() {
            ["agents"] | ["tasks"] => (true, false),
            ["agents", name] if generated(name, "bridge-run-") => (false, false),
            ["tasks", name] | ["tasks", name, "result"] if generated(name, "bridge-task-") => {
                (false, false)
            }
            ["tasks", name, "events"] if generated(name, "bridge-task-") => (false, true),
            ["tasks", name, "cancel"] if generated(name, "bridge-task-") => {
                if self.after.is_some() {
                    return Err(ProtocolError::Invalid);
                }
                return match &self.body {
                    Some(Value::Object(body))
                        if body.len() == 1
                            && body
                                .get("uid")
                                .and_then(Value::as_str)
                                .is_some_and(valid_run_uid) =>
                    {
                        Ok(true)
                    }
                    _ => Err(ProtocolError::Invalid),
                };
            }
            _ => return Err(ProtocolError::Invalid),
        };
        if self.after.is_some() != events {
            return Err(ProtocolError::Invalid);
        }
        match (&self.body, post) {
            (None, false) => Ok(false),
            (Some(Value::Object(body)), true) => {
                let prefix = if parts[0] == "agents" {
                    "bridge-run-"
                } else {
                    "bridge-task-"
                };
                if body.len() != 3
                    || body.get("namespace").and_then(Value::as_str) != Some(ORKA_NAMESPACE)
                    || !body
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|n| generated(n, prefix))
                    || !body.get("spec").is_some_and(Value::is_object)
                {
                    return Err(ProtocolError::Invalid);
                }
                let size = serde_json::to_vec(body)
                    .map_err(|_| ProtocolError::Invalid)?
                    .len();
                if size > MAX_ORKA_BODY {
                    return Err(ProtocolError::Oversize);
                }
                Ok(true)
            }
            _ => Err(ProtocolError::Invalid),
        }
    }
}
