//! Read-only access to the agent a recorded attempt acknowledged. Name, namespace, UID and destination all
//! come from this owner's own history for that Send; the caller supplies only the Send id.
use super::*;

/// What Overview needs to find the exact card: the place key and the recorded agent identity.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFocus {
    pub key: String,
    pub name: String,
    pub namespace: String,
    pub uid: String,
}
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AgentCheck {
    /// The recorded name still resolves to the recorded UID.
    Present,
    /// A confirmed 404, or the name now belongs to a different UID.
    Gone,
    /// Denied, unreachable or unconfirmed access. Never read as deleted.
    CannotCheck,
}
impl SendOwner {
    fn recorded_agent(&self, id: &str) -> Result<(AttemptSummary, String), &'static str> {
        let (summary, _) = self.summary.record(id)?;
        // A first create without an acknowledged agent has nothing to link to.
        let uid = summary.agent_uid.clone().ok_or("notAcknowledged")?;
        Ok((summary, uid))
    }
    pub fn agent_focus(&self, id: &str) -> Result<AgentFocus, &'static str> {
        let (summary, uid) = self.recorded_agent(id)?;
        Ok(AgentFocus {
            key: crate::overview_places::attempt_key(&summary).ok_or("notRecoverable")?,
            name: summary.planned_agent_name,
            namespace: summary.target_uids.watched_namespace,
            uid,
        })
    }
    /// One GET of the recorded agent through a fresh, destination-validated read transport. No mutation.
    pub async fn agent_check(&self, id: &str) -> Result<AgentCheck, &'static str> {
        let (summary, uid) = self.recorded_agent(id)?;
        let (access, reader) = self.session.history_access();
        let Some(access) = access else {
            return Ok(AgentCheck::CannotCheck);
        };
        if binding::target(access.binding()) != summary.target_uids {
            return Ok(AgentCheck::CannotCheck);
        }
        let Ok(kubeconfig) = self.session.read_configuration(reader).await else {
            return Ok(AgentCheck::CannotCheck);
        };
        let fresh = access.fresh_client(kubeconfig);
        let Ok(transport) = access.read_transport(&fresh).await else {
            return Ok(AgentCheck::CannotCheck);
        };
        let found = transport
            .request(
                &format!("agents/{}", summary.planned_agent_name),
                None,
                None,
                transport.deadline(),
            )
            .await;
        transport.close().await;
        Ok(match found {
            // Only a well-formed agent in the recorded namespace and name can say the UID changed.
            Ok(value) => {
                match crate::overview_places::agent(&value, &summary.target_uids.watched_namespace)
                {
                    Ok(seen) if seen.name == summary.planned_agent_name => {
                        if seen.uid == uid {
                            AgentCheck::Present
                        } else {
                            AgentCheck::Gone
                        }
                    }
                    _ => AgentCheck::CannotCheck,
                }
            }
            Err(ErrorKind::NotFound) => AgentCheck::Gone,
            Err(_) => AgentCheck::CannotCheck,
        })
    }
}
