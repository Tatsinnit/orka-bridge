use super::*;
use crate::clusters::{ContextId, TargetBinding};
use crate::send_summary::{AttemptSummary, AttemptVerdict};
use bridge_runtime_protocol::{SnapshotId, Uuid};

fn binding() -> TargetBinding {
    TargetBinding {
        cluster_identity: "a".repeat(64),
        context_id: ContextId(Uuid::new_v4()),
        context_name: "team".into(),
        server_address: "https://cluster.invalid".into(),
        service_name: "orka".into(),
        service_namespace: "control".into(),
        service_uid: "service".into(),
        deployment_name: "controller".into(),
        deployment_namespace: "control".into(),
        deployment_uid: "deployment".into(),
        watched_namespace: "agents".into(),
        image: "image".into(),
        api_port: 8080,
    }
}
fn record() -> AttemptSummary {
    serde_json::from_value(serde_json::json!({"id":Uuid::new_v4().to_string(),"source":SnapshotId::of(b"agent",b"tests"),"targetDisplay":"team · agents","targetUids":{"clusterIdentity":"a".repeat(64),"serviceNamespace":"control","serviceUid":"service","deploymentNamespace":"control","deploymentUid":"deployment","watchedNamespace":"agents"},"provider":"model","plannedAgentName":"same-name","agentUid":"one","verdict":"passed","time":1})).unwrap()
}
fn place() -> Place {
    let mut p = Place::new(binding());
    p.stale = false;
    p.observed_at = Some(1);
    p.agents = vec![
        agent(
            &serde_json::json!({"metadata":{"name":"same-name","namespace":"agents","uid":"one"}}),
            "agents",
        )
        .unwrap(),
        agent(
            &serde_json::json!({"metadata":{"name":"same-name","namespace":"agents","uid":"two"}}),
            "agents",
        )
        .unwrap(),
    ];
    p
}
#[test]
fn legacy_history_is_readable_but_never_associated_with_live_inventory() {
    let mut legacy = serde_json::to_value(record()).unwrap();
    legacy["targetUids"]
        .as_object_mut()
        .unwrap()
        .remove("clusterIdentity");
    let legacy: AttemptSummary = serde_json::from_value(legacy).unwrap();
    let result = project(vec![place()], Some(vec![legacy]));
    assert!(result.places[0].agents.iter().all(|a| a.record.is_none()));
    assert_eq!(result.unobserved.len(), 1);
}
#[test]
fn recorded_target_and_uid_only_never_name_or_unacknowledged() {
    let summary = record();
    let mut unknown = record();
    unknown.agent_uid = None;
    unknown.verdict = AttemptVerdict::Unknown;
    let result = project(vec![place()], Some(vec![unknown.clone(), summary.clone()]));
    assert!(result.places[0].agents[0].record.is_some());
    assert!(result.places[0].agents[1].record.is_none());
    assert_eq!(result.unobserved.len(), 1);
    assert!(result.unobserved[0].agent_uid.is_none());
    let mut other = place();
    other.identity.deployment_uid = "replacement".into();
    assert!(
        project(vec![other], Some(vec![summary.clone()])).places[0].agents[0]
            .record
            .is_none()
    );
    let mut other = place();
    other.identity.context_name = "other".into();
    assert!(
        project(vec![other], Some(vec![summary])).places[0].agents[0]
            .record
            .is_none()
    );
}
#[test]
fn copied_uids_under_another_destination_never_join_history_or_place_keys() {
    let original = place();
    let mut copied = place();
    copied.identity.cluster_identity = Some("b".repeat(64));
    assert_ne!(original.identity.key(), copied.identity.key());
    let result = project(vec![copied], Some(vec![record()]));
    assert!(result.places[0].agents.iter().all(|a| a.record.is_none()));
    assert_eq!(result.unobserved.len(), 1);
}
#[test]
fn every_version_joins_independently_and_history_error_never_means_not_tested() {
    let first = record();
    let mut second = record();
    second.agent_uid = Some("two".into());
    second.verdict = AttemptVerdict::Unknown;
    let result = project(vec![place()], Some(vec![second, first]));
    assert_eq!(
        result.places[0].agents[0]
            .record
            .as_ref()
            .unwrap()
            .summary
            .verdict,
        AttemptVerdict::Passed
    );
    assert_eq!(
        result.places[0].agents[1]
            .record
            .as_ref()
            .unwrap()
            .summary
            .verdict,
        AttemptVerdict::Unknown
    );
    assert_eq!(project(vec![place()], None).history_state, "unavailable");
}
#[test]
fn optional_runtime_facts_are_not_inferred_and_required_scope_is_checked() {
    let value = serde_json::json!({"metadata":{"name":"agent","namespace":"agents","uid":"uid"},"spec":{"type":"runtime","model":{"name":"model"},"tools":[{"name":"web_fetch"}]}});
    let a = agent(&value, "agents").unwrap();
    assert_eq!(a.agent_type.as_deref(), Some("runtime"));
    assert_eq!(a.model.as_deref(), Some("model"));
    assert_eq!(a.tools, Some(vec!["web_fetch".into()]));
    assert!(agent(&value, "other").is_err());
    let mut invalid = value.clone();
    invalid["spec"]["model"] = serde_json::json!(42);
    invalid["spec"]["tools"] = serde_json::json!([{"url":"not-a-tool-name"}]);
    let a = agent(&invalid, "agents").unwrap();
    assert!(a.model.is_none());
    assert!(a.tools.is_none());
    invalid["metadata"]["uid"] = serde_json::Value::Null;
    assert!(agent(&invalid, "agents").is_err());
}
#[test]
fn place_key_ignores_ephemeral_handles_and_image_but_not_installation_or_namespace() {
    let a = binding();
    let mut b = a.clone();
    b.context_id = ContextId(Uuid::new_v4());
    b.image = "upgraded".into();
    assert_eq!(Place::new(a.clone()).key, Place::new(b.clone()).key);
    b.watched_namespace = "different".into();
    assert_ne!(Place::new(a).key, Place::new(b).key);
}

