use bridge_engine::{
    agent_folder::DocumentId, example_run::RunState, local_results::LocalResults,
    overview::LocalRunObservation,
};
use bridge_runtime_protocol::{SnapshotId, Uuid};
use serde_json::json;

fn observation(document: DocumentId, installation: Uuid, uid: &str) -> LocalRunObservation {
    serde_json::from_value(json!({
        "installation": installation, "documentId": document,
        "source": SnapshotId::of(b"agent", b"tests"), "namespace": "orka-system",
        "agentName": "bridge-run-display-only", "agentUid": uid,
        "observedAt": 1, "state": "complete",
        "report": {"verdict": "failed", "passed": 2, "total": 3}
    }))
    .unwrap()
}

#[test]
fn reopen_keeps_latest_per_document_and_every_acknowledged_identity() {
    let home = tempfile::tempdir().unwrap();
    let root = Some(home.path().join("data"));
    let store = LocalResults::new(root.clone());
    let installation = Uuid::new_v4();
    let a = DocumentId::SAMPLE;
    let b: DocumentId = serde_json::from_value(json!(Uuid::new_v4())).unwrap();
    store
        .record(observation(a, installation, "a-first"))
        .unwrap();
    store
        .record(observation(b, installation, "b-first"))
        .unwrap();
    let mut next = observation(a, installation, "a-second");
    next.source = SnapshotId::of(b"edited", b"tests");
    store.record(next.clone()).unwrap();
    // Same name and UID on another VM is a different ownership tuple.
    store
        .record(observation(b, Uuid::new_v4(), "a-first"))
        .unwrap();
    let snapshot = LocalResults::new(root).list().unwrap();
    assert_eq!(snapshot.results.len(), 2);
    assert_eq!(snapshot.run_agents.len(), 4);
    let saved = snapshot
        .results
        .iter()
        .find(|r| r.document_id == a)
        .unwrap();
    assert_eq!(saved.agent_uid, "a-second");
    assert_eq!(saved.source, next.source);
    let report = saved.report.as_ref().unwrap();
    assert_eq!((report.passed, report.total), (2, 3));
    let wire = serde_json::to_string(&snapshot).unwrap();
    assert!(!wire.contains(home.path().to_str().unwrap()));
    assert!(!wire.contains("systemPrompt"));
    assert!(!wire.contains("answer"));
}

#[test]
fn missing_history_is_empty_and_read_only_but_interrupted_runs_are_unknown() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("data");
    let store = LocalResults::new(Some(path.clone()));
    assert!(store.list().unwrap().results.is_empty());
    assert!(!path.exists());
    let mut row = observation(DocumentId::SAMPLE, Uuid::new_v4(), "run");
    row.state = RunState::Running;
    row.report = None;
    store.record(row).unwrap();
    let saved = LocalResults::new(Some(path)).list().unwrap();
    assert_eq!(saved.results[0].state, RunState::Unknown);
    assert!(saved.results[0].report.is_none());
    assert_eq!(saved.run_agents[0].agent_uid, "run");
}

#[test]
fn malformed_history_is_unavailable_and_not_replaced() {
    for corrupt in [
        b"not json".as_slice(),
        br#"{"version":99,"results":[],"runAgents":[]}"#,
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = LocalResults::new(Some(home.path().into()));
        let row = observation(DocumentId::SAMPLE, Uuid::new_v4(), "run");
        store.record(row.clone()).unwrap();
        let path = home.path().join("local-results-v1/history.json");
        std::fs::write(&path, corrupt).unwrap();
        assert!(store.list().is_err());
        assert!(store.record(row).is_err());
        assert_eq!(std::fs::read(path).unwrap(), corrupt);
    }
}

#[test]
fn invalid_counts_and_idle_state_are_not_saved() {
    let home = tempfile::tempdir().unwrap();
    let store = LocalResults::new(Some(home.path().into()));
    let row = observation(DocumentId::SAMPLE, Uuid::new_v4(), "run");
    let mut bad = row;
    bad.report.as_mut().unwrap().passed = 4;
    assert!(store.record(bad.clone()).is_err());
    bad.report.as_mut().unwrap().passed = 2;
    bad.state = RunState::Idle;
    assert!(store.record(bad.clone()).is_err());
    bad.state = RunState::Complete;
    bad.source.agent = "not-a-hash".into();
    assert!(store.record(bad).is_err());
}

#[test]
fn oversized_and_invalid_ledgers_do_not_turn_into_empty_history() {
    let home = tempfile::tempdir().unwrap();
    let store = LocalResults::new(Some(home.path().into()));
    let row = observation(DocumentId::SAMPLE, Uuid::new_v4(), "run");
    store.record(row.clone()).unwrap();
    let path = home.path().join("local-results-v1/history.json");
    let valid: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut invalid = Vec::new();
    let mut duplicate = valid.clone();
    duplicate["results"]
        .as_array_mut()
        .unwrap()
        .push(valid["results"][0].clone());
    invalid.push(serde_json::to_vec(&duplicate).unwrap());
    let mut counts = valid.clone();
    counts["results"][0]["report"]["passed"] = json!(4);
    invalid.push(serde_json::to_vec(&counts).unwrap());
    let mut ownership = valid;
    ownership["runAgents"] = json!([]);
    invalid.push(serde_json::to_vec(&ownership).unwrap());
    invalid.push(vec![b' '; 16 * 1024 * 1024 + 1]);
    for bytes in invalid {
        std::fs::write(&path, &bytes).unwrap();
        let reopened = LocalResults::new(Some(home.path().into()));
        assert!(reopened.list().is_err());
        assert!(reopened.record(row.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn test_counts_are_cases_not_assertions_and_unknown_cases_are_in_total() {
    use bridge_engine::{
        engine::run::{CaseReport, RunReport, Verdict},
        overview::ReportSummary,
    };
    let cases = [Verdict::Passed, Verdict::Failed, Verdict::Unknown]
        .into_iter()
        .map(|verdict| CaseReport {
            id: "display".into(),
            question: "PRIVATE QUESTION".into(),
            phase: "done".into(),
            answer: Some("PRIVATE ANSWER".into()),
            completed_tools: Vec::new(),
            assertions: Vec::new(),
            verdict,
        })
        .collect();
    let summary = ReportSummary::from(&RunReport {
        cases,
        verdict: Verdict::Failed,
    });
    assert_eq!((summary.passed, summary.total), (1, 3));
    assert_eq!(
        serde_json::to_value(summary).unwrap(),
        json!({"verdict":"failed", "passed":1, "total":3})
    );
}

#[cfg(unix)]
#[test]
fn symlinked_history_is_not_read_or_overwritten() {
    let home = tempfile::tempdir().unwrap();
    let store = LocalResults::new(Some(home.path().into()));
    let row = observation(DocumentId::SAMPLE, Uuid::new_v4(), "run");
    store.record(row.clone()).unwrap();
    let leaf = home.path().join("local-results-v1/history.json");
    let outside = home.path().join("outside");
    std::fs::rename(&leaf, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &leaf).unwrap();
    let before = std::fs::read(&outside).unwrap();
    assert!(store.list().is_err());
    assert!(store.record(row).is_err());
    assert!(std::fs::symlink_metadata(leaf)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(outside).unwrap(), before);
}
