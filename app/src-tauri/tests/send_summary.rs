use bridge_engine::send_summary::{
    AttemptSummary, AttemptVerdict, SummaryError, SummaryStore, TargetUids,
};
use bridge_runtime_protocol::{SnapshotId, Uuid};

fn summary(id: Uuid, verdict: AttemptVerdict) -> AttemptSummary {
    AttemptSummary {
        id: id.to_string(),
        source: SnapshotId::of(b"agent", b"tests"),
        target_display: "team-dev · agents".into(),
        target_uids: TargetUids {
            service_namespace: "control".into(),
            service_uid: "service-1".into(),
            deployment_namespace: "control".into(),
            deployment_uid: "deployment-1".into(),
            watched_namespace: "agents".into(),
        },
        provider: "team-model".into(),
        planned_agent_name: "bridge-run-before-post".into(),
        agent_uid: None,
        verdict,
        time: 1_762_876_800,
    }
}

#[test]
fn planned_agent_saved_before_post() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let attempt = summary(Uuid::new_v4(), AttemptVerdict::Running);
    store.begin(attempt.clone()).unwrap();
    let history = store.list().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].planned_agent_name, attempt.planned_agent_name);
    assert_eq!(history[0].verdict, AttemptVerdict::Running);
    assert_eq!(history[0].agent_uid, None);
}

#[test]
fn completed_stays_completed() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let id = Uuid::new_v4();
    store.begin(summary(id, AttemptVerdict::Running)).unwrap();
    let mut done = summary(id, AttemptVerdict::Passed);
    done.agent_uid = Some("uid-agent".into());
    store.finish(done.clone()).unwrap();
    let reopened = SummaryStore::new(Some(dir.path().into()));
    assert_eq!(reopened.list().unwrap(), vec![done]);
}

#[test]
fn no_task_handle_or_source_or_token() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut attempt = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
    attempt.provider = "provider-display".into();
    attempt.agent_uid = Some("uid-agent".into());
    store.begin(attempt).unwrap();
    let bytes = std::fs::read(dir.path().join("send-history-v1/history.json")).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains("bridge-task"));
    assert!(!text.contains("token"));
    assert!(!text.contains("Bearer"));
    assert!(!text.contains("systemPrompt"));
}

#[test]
fn missing_native_root_is_visible_not_silently_disabled() {
    let store = SummaryStore::new(None);
    assert_eq!(store.list(), Err(SummaryError::Unavailable));
    assert!(store
        .begin(summary(Uuid::new_v4(), AttemptVerdict::Running))
        .is_err());
}

#[test]
fn reading_absent_history_does_not_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().join("not-created")));
    assert!(store.list().unwrap().is_empty());
    assert!(!dir.path().join("not-created").exists());
}

#[cfg(unix)]
#[test]
fn summary_directory_is_private_and_symlinks_are_not_followed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    store
        .begin(summary(Uuid::new_v4(), AttemptVerdict::Running))
        .unwrap();
    let root = dir.path().join("send-history-v1");
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o077,
        0
    );
    let outside = tempfile::tempdir().unwrap();
    std::fs::remove_file(root.join("history.json")).unwrap();
    std::fs::remove_dir(&root).unwrap();
    symlink(outside.path(), &root).unwrap();
    assert!(store
        .begin(summary(Uuid::new_v4(), AttemptVerdict::Running))
        .is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn byte_bound_keeps_recent_sends_without_permanently_blocking_new_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut latest = String::new();
    for _ in 0..260 {
        let mut attempt = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
        attempt.target_display = "c".repeat(700);
        latest = attempt.id.clone();
        store.begin(attempt).unwrap();
    }
    let small = summary(Uuid::new_v4(), AttemptVerdict::Running);
    store.begin(small.clone()).unwrap();
    let saved = store.list().unwrap();
    assert!(saved.len() < 256);
    assert!(saved.iter().any(|a| a.id == latest));
    assert_eq!(saved.last().unwrap().id, small.id);
    assert!(
        std::fs::metadata(dir.path().join("send-history-v1/history.json"))
            .unwrap()
            .len()
            <= 256 * 1024
    );
}

