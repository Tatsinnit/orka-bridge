use crate::{orka::generated, ProtocolError, MAX_ORKA_BODY, ORKA_NAMESPACE};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatMethod {
    Get,
    Post,
    Delete,
}
/// Chat has a separate method/path contract; Run never acquires DELETE authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatRequest {
    pub method: ChatMethod,
    pub path: String,
    pub after: Option<u64>,
    pub body: Option<Value>,
}
impl ChatRequest {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.path.len() > 64 {
            return Err(ProtocolError::Invalid);
        }
        let parts: Vec<_> = self.path.split('/').collect();
        let (create, events) = match (self.method, parts.as_slice()) {
            (ChatMethod::Post, ["agents"] | ["tasks"]) => (true, false),
            (ChatMethod::Get, ["agents", name]) if generated(name, "bridge-chat-agent-") => {
                (false, false)
            }
            (ChatMethod::Get | ChatMethod::Delete, ["tasks", name])
                if generated(name, "bridge-chat-task-") =>
            {
                (false, false)
            }
            (ChatMethod::Get, ["tasks", name, "result"])
                if generated(name, "bridge-chat-task-") =>
            {
                (false, false)
            }
            (ChatMethod::Get, ["tasks", name, "events"])
                if generated(name, "bridge-chat-task-") =>
            {
                (false, true)
            }
            (ChatMethod::Get, ["sessions", name]) if generated(name, "bridge-chat-") => {
                (false, false)
            }
            _ => return Err(ProtocolError::Invalid),
        };
        if self.after.is_some() != events {
            return Err(ProtocolError::Invalid);
        }
        if !create {
            return if self.body.is_none() {
                Ok(())
            } else {
                Err(ProtocolError::Invalid)
            };
        }
        let body = self
            .body
            .as_ref()
            .and_then(Value::as_object)
            .ok_or(ProtocolError::Invalid)?;
        let prefix = if parts[0] == "agents" {
            "bridge-chat-agent-"
        } else {
            "bridge-chat-task-"
        };
        if body.len() != 3
            || body.get("namespace").and_then(Value::as_str) != Some(ORKA_NAMESPACE)
            || !body
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| generated(name, prefix))
            || !body.get("spec").is_some_and(Value::is_object)
        {
            return Err(ProtocolError::Invalid);
        }
        if parts[0] == "tasks" {
            let spec = &body["spec"];
            if spec["type"] != "ai"
                || !spec["agentRef"]["name"]
                    .as_str()
                    .is_some_and(|n| generated(n, "bridge-chat-agent-"))
                || spec["agentRef"]
                    .get("namespace")
                    .is_some_and(|n| n.as_str() != Some(ORKA_NAMESPACE))
                || !spec["sessionRef"]["name"]
                    .as_str()
                    .is_some_and(|n| generated(n, "bridge-chat-"))
            {
                return Err(ProtocolError::Invalid);
            }
        }
        if serde_json::to_vec(body)
            .map_err(|_| ProtocolError::Invalid)?
            .len()
            > MAX_ORKA_BODY
        {
            return Err(ProtocolError::Oversize);
        }
        Ok(())
    }
}