fn evidence() -> crate::send_summary::AttemptDetails {
    serde_json::from_value(serde_json::json!({
        "documentId":"01234567-89ab-4cde-8fab-0123456789ab", "model":"model", "capturedAt":1,
        "cases":[
            {"id":"one","input":"private question","assertions":[{"id":"answer","type":"contains","value":"private"}]},
            {"id":"two","input":"private question","assertions":[{"id":"answer","type":"contains","value":"private"}]}
        ], "resources":[], "detailState":"saved", "availability":null,
        "report":{"verdict":"passed","cases":[
            {"id":"one","question":"private question","phase":"Succeeded","answer":"private answer","completedTools":[],"assertions":[],"verdict":"passed"},
            {"id":"two","question":"private question","phase":"Succeeded","answer":"private answer","completedTools":[],"assertions":[],"verdict":"passed"}
        ]}
    })).unwrap()
}
fn projected(store: &crate::send_summary::SummaryStore) -> Value {
    serde_json::to_value(store.overview(vec![place()], None)).unwrap()
}
#[test]
fn overview_batch_exposes_only_matched_document_and_bounded_final_counts_without_changing_v1() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::send_summary::SummaryStore::new(Some(root.path().into()));
    let summary = record();
    store.checkpoint(summary.clone(), evidence()).unwrap();
    let ledger = root.path().join("send-history-v1/history.json");
    let before = std::fs::read(&ledger).unwrap();
    let view = projected(&store);
    let record = &view["places"][0]["agents"][0]["record"];
    assert_eq!(record["id"], summary.id);
    assert_eq!(record["documentId"], "01234567-89ab-4cde-8fab-0123456789ab");
    assert_eq!(record["detailState"], "saved");
    assert_eq!(
        record["report"],
        serde_json::json!({"verdict":"passed","passed":2,"total":2})
    );
    for private in ["private question", "private answer", "resources", "cases"] {
        assert!(!view.to_string().contains(private));
    }
    assert!(view["places"][0]["agents"][1]["record"].is_null());
    assert_eq!(std::fs::read(ledger).unwrap(), before);
}
#[test]
fn overview_matching_failed_report_keeps_counts_and_unavailable_store_keeps_places() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::send_summary::SummaryStore::new(Some(root.path().into()));
    let mut summary = record();
    summary.verdict = AttemptVerdict::Failed;
    let mut details = evidence();
    let report = details.report.as_mut().unwrap();
    report.verdict = crate::engine::run::Verdict::Failed;
    report.cases[1].verdict = crate::engine::run::Verdict::Failed;
    store.checkpoint(summary, details).unwrap();
    assert_eq!(
        projected(&store)["places"][0]["agents"][0]["record"]["report"],
        serde_json::json!({"verdict":"failed","passed":1,"total":2})
    );
    let unavailable = projected(&crate::send_summary::SummaryStore::new(None));
    assert_eq!(unavailable["historyState"], "unavailable");
    assert_eq!(
        unavailable["places"][0]["agents"].as_array().unwrap().len(),
        2
    );
    assert!(unavailable["places"][0]["agents"][0]["record"].is_null());
}
#[test]
fn overview_saved_final_counts_survive_unavailable_reachability() {
    for failed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let store = crate::send_summary::SummaryStore::new(Some(root.path().into()));
        let mut summary = record();
        let mut details = evidence();
        details.availability = Some("unavailable".into());
        if failed {
            summary.verdict = AttemptVerdict::Failed;
            let report = details.report.as_mut().unwrap();
            report.verdict = crate::engine::run::Verdict::Failed;
            report.cases[1].verdict = crate::engine::run::Verdict::Failed;
        }
        store.checkpoint(summary, details).unwrap();
        assert_eq!(
            projected(&store)["places"][0]["agents"][0]["record"]["report"],
            if failed {
                serde_json::json!({"verdict":"failed","passed":1,"total":2})
            } else {
                serde_json::json!({"verdict":"passed","passed":2,"total":2})
            }
        );
    }
}
#[test]
fn overview_partial_recovered_failure_does_not_shrink_the_test_total() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::send_summary::SummaryStore::new(Some(root.path().into()));
    let mut summary = record();
    summary.verdict = AttemptVerdict::Failed;
    let mut details = evidence();
    details.availability = Some("unavailable".into());
    let report = details.report.as_mut().unwrap();
    report.verdict = crate::engine::run::Verdict::Failed;
    report.cases.truncate(1);
    report.cases[0].verdict = crate::engine::run::Verdict::Failed;
    store.checkpoint(summary, details).unwrap();
    let view = projected(&store);
    let record = &view["places"][0]["agents"][0]["record"];
    assert_eq!(record["verdict"], "failed");
    assert_eq!(record["documentId"], "01234567-89ab-4cde-8fab-0123456789ab");
    assert!(record["report"].is_null());
}
#[test]
fn overview_never_borrows_older_counts_or_exports_untrusted_sidecars() {
    use crate::send_summary::SummaryStore;
    for state in [
        "legacy",
        "corrupt",
        "removed",
        "omitted",
        "mismatch",
        "running",
        "unknown",
        "cancelled",
        "oversized",
    ] {
        let root = tempfile::tempdir().unwrap();
        let store = SummaryStore::new(Some(root.path().into()));
        store.checkpoint(record(), evidence()).unwrap();
        let mut latest = record();
        latest.time = 2;
        let mut details = evidence();
        match state {
            "legacy" => {
                store.begin(latest.clone()).unwrap();
            }
            "removed" | "omitted" => {
                details.detail_state = state.into();
                if state == "removed" {
                    details.document_id = None;
                }
                store.checkpoint(latest.clone(), details).unwrap();
            }
            _ => {
                match state {
                    "mismatch" => {
                        details.report.as_mut().unwrap().verdict =
                            crate::engine::run::Verdict::Failed
                    }
                    "running" => latest.verdict = AttemptVerdict::Running,
                    "unknown" => latest.verdict = AttemptVerdict::Unknown,
                    "cancelled" => latest.verdict = AttemptVerdict::Cancelled,
                    "oversized" => {
                        let case = details.report.as_ref().unwrap().cases[0].clone();
                        details.report.as_mut().unwrap().cases = vec![case; 33];
                    }
                    _ => (),
                }
                store.checkpoint(latest.clone(), details).unwrap();
                if state == "corrupt" {
                    std::fs::write(
                        root.path()
                            .join(format!("send-history-v1/{}.json", latest.id)),
                        b"corrupt",
                    )
                    .unwrap();
                }
            }
        }
        let view = projected(&store);
        let result = &view["places"][0]["agents"][0]["record"];
        assert_eq!(result["id"], latest.id, "{state}");
        assert!(result["report"].is_null(), "{state}: {result}");
        let expected = match state {
            "corrupt" => "unavailable",
            "removed" | "omitted" | "legacy" => state,
            _ => "saved",
        };
        assert_eq!(result["detailState"], expected, "{state}");
        if matches!(state, "corrupt" | "removed" | "legacy") {
            assert!(result["documentId"].is_null(), "{state}");
        } else {
            assert_eq!(
                result["documentId"], "01234567-89ab-4cde-8fab-0123456789ab",
                "{state}"
            );
        }
        if state == "running" {
            assert_eq!(result["verdict"], "unknown");
        }
    }
}
#[test]
fn overview_pending_publication_suppresses_report_and_active_attempt_alone_stays_running() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::send_summary::SummaryStore::new(Some(root.path().into()));
    let mut summary = record();
    store.checkpoint(summary.clone(), evidence()).unwrap();
    std::fs::write(
        root.path().join("send-history-v1/saving"),
        summary.id.as_bytes(),
    )
    .unwrap();
    let view = projected(&store);
    let result = &view["places"][0]["agents"][0]["record"];
    assert_eq!(result["verdict"], "unknown");
    assert!(result["report"].is_null());
    assert_eq!(result["documentId"], "01234567-89ab-4cde-8fab-0123456789ab");
    std::fs::remove_file(root.path().join("send-history-v1/saving")).unwrap();
    summary.verdict = AttemptVerdict::Running;
    store.checkpoint(summary.clone(), evidence()).unwrap();
    let live = serde_json::to_value(store.overview(vec![place()], Some(&summary.id))).unwrap();
    assert_eq!(
        live["places"][0]["agents"][0]["record"]["verdict"],
        "running"
    );
    assert!(live["places"][0]["agents"][0]["record"]["report"].is_null());
    assert_eq!(
        projected(&store)["places"][0]["agents"][0]["record"]["verdict"],
        "unknown"
    );
}
#[test]
fn overview_unmatched_corrupt_sidecar_does_not_poison_projection_or_history() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::send_summary::SummaryStore::new(Some(root.path().into()));
    let summary = record();
    store.checkpoint(summary.clone(), evidence()).unwrap();
    let mut other = record();
    other.agent_uid = Some("unobserved".into());
    store.checkpoint(other.clone(), evidence()).unwrap();
    std::fs::write(
        root.path()
            .join(format!("send-history-v1/{}.json", other.id)),
        b"corrupt",
    )
    .unwrap();
    let view = projected(&store);
    assert_eq!(view["historyState"], "complete");
    assert_eq!(view["unobserved"][0]["id"], other.id);
    assert!(view["unobserved"][0].get("documentId").is_none());
    assert_eq!(
        view["places"][0]["agents"][0]["record"]["report"]["total"],
        2
    );
}