fn details() -> bridge_engine::send_summary::AttemptDetails {
    serde_json::from_value(serde_json::json!({
        "documentId":Uuid::new_v4().to_string(), "model":"model", "capturedAt":1,
        "cases":[{"id":"one","input":"question","assertions":[{"id":"a","type":"contains","value":"answer"}]}],
        "resources":[], "report":null, "detailState":"saved", "availability":null
    })).unwrap()
}

#[test]
fn details_restart_remove_and_legacy_keep_exact_summary_shape() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let old = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
    store.begin(old.clone()).unwrap();
    assert_eq!(store.details(&old.id).unwrap().detail_state, "legacy");
    let attempt = summary(Uuid::new_v4(), AttemptVerdict::Passed);
    store.checkpoint(attempt.clone(), details()).unwrap();
    let reopened = SummaryStore::new(Some(dir.path().into()));
    assert_eq!(
        reopened.details(&attempt.id).unwrap().model.as_deref(),
        Some("model")
    );
    assert_eq!(reopened.list().unwrap(), vec![old, attempt.clone()]);
    let removed = reopened.remove_details(&attempt.id).unwrap();
    assert_eq!(removed.detail_state, "removed");
    assert!(removed.model.is_none());
    assert_eq!(reopened.list().unwrap().last().unwrap(), &attempt);
}

#[test]
fn overflow_retains_control_and_saturation_never_evicts_unresolved_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut evidence = details();
    evidence.resources.push(
        serde_json::from_value(serde_json::json!({
            "kind":"Task","name":"bridge-task-pending","namespace":"agents", "uid":null,
            "generation":0,"specDigest":"a".repeat(64),"terminal":false
        }))
        .unwrap(),
    );
    let mut first = String::new();
    for i in 0..256 {
        let mut attempt = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
        attempt.agent_uid = Some("known".into());
        if i == 0 {
            first = attempt.id.clone();
        }
        store.checkpoint(attempt, evidence.clone()).unwrap();
    }
    assert!(store
        .checkpoint(summary(Uuid::new_v4(), AttemptVerdict::Running), details())
        .is_err());
    assert!(store.remove_details(&first).is_err());
    assert_eq!(store.list().unwrap().len(), 256);
    evidence.report = Some(
        serde_json::from_value(serde_json::json!({"cases":[{
        "id":"one","question":"question","phase":"Succeeded","answer":"x".repeat(4*1024*1024),
        "completedTools":[],"assertions":[],"verdict":"unknown"
    }],"verdict":"unknown"}))
        .unwrap(),
    );
    let attempt = store.list().unwrap()[0].clone();
    store.checkpoint(attempt, evidence).unwrap();
    let view = store.details(&first).unwrap();
    assert_eq!(view.detail_state, "omitted");
    assert!(view.interrupted);
    let report = view.report.unwrap();
    assert_eq!(report.cases[0].phase, "Succeeded");
    assert!(report.cases[0].answer.is_none());
    assert_eq!(report.verdict, bridge_engine::engine::run::Verdict::Unknown);
    assert!(
        std::fs::metadata(dir.path().join(format!("send-history-v1/{first}.json")))
            .unwrap()
            .len()
            <= 4 * 1024 * 1024
    );
}

