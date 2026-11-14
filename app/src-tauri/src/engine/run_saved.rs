//! Read-only durable identities. Unlike RunHandle these confer no execution authority.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
fn digest(spec: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(spec).unwrap()))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedResource {
    pub kind: String,
    pub name: String,
    pub namespace: String,
    pub uid: Option<String>,
    pub generation: u64,
    pub spec_digest: String,
    pub terminal: bool,
}
impl SavedResource {
    pub(super) fn planned(kind: &str, name: &str, namespace: &str, spec: &Value) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
            namespace: namespace.into(),
            uid: None,
            generation: 0,
            spec_digest: digest(spec),
            terminal: false,
        }
    }
    pub(super) fn acknowledged(resource: &ResourceId) -> Self {
        let mut saved = Self::planned(
            resource.kind,
            &resource.name,
            &resource.namespace,
            &resource.spec,
        );
        saved.uid = Some(resource.uid.clone());
        saved.generation = resource.generation;
        saved
    }
    fn matches(&self, value: &Value) -> bool {
        value["apiVersion"] == "core.orka.ai/v1alpha1"
            && value["kind"] == self.kind
            && value["metadata"]["name"] == self.name
            && value["metadata"]["namespace"] == self.namespace
            && value["metadata"]["uid"].as_str() == self.uid.as_deref()
            && value["metadata"]["generation"].as_u64().unwrap_or(0) == self.generation
            && digest(&value["spec"]) == self.spec_digest
    }
}
pub struct Recovery {
    pub report: Option<RunReport>,
    pub availability: &'static str,
    pub refreshed: bool,
}
impl NativeRunner<'_> {
    async fn read_saved(
        &self,
        resource: &SavedResource,
        deadline: Instant,
    ) -> Result<Value, ErrorKind> {
        if resource.namespace != self.http.namespace() {
            return Err(ErrorKind::Identity);
        }
        let path = format!(
            "{}/{}",
            if resource.kind == "Agent" {
                "agents"
            } else {
                "tasks"
            },
            resource.name
        );
        let value = self.http.request(&path, None, None, deadline).await?;
        if !resource.matches(&value) {
            return Err(ErrorKind::Identity);
        }
        Ok(value)
    }
    /// A finite GET-only read. This deliberately does not construct a RunHandle.
    pub async fn recover(
        &self,
        resources: &mut [SavedResource],
        cases: &[RunCase],
        previous: Option<RunReport>,
    ) -> Recovery {
        let mut report = previous.unwrap_or(RunReport {
            cases: Vec::new(),
            verdict: Verdict::Unknown,
        });
        let mut availability = "available";
        let mut refreshed = false;
        let mut verified_failure = false;
        let agent = resources.iter().find(|r| r.kind == "Agent").cloned();
        let deadline = self.http.deadline();
        let (_tx, mut stop) = watch::channel(false);
        let mut seen = 0;
        for (task, case) in resources.iter_mut().filter(|r| r.kind == "Task").zip(cases) {
            seen += 1;
            let read=async {
                let agent=agent.as_ref().filter(|a| a.uid.is_some()).ok_or(ErrorKind::Identity)?;
                self.read_saved(agent, deadline).await?;
                if task.namespace!=self.http.namespace() { return Err(ErrorKind::Identity); }
                let path=format!("tasks/{}",task.name);
                let value=self.http.request(&path,None,None,deadline).await?;
                if task.uid.is_none() {
                    // A lost POST response is reconciled only against the captured intended Task spec.
                    let requested=json!({"type":"ai","agentRef":{"name":agent.name},"prompt":case.input(),"timeout":"120s","retryPolicy":{"maxRetries":0}});
                    if digest(&requested)!=task.spec_digest { return Err(ErrorKind::Identity); }
                    let ack=ResourceId::acknowledge(&value,&requested,&task.name,&task.namespace,"Task")?;
                    if value["kind"]!="Task" || value["apiVersion"]!="core.orka.ai/v1alpha1" {return Err(ErrorKind::Identity);}
                    *task=SavedResource::acknowledged(&ack);
                }
                if !task.matches(&value) { return Err(ErrorKind::Identity); }
                let phase=value["status"]["phase"].as_str().ok_or(ErrorKind::Response)?;
                if matches!(phase,"Pending"|"Running"|"Finalizing"|"Scheduled") { task.terminal=false; return Ok(None); }
                if !matches!(phase,"Succeeded"|"Failed"|"Cancelled") { return Err(ErrorKind::Response); }
                task.terminal=true;
                let old=report.cases.iter().find(|r| r.id==case.id);
                let needs_answer=case.assertions().iter().any(|a|matches!(a,Assertion::Contains{..}));
                let answer=match self.http.request(&format!("{path}/result"),None,None,deadline).await {
                    Ok(result)=>result["result"].as_str().map(str::to_owned),
                    Err(error)=> { if error!=ErrorKind::NotFound || needs_answer { availability=if error==ErrorKind::NotFound {"missing"}else{"unavailable"}; } None }
                };
                let captured=answer.is_some();
                if !captured && availability=="available" && needs_answer { availability="missing"; }
                let answer=answer.or_else(|| old.and_then(|r| r.answer.clone()));
                let tools=match self.tools(&task.name,&task.namespace,&agent.name,deadline,&mut stop).await {
                    Ok(tools)=>tools,
                    Err(error)=> { if availability!="missing" { availability=if error==ErrorKind::NotFound {"missing"}else{"unavailable"}; } old.map(|r|r.completed_tools.clone()).unwrap_or_default() }
                };
                let after=self.http.request(&path,None,None,deadline).await?;
                if !task.matches(&after) || after["status"]["phase"]!=phase {return Err(ErrorKind::Identity);}
                self.read_saved(agent, deadline).await?;
                let assertions=if phase=="Cancelled" {Vec::new()} else {case.evaluate(answer.as_deref(),&tools)};
                let verdict=if phase=="Cancelled" {Verdict::Cancelled} else if phase=="Succeeded" {aggregate(assertions.iter().map(|r|r.verdict))} else {Verdict::Failed};
                Ok(Some((CaseReport {id:case.id.clone(),question:case.input().into(),phase:phase.into(),answer,completed_tools:tools,assertions,verdict},captured)))
            }.await;
            match read {
                Ok(None) => (),
                Ok(Some((row, captured))) => {
                    verified_failure |=
                        row.phase == "Failed" || (captured && row.verdict == Verdict::Failed);
                    refreshed |= captured
                        || report
                            .cases
                            .iter()
                            .find(|old| old.id == row.id)
                            .is_none_or(|old| {
                                old.phase != row.phase || old.completed_tools != row.completed_tools
                            });
                    if let Some(old) = report.cases.iter_mut().find(|r| r.id == row.id) {
                        *old = row;
                    } else {
                        report.cases.push(row);
                    }
                }
                Err(ErrorKind::NotFound | ErrorKind::Identity) => availability = "missing",
                Err(_) => {
                    if availability != "missing" {
                        availability = "unavailable"
                    }
                }
            }
        }
        report.verdict = if report.cases.iter().any(|r| r.verdict == Verdict::Cancelled) {
            Verdict::Cancelled
        } else if verified_failure {
            Verdict::Failed
        } else if seen == cases.len()
            && availability == "available"
            && resources
                .iter()
                .filter(|r| r.kind == "Task")
                .all(|r| r.terminal)
        {
            aggregate(report.cases.iter().map(|r| r.verdict))
        } else {
            Verdict::Unknown
        };
        self.close().await;
        Recovery {
            report: (!report.cases.is_empty()).then_some(report),
            availability,
            refreshed,
        }
    }
}