fn stored() -> metadata::Remembered {
    metadata::Remembered {
        identity: metadata::Identity::from(&binding()),
        observed_at: 123,
    }
}
#[tokio::test]
async fn metadata_roundtrip_is_allowlisted_stale_passive_and_removal_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, items) = metadata::Store::load(Some(dir.path().into()));
    assert!(items.is_empty());
    store.save(&[stored()]).unwrap();
    let bytes = std::fs::read(dir.path().join("clusters-v1.json")).unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"version":1,"places":[{
        "identity":{"clusterIdentity":"a".repeat(64),"contextName":"team","serviceName":"orka","serviceNamespace":"control",
        "serviceUid":"service","deploymentNamespace":"control","deploymentUid":"deployment","watchedNamespace":"agents"},
        "observedAt":123}]})
    );
    let inventory = Inventory::with_storage(Some(dir.path().into()));
    assert!(inventory
        .state
        .lock()
        .unwrap()
        .entries
        .values()
        .all(|e| e.access.is_none()));
    let session = ClusterSession::with_storage(Some(dir.path().into()));
    let snapshot = session.places();
    assert_eq!(snapshot.persistence_state, "complete");
    assert!(snapshot.selected_key.is_none());
    assert_eq!(snapshot.places[0].key, Place::new(binding()).key);
    assert_eq!(snapshot.places[0].observed_at, None);
    assert_eq!(
        serde_json::to_value(&snapshot.places[0]).unwrap()["checkedAt"],
        123
    );
    assert!(snapshot.places[0].stale);
    assert!(snapshot.places[0].agents.is_empty());
    assert!(session.overview().await[0].stale);
    session.shutdown().await;
    assert_eq!(
        std::fs::read(dir.path().join("clusters-v1.json")).unwrap(),
        bytes
    );
    session.forget(&snapshot.places[0].key).await.unwrap();
    let restored = ClusterSession::with_storage(Some(dir.path().into()));
    assert!(restored.places().places.is_empty());
    assert_eq!(restored.places().persistence_state, "complete");
}
#[test]
fn metadata_corrupt_unknown_oversized_and_symlink_are_sticky_and_never_overwritten() {
    let good = serde_json::json!({"version":1,"places":[]}).to_string();
    for bytes in [
        b"corrupt".to_vec(),
        good.replace("\"version\":1", "\"version\":2").into_bytes(),
        good.replace("\"version\":1", "\"version\":1,\"credentials\":{}")
            .into_bytes(),
        vec![b' '; 1024 * 1024 + 1],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clusters-v1.json");
        std::fs::write(&file, &bytes).unwrap();
        let (mut store, items) = metadata::Store::load(Some(dir.path().into()));
        assert!(items.is_empty());
        assert_eq!(store.state(), "unavailable");
        assert!(store.save(&[stored()]).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("original");
        std::fs::write(&original, &good).unwrap();
        std::os::unix::fs::symlink(&original, dir.path().join("clusters-v1.json")).unwrap();
        let (mut store, _) = metadata::Store::load(Some(dir.path().into()));
        assert_eq!(store.state(), "unavailable");
        assert!(store.save(&[stored()]).is_err());
        assert_eq!(std::fs::read_to_string(&original).unwrap(), good);
        assert!(dir.path().join("clusters-v1.json").is_symlink());
    }
}
#[tokio::test]
async fn metadata_write_failure_latches_and_forget_cannot_claim_success() {
    let dir = tempfile::tempdir().unwrap();
    let (mut store, _) = metadata::Store::load(Some(dir.path().into()));
    store.save(&[stored()]).unwrap();
    let session = ClusterSession::with_storage(Some(dir.path().into()));
    let key = session.places().places[0].key.clone();
    let file = dir.path().join("clusters-v1.json");
    std::fs::rename(&file, dir.path().join("prior")).unwrap();
    std::fs::create_dir(&file).unwrap();
    assert!(session.forget(&key).await.is_err());
    assert_eq!(session.places().places.len(), 1);
    assert_eq!(session.places().persistence_state, "unavailable");
    std::fs::remove_dir(&file).unwrap();
    std::fs::rename(dir.path().join("prior"), &file).unwrap();
    let bytes = std::fs::read(&file).unwrap();
    assert!(session.forget(&key).await.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
}

#[test]
fn metadata_rejects_out_of_contract_text_times_counts_and_nested_unknown_fields() {
    assert_eq!(
        ClusterSession::with_storage(None)
            .places()
            .persistence_state,
        "unavailable"
    );
    assert_eq!(
        ClusterSession::default().places().persistence_state,
        "complete"
    );
    let base = serde_json::json!({"version":1,"places":[stored()]});
    let mut invalid = Vec::new();
    for field in [
        "contextName",
        "serviceName",
        "serviceNamespace",
        "deploymentNamespace",
    ] {
        for value in [
            "x".repeat(2049),
            "control\u{7f}".into(),
            "control\n".into(),
            String::new(),
        ] {
            let mut item = base.clone();
            item["places"][0]["identity"][field] = value.into();
            invalid.push(item);
        }
    }
    for field in ["serviceUid", "deploymentUid"] {
        let mut item = base.clone();
        item["places"][0]["identity"][field] = "x".repeat(4097).into();
        invalid.push(item);
    }
    for field in ["serverAddress", "contextId", "providers"] {
        let mut item = base.clone();
        item["places"][0]["identity"][field] = "unexpected".into();
        invalid.push(item);
    }
    let mut future = base.clone();
    future["places"][0]["observedAt"] = serde_json::json!(8_640_000_000_000_001_u64);
    invalid.push(future);
    let mut crowded = base.clone();
    crowded["places"] = serde_json::json!((0..1025)
        .map(|i| {
            let mut p = serde_json::to_value(stored()).unwrap();
            p["identity"]["serviceUid"] = format!("service-{i}").into();
            p
        })
        .collect::<Vec<_>>());
    invalid.push(crowded);
    for value in invalid {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clusters-v1.json");
        let bytes = value.to_string().into_bytes();
        std::fs::write(&file, &bytes).unwrap();
        let (mut store, items) = metadata::Store::load(Some(dir.path().into()));
        assert_eq!(store.state(), "unavailable", "{value}");
        assert!(items.is_empty());
        assert!(store.save(&[stored()]).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
    }
}
#[test]
fn an_attempts_destination_key_is_overviews_place_key_and_follows_frozen_identity() {
    assert_eq!(attempt_key(&record()), Some(place().key));
    // A replaced deployment is a different place, whatever its name says.
    let mut replaced = record();
    replaced.target_uids.deployment_uid = "replacement".into();
    assert_ne!(attempt_key(&replaced), Some(place().key));
    // Copied UIDs or legacy history must not focus a live card on another cluster.
    let mut copied = record();
    copied.target_uids.cluster_identity = Some("b".repeat(64));
    assert_ne!(attempt_key(&copied), Some(place().key));
    copied.target_uids.cluster_identity = None;
    assert_ne!(attempt_key(&copied), Some(place().key));
    // Display text that does not name the recorded namespace cannot become a key.
    let mut odd = record();
    odd.target_display = "team".into();
    assert_eq!(attempt_key(&odd), None);
}