#[test]
fn total_quota_omits_optional_answers_without_losing_protected_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut evidence = details();
    evidence.resources.push(serde_json::from_value(serde_json::json!({"kind":"Task","name":"bridge-task-pending","namespace":"agents","uid":null,"generation":0,"specDigest":"a".repeat(64),"terminal":false})).unwrap());
    let report = |size| {
        serde_json::from_value(serde_json::json!({"cases":[{"id":"one","question":"question","phase":"Succeeded","answer":"x".repeat(size),"completedTools":[],"assertions":[],"verdict":"unknown"}],"verdict":"unknown"})).unwrap()
    };
    evidence.report = Some(report(4_020_000));
    for _ in 0..16 {
        let mut old = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
        old.agent_uid = Some("known".into());
        store.checkpoint(old, evidence.clone()).unwrap();
    }
    let mut current = summary(Uuid::new_v4(), AttemptVerdict::Running);
    current.agent_uid = Some("known".into());
    evidence.report = None;
    store.checkpoint(current.clone(), evidence.clone()).unwrap();
    evidence.report = Some(report(2_000_000));
    assert!(!store
        .checkpoint(current.clone(), evidence)
        .expect("optional answers must not consume reserved control capacity"));
    let view = store.details(&current.id).unwrap();
    assert_eq!(view.detail_state, "omitted");
    assert!(view.interrupted);
    assert!(view.report.unwrap().cases[0].answer.is_none());
    assert_eq!(store.list().unwrap().len(), 17);
    let total: u64 = std::fs::read_dir(dir.path().join("send-history-v1"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(total <= 64 * 1024 * 1024);
}

#[test]
fn agent_acknowledgement_space_is_reserved_in_the_summary_ledger_before_create() {
    fn pad(row: &mut AttemptSummary, mut left: usize) -> usize {
        for field in [
            &mut row.target_display,
            &mut row.provider,
            &mut row.planned_agent_name,
            &mut row.target_uids.service_namespace,
            &mut row.target_uids.service_uid,
            &mut row.target_uids.deployment_namespace,
            &mut row.target_uids.deployment_uid,
            &mut row.target_uids.watched_namespace,
        ] {
            let add = left.min(4096 - field.len());
            field.push_str(&"x".repeat(add));
            left -= add;
        }
        left
    }
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut evidence = details();
    evidence.resources = serde_json::from_value(serde_json::json!([
        {"kind":"Agent","name":"bridge-run-known","namespace":"agents","uid":"known","generation":1,"specDigest":"a".repeat(64),"terminal":false},
        {"kind":"Task","name":"bridge-task-pending","namespace":"agents","uid":null,"generation":0,"specDigest":"b".repeat(64),"terminal":false}
    ])).unwrap();
    for _ in 0..7 {
        let mut old = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
        old.agent_uid = Some("known".into());
        pad(&mut old, usize::MAX);
        store.checkpoint(old, evidence.clone()).unwrap();
    }
    let current = summary(Uuid::new_v4(), AttemptVerdict::Running);
    let mut last = summary(Uuid::new_v4(), AttemptVerdict::Unknown);
    last.agent_uid = Some("known".into());
    let mut rows = store.list().unwrap();
    rows.extend([last.clone(), current.clone()]);
    let left = 256 * 1024
        - 16
        - serde_json::to_vec(&serde_json::json!({"version":1,"attempts":rows}))
            .unwrap()
            .len();
    assert_eq!(pad(&mut last, left), 0);
    store.checkpoint(last, evidence.clone()).unwrap();
    let mut rows = store.list().unwrap();
    rows.push(current.clone());
    assert_eq!(
        serde_json::to_vec(&serde_json::json!({"version":1,"attempts":rows}))
            .unwrap()
            .len(),
        256 * 1024 - 16
    );
    evidence.resources.retain(|r| r.kind == "Agent");
    evidence.resources[0].uid = None;
    evidence.resources[0].generation = 0;
    let control = tempfile::tempdir().unwrap();
    SummaryStore::new(Some(control.path().into()))
        .checkpoint(current.clone(), evidence.clone())
        .unwrap();
    assert!(
        store.checkpoint(current, evidence).is_err(),
        "refuse before a POST whose UID cannot fit in history.json"
    );
    assert_eq!(store.list().unwrap().len(), 8);
}

#[test]
fn per_attempt_capacity_reserves_receipts_before_posting() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut evidence = details();
    let assertions: Vec<_> = (0..64).map(|i| serde_json::json!({"id":format!("a-{i}"),"type":"contains","value":"x".repeat(65_000)})).collect();
    evidence.cases = serde_json::from_value(
        serde_json::json!([{"id":"one","input":"question","assertions":assertions}]),
    )
    .unwrap();
    evidence.resources = serde_json::from_value(serde_json::json!([{"kind":"Agent","name":"bridge-run-planned","namespace":"agents","uid":null,"generation":0,"specDigest":"a".repeat(64),"terminal":false}])).unwrap();
    let extra = 4 * 1024 * 1024 - 16 - serde_json::to_vec(&evidence).unwrap().len();
    let mut cases = serde_json::to_value(&evidence.cases).unwrap();
    let value = cases[0]["assertions"][0]["value"]
        .as_str()
        .unwrap()
        .to_string()
        + &"x".repeat(extra);
    cases[0]["assertions"][0]["value"] = serde_json::json!(value);
    evidence.cases = serde_json::from_value(cases).unwrap();
    assert_eq!(
        serde_json::to_vec(&evidence).unwrap().len(),
        4 * 1024 * 1024 - 16
    );
    let mut acknowledged = evidence.clone();
    acknowledged.resources[0].uid = Some(Uuid::new_v4().to_string());
    assert!(serde_json::to_vec(&acknowledged).unwrap().len() > 4 * 1024 * 1024);
    assert!(
        store
            .checkpoint(summary(Uuid::new_v4(), AttemptVerdict::Running), evidence)
            .is_err(),
        "the per-attempt limit must include future receipt growth before any POST"
    );
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn aggregate_history_quota_includes_the_summary_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut evidence = details();
    evidence.report = Some(serde_json::from_value(serde_json::json!({"cases":[{"id":"one","question":"question","phase":"Succeeded","answer":"","completedTools":[],"assertions":[],"verdict":"unknown"}],"verdict":"unknown"})).unwrap());
    let overhead = serde_json::to_vec(&evidence).unwrap().len();
    evidence.report.as_mut().unwrap().cases[0].answer =
        Some("x".repeat(4 * 1024 * 1024 - overhead));
    assert_eq!(
        serde_json::to_vec(&evidence).unwrap().len(),
        4 * 1024 * 1024
    );
    for _ in 0..16 {
        store
            .checkpoint(
                summary(Uuid::new_v4(), AttemptVerdict::Unknown),
                evidence.clone(),
            )
            .unwrap();
    }
    assert_eq!(
        store.list().unwrap().len(),
        15,
        "sixteen full sidecars leave no room for their ledger"
    );
    let total: u64 = std::fs::read_dir(dir.path().join("send-history-v1"))
        .unwrap()
        .map(|e| e.unwrap().metadata().unwrap().len())
        .sum();
    assert!(total <= 64 * 1024 * 1024);
}

