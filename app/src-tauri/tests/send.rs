use bridge_engine::{
    agent_folder::AgentFolder,
    clusters::commands::ClusterSession,
    send::SendOwner,
    send_summary::{AttemptSummary, AttemptVerdict, SummaryStore, TargetUids},
};
use bridge_runtime_protocol::{SnapshotId, Uuid};
use std::sync::Arc;

fn attempt(id: Uuid, verdict: AttemptVerdict, planned: &str) -> AttemptSummary {
    AttemptSummary {
        id: id.to_string(),
        source: SnapshotId::of(b"agent", b"tests"),
        target_display: "team-dev · agents".into(),
        target_uids: TargetUids {
            cluster_identity: Some("a".repeat(64)),
            service_namespace: "control".into(),
            service_uid: "service-1".into(),
            deployment_namespace: "control".into(),
            deployment_uid: "deployment-1".into(),
            watched_namespace: "agents".into(),
        },
        provider: "team-model".into(),
        planned_agent_name: planned.into(),
        agent_uid: None,
        verdict,
        time: 1_762_876_800,
    }
}

#[test]
fn running_reopens_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    store
        .begin(attempt(
            Uuid::new_v4(),
            AttemptVerdict::Running,
            "bridge-run-before-restart",
        ))
        .unwrap();
    let owner = SendOwner::with_history_root(
        Arc::new(AgentFolder::new(None)),
        ClusterSession::default(),
        Some(dir.path().into()),
    );
    let history = owner.history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].verdict, AttemptVerdict::Unknown);
}

#[test]
fn unknown_not_adopted_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let first = attempt(Uuid::new_v4(), AttemptVerdict::Unknown, "bridge-run-same");
    let mut second = attempt(Uuid::new_v4(), AttemptVerdict::Passed, "bridge-run-same");
    second.agent_uid = Some("uid-new".into());
    store.begin(first.clone()).unwrap();
    store.finish(second.clone()).unwrap();
    let owner = SendOwner::with_history_root(
        Arc::new(AgentFolder::new(None)),
        ClusterSession::default(),
        Some(dir.path().into()),
    );
    let history = owner.history().unwrap();
    assert!(history.iter().any(|attempt| attempt.id == first.id
        && attempt.verdict == AttemptVerdict::Unknown
        && attempt.agent_uid.is_none()));
    assert!(history.iter().any(|attempt| attempt.id == second.id
        && attempt.verdict == AttemptVerdict::Passed
        && attempt.agent_uid.as_deref() == Some("uid-new")));
}
