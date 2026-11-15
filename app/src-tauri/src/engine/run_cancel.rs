//! One explicit cancellation pass. No submission, deletion, retry or observation loop.
use super::*;
use saved::{cancel_identity, Cancellation};

impl NativeRunner<'_> {
    pub async fn cancel_saved(
        &self,
        resources: &mut [SavedResource],
        cases: &[RunCase],
        checkpoint: &mut (dyn FnMut(&[SavedResource]) -> Result<(), ErrorKind> + Send),
    ) -> Result<Cancellation, ErrorKind> {
        let agent = resources.iter().find(|r| r.kind == "Agent").cloned();
        let indices: Vec<_> = resources
            .iter()
            .enumerate()
            .filter(|(_, r)| r.kind == "Task")
            .map(|(i, _)| i)
            .collect();
        let mut outcomes = Vec::new();
        for (case_index, index) in indices.into_iter().enumerate() {
            if resources[index].terminal {
                continue;
            }
            let deadline = self.http.deadline();
            let checked = async {
                let agent = agent.as_ref().filter(|a| a.uid.is_some()).ok_or(ErrorKind::Identity)?;
                self.read_saved(agent, deadline).await?;
                let task = &mut resources[index];
                let case = cases.get(case_index).ok_or(ErrorKind::Identity)?;
                if task.namespace != self.http.namespace() || !cancel_identity(&task.name, task.uid.as_deref().unwrap_or("pending")) { return Err(ErrorKind::Identity); }
                let value = self.http.request(&format!("tasks/{}",task.name),None,None,deadline).await?;
                if task.uid.is_none() {
                    let intended = json!({"type":"ai","agentRef":{"name":agent.name},"prompt":case.input(),"timeout":"120s","retryPolicy":{"maxRetries":0}});
                    if saved::digest(&intended) != task.spec_digest { return Err(ErrorKind::Identity); }
                    let ack = ResourceId::acknowledge(&value,&intended,&task.name,&task.namespace,"Task")?;
                    if value["kind"] != "Task" || value["apiVersion"] != "core.orka.ai/v1alpha1" { return Err(ErrorKind::Identity); }
                    *task = SavedResource::acknowledged(&ack);
                }
                if !task.matches(&value) { return Err(ErrorKind::Identity); }
                let phase = value["status"]["phase"].as_str().unwrap_or("");
                if matches!(phase,"Succeeded"|"Failed"|"Cancelled") { task.terminal=true; return Ok(true); }
                if !matches!(phase,""|"Pending"|"Scheduled"|"Running"|"Finalizing") { return Err(ErrorKind::Response); }
                Ok(false)
            }.await;
            match checked {
                Ok(true) => continue,
                Err(ErrorKind::Permission | ErrorKind::Authentication) => {
                    outcomes.push(Cancellation::Denied);
                    continue;
                }
                Err(_) => {
                    outcomes.push(Cancellation::Unconfirmed);
                    continue;
                }
                Ok(false) => (),
            }
            // Adopted UID and cancellation intent must reach durable storage before the POST.
            checkpoint(resources)?;
            let task = &mut resources[index];
            let posted = self
                .http
                .cancel_task(&task.name, task.uid.as_deref().unwrap(), deadline)
                .await
                .and_then(|ack| {
                    if ack["name"] != task.name
                        || ack["namespace"] != task.namespace
                        || ack["requested"].as_bool().is_none()
                        || !matches!(
                            ack["phase"].as_str(),
                            Some(
                                "Pending"
                                    | "Scheduled"
                                    | "Running"
                                    | "Finalizing"
                                    | "Succeeded"
                                    | "Failed"
                                    | "Cancelled"
                            )
                        )
                    {
                        Err(ErrorKind::Response)
                    } else {
                        Ok(ack)
                    }
                });
            // An acknowledgement (including 409) is never terminal proof. Read exact identity.
            let read = self.read_saved(task, deadline).await;
            if read.as_ref().is_ok_and(|v| {
                matches!(
                    v["status"]["phase"].as_str(),
                    Some("Succeeded" | "Failed" | "Cancelled")
                )
            }) {
                task.terminal = true;
                continue;
            }
            outcomes.push(match posted {
                Ok(_) if read.is_ok() => Cancellation::Requested,
                Err(ErrorKind::Permission | ErrorKind::Authentication) => Cancellation::Denied,
                Err(ErrorKind::Unsupported) => Cancellation::Unsupported,
                _ => Cancellation::Unconfirmed,
            });
        }
        Ok([
            Cancellation::Unconfirmed,
            Cancellation::Denied,
            Cancellation::Unsupported,
            Cancellation::Requested,
        ]
        .into_iter()
        .find(|o| outcomes.contains(o))
        .unwrap_or(Cancellation::Confirmed))
    }
}