#[test]
fn interrupted_detail_publication_never_exposes_a_saved_pass_after_next_send() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let attempt = summary(Uuid::new_v4(), AttemptVerdict::Passed);
    let mut detail = details();
    detail.report =
        Some(serde_json::from_value(serde_json::json!({"cases":[], "verdict":"passed"})).unwrap());
    store.checkpoint(attempt.clone(), detail).unwrap();
    std::fs::write(dir.path().join("send-history-v1/saving"), &attempt.id).unwrap();
    assert!(store.details(&attempt.id).unwrap().report.is_none());
    store
        .begin(summary(Uuid::new_v4(), AttemptVerdict::Running))
        .unwrap();
    let quarantined = store.details(&attempt.id).unwrap();
    assert!(quarantined.report.is_none());
    assert_eq!(quarantined.detail_state, "omitted");
    assert_eq!(store.list().unwrap()[0].verdict, AttemptVerdict::Unknown);
}

#[test]
fn corrupt_summary_not_pass() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    store
        .begin(summary(Uuid::new_v4(), AttemptVerdict::Running))
        .unwrap();
    let root = dir.path().join("send-history-v1");
    std::fs::write(
        root.join("history.json"),
        b"{\"version\":1,\"attempts\":[{\"id\":\"bad\"}]}",
    )
    .unwrap();
    assert_eq!(store.list(), Err(SummaryError::Corrupt));
}
