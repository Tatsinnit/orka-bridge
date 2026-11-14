//! Read-only local Overview. Native owners choose files and workspace scope.
use crate::engine::run::Verdict;
use crate::example_run::RunState;
use crate::{
    agent_folder::DocumentId,
    agents::{Agents, InspectedFile},
    engine::{
        self,
        run::ParsedPair,
        types::{AgentSummary, EngineAction, EngineOutput, EngineReply},
    },
    example_run::ExampleRun,
    local_vm::{LocalVm, LocalVmStatus},
    runtime::{client::VmClient, types::RuntimeError},
};
use bridge_runtime_protocol::{SnapshotId, Uuid};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalRunObservation {
    pub installation: Uuid,
    pub document_id: DocumentId,
    pub source: SnapshotId,
    pub namespace: String,
    pub agent_name: String,
    pub agent_uid: String,
    pub observed_at: u64,
    pub state: RunState,
    pub report: Option<ReportSummary>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportSummary {
    pub verdict: Verdict,
    pub passed: usize,
    pub total: usize,
}
impl From<&crate::engine::run::RunReport> for ReportSummary {
    fn from(report: &crate::engine::run::RunReport) -> Self {
        Self {
            verdict: report.verdict,
            passed: report
                .cases
                .iter()
                .filter(|case| case.verdict == Verdict::Passed)
                .count(),
            total: report.cases.len(),
        }
    }
}
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFiles {
    pub state: &'static str,
    pub files: Vec<SavedFile>,
    pub run_active: bool,
    pub history_state: &'static str,
    pub run_agents: Vec<crate::local_results::RunAgent>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFile {
    pub document_id: DocumentId,
    pub name: String,
    pub state: &'static str,
    pub source: Option<SnapshotId>,
    pub summary: Option<DeclaredSummary>,
    pub reason: Option<&'static str>,
    pub local_result: Option<LocalRunObservation>,
}
#[derive(Serialize)]
pub struct DeclaredSummary {
    pub name: Option<String>,
    pub description: Option<String>,
    pub model: String,
    pub tools: Vec<String>,
}
fn summarize(inspection: InspectedFile) -> Option<SavedFile> {
    let pair = inspection.pair;
    if inspection.document.document_id == DocumentId::SAMPLE && matches!(pair, Ok(None)) {
        return None;
    }
    let mut file = SavedFile {
        document_id: inspection.document.document_id,
        name: inspection.document.name,
        state: "unreadable",
        source: None,
        summary: None,
        reason: Some("The saved file is unreadable."),
        local_result: None,
    };
    if let Ok(Some(pair)) = pair {
        file.state = "readable";
        file.source = Some(pair.source);
        file.summary = ParsedPair::parse(pair.agent.as_bytes(), pair.tests.as_bytes())
            .ok()
            .map(|pair| DeclaredSummary {
                name: pair.name().map(str::to_owned),
                description: pair.description().map(str::to_owned),
                model: pair.model().into(),
                tools: pair.tools().to_vec(),
            });
        file.reason = file
            .summary
            .is_none()
            .then_some("The saved declarations are unsupported or invalid.");
    }
    Some(file)
}
pub fn saved_files(agents: &Agents, run_active: bool) -> SavedFiles {
    match agents.inspect_files(summarize) {
        Ok(files) => SavedFiles {
            state: "complete",
            files,
            run_active,
            history_state: "unavailable",
            run_agents: Vec::new(),
        },
        Err(_) => SavedFiles {
            state: "unavailable",
            files: Vec::new(),
            run_active,
            history_state: "unavailable",
            run_agents: Vec::new(),
        },
    }
}

/// Saved local results do not depend on a running VM or live Agent listing.
pub fn saved_files_with_history(agents: &Agents, run: &ExampleRun) -> SavedFiles {
    let mut files = saved_files(agents, run.is_active());
    if let Ok(history) = run.local_results() {
        files.history_state = "complete";
        files.run_agents = history.run_agents;
        for file in &mut files.files {
            file.local_result = history
                .results
                .iter()
                .find(|row| row.document_id == file.document_id)
                .cloned();
        }
    }
    files
}

// The pinned Mac source lacks the state-disk list route. Do not probe by executing it.
pub const IMAGE_AVAILABLE: bool = !cfg!(target_os = "macos");
pub const NAMESPACE: &str = "orka-system";
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAgents {
    pub state: &'static str,
    pub installation: Option<Uuid>,
    pub agents: Vec<AgentSummary>,
    pub observed_at: Option<u64>,
    pub observation: Option<LocalRunObservation>,
    pub run_active: bool,
    pub reason: Option<&'static str>,
}
impl LiveAgents {
    fn unavailable(reason: Option<&'static str>, run_active: bool) -> Self {
        Self {
            state: "unavailable",
            installation: None,
            agents: Vec::new(),
            observed_at: None,
            observation: None,
            run_active,
            reason,
        }
    }
}
fn action(installation: Uuid) -> EngineAction {
    EngineAction::ListAgents {
        context: format!("kind-bridge-{}", installation.simple()),
        namespace: NAMESPACE.into(),
    }
}
fn project(
    installation: Uuid,
    output: EngineOutput,
    observation: Option<LocalRunObservation>,
    run_active: bool,
) -> LiveAgents {
    let Ok(EngineReply::Agents { agents, .. }) = engine::decode(&action(installation), output)
    else {
        return LiveAgents::unavailable(Some("read"), run_active);
    };
    let observation = observation.filter(|o| {
        o.installation == installation
            && agents
                .iter()
                .any(|a| a.namespace == o.namespace && a.uid == o.agent_uid)
    });
    LiveAgents {
        state: "complete",
        installation: Some(installation),
        agents,
        observed_at: Some(now()),
        observation,
        run_active,
        reason: None,
    }
}
fn read(installation: Uuid, client: &mut VmClient) -> std::io::Result<(Uuid, EngineOutput)> {
    let operation = client.execute(action(installation), None)?;
    let result = client.result(operation)?;
    Ok((
        installation,
        EngineOutput {
            exit: result.exit,
            stdout: result.stdout,
            stderr: if result.has_diagnostics {
                vec![1]
            } else {
                Vec::new()
            },
        },
    ))
}
pub async fn live_agents(vm: Arc<LocalVm>, run: Arc<ExampleRun>) -> LiveAgents {
    live_with_support(vm, run, IMAGE_AVAILABLE).await
}
async fn live_with_support(vm: Arc<LocalVm>, run: Arc<ExampleRun>, supported: bool) -> LiveAgents {
    let active = run.is_active();
    if !supported {
        return LiveAgents::unavailable(Some("image"), active);
    }
    if active {
        return failure(RuntimeError::Busy, true);
    }
    if vm.status() != LocalVmStatus::Ready {
        return LiveAgents::unavailable(Some("stopped"), false);
    }
    match vm.with_client(read).await {
        Ok(Ok((installation, output))) => project(
            installation,
            output,
            run.overview_observation(),
            run.is_active(),
        ),
        Ok(Err(_)) => LiveAgents::unavailable(Some("read"), run.is_active()),
        Err(error) => failure(error, run.is_active()),
    }
}
fn failure(error: RuntimeError, run_active: bool) -> LiveAgents {
    let mut reply = LiveAgents::unavailable(Some("read"), run_active);
    if matches!(error, RuntimeError::Busy) {
        reply.state = "busy";
        reply.reason = None;
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_folder::AgentFolder;
    use serde_json::json;
    fn output(metadata: serde_json::Value) -> EngineOutput {
        EngineOutput { exit: 0, stdout: serde_json::to_vec(&json!({"apiVersion":"v1","kind":"List","metadata":metadata,"items":[{"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":{"name":"same-name","namespace":NAMESPACE,"uid":"observed-uid"},"spec":{"secret":"not projected"}}]})).unwrap(), stderr: b"PRIVATE DIAGNOSTIC".to_vec() }
    }
    #[test]
    fn selector_uses_installation_and_configured_namespace() {
        let installation = Uuid::new_v4();
        let EngineAction::ListAgents { context, namespace } = action(installation) else {
            panic!()
        };
        assert_eq!(context, format!("kind-bridge-{}", installation.simple()));
        assert_eq!(namespace, NAMESPACE);
    }
    #[test]
    fn only_session_contention_is_busy() {
        assert_eq!(failure(RuntimeError::Busy, false).state, "busy");
        for error in [RuntimeError::Closed, RuntimeError::Unavailable] {
            assert_eq!(failure(error, true).state, "unavailable");
        }
        let mut bad = output(json!({}));
        bad.exit = 1;
        assert_eq!(
            project(Uuid::new_v4(), bad, None, false).state,
            "unavailable"
        );
    }
    #[test]
    fn continuation_and_truncation_are_unavailable() {
        for metadata in [json!({"continue":"next"}), json!({"remainingItemCount":1})] {
            assert_eq!(
                project(Uuid::new_v4(), output(metadata), None, false).state,
                "unavailable"
            );
        }
        let mut truncated = output(json!({}));
        truncated.stdout.pop();
        assert_eq!(
            project(Uuid::new_v4(), truncated, None, false).state,
            "unavailable"
        );
    }
    #[test]
    fn association_requires_installation_namespace_uid_never_name() {
        let installation = Uuid::new_v4();
        let observation = LocalRunObservation {
            installation,
            document_id: DocumentId::SAMPLE,
            source: SnapshotId::of(b"a", b"b"),
            namespace: NAMESPACE.into(),
            agent_name: "display-only".into(),
            agent_uid: "observed-uid".into(),
            observed_at: 1,
            state: RunState::Complete,
            report: Some(ReportSummary {
                verdict: Verdict::Passed,
                passed: 1,
                total: 1,
            }),
        };
        assert!(project(
            installation,
            output(json!({})),
            Some(observation.clone()),
            false
        )
        .observation
        .is_some());
        let wire = serde_json::to_string(&project(
            installation,
            output(json!({})),
            Some(observation.clone()),
            false,
        ))
        .unwrap();
        assert!(!wire.contains("PRIVATE DIAGNOSTIC"));
        assert!(!wire.contains("not projected"));
        for mismatch in 0..3 {
            let mut other = observation.clone();
            match mismatch {
                0 => other.installation = Uuid::new_v4(),
                1 => other.namespace = "other".into(),
                _ => {
                    other.agent_uid = "recreated-uid".into();
                    other.agent_name = "same-name".into();
                }
            }
            assert!(project(installation, output(json!({})), Some(other), false)
                .observation
                .is_none());
        }
    }
    #[tokio::test]
    async fn old_mac_image_does_not_attempt_live_list_or_start_vm() {
        let vm = LocalVm::unavailable();
        let run = ExampleRun::new(
            tokio::runtime::Handle::current(),
            true,
            Arc::new(AgentFolder::new(None)),
        );
        let reply = live_with_support(vm.clone(), run, false).await;
        assert_eq!(reply.state, "unavailable");
        assert_eq!(reply.reason, Some("image"));
        assert_eq!(vm.status(), LocalVmStatus::NotIntegrated);
        assert!(reply.observed_at.is_none());
    }
}
