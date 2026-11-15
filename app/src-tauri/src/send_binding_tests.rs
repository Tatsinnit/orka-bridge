use super::*;
use serde_json::json;

fn target() -> TargetUids {
    TargetUids {
        service_namespace: "control".into(),
        service_uid: "service-1".into(),
        deployment_namespace: "control".into(),
        deployment_uid: "deployment-1".into(),
        watched_namespace: "agents".into(),
    }
}
fn binding() -> Binding {
    Binding {
        document: DocumentId::SAMPLE,
        target: target(),
        current: true,
        name: "bridge-run-owned".into(),
        desired: json!({"model":{"name":"one"}}),
        acknowledged: None,
        intent: Some(json!({"model":{"name":"one"}})),
        attempt: Uuid::new_v4().to_string(),
        unfinished: true,
    }
}
fn receipt(spec: Value, generation: u64) -> Value {
    json!({"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":{"name":"bridge-run-owned","namespace":"agents","uid":"agent-uid","generation":generation},"spec":spec})
}
#[test]
fn intent_survives_restart_without_attempt_details_and_never_comes_from_legacy_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    assert!(store
        .binding(DocumentId::SAMPLE, &target())
        .unwrap()
        .is_none());
    let pending = binding();
    store.save_binding(pending.clone()).unwrap();
    let reopened = SummaryStore::new(Some(dir.path().into()));
    assert!(reopened.list().unwrap().is_empty());
    let saved = reopened
        .binding(DocumentId::SAMPLE, &target())
        .unwrap()
        .unwrap();
    assert!(saved.intent.is_some());
    assert!(saved.unfinished);
    assert_eq!(saved.attempt, pending.attempt);
    let mut other = target();
    other.service_uid = "replacement".into();
    assert!(reopened
        .binding(DocumentId::SAMPLE, &other)
        .unwrap()
        .is_none());
}
#[test]
fn desired_equality_is_not_inclusion_and_optional_soul_removal_is_an_update() {
    let mut saved = binding();
    saved.intent = Some(json!({"model":{"name":"one"},"soul":{"inline":"persona"}}));
    saved
        .acknowledge(receipt(
            json!({"model":{"name":"one"},"soul":{"inline":"persona"},"default":true}),
            1,
        ))
        .unwrap();
    assert_eq!(
        saved.action(&json!({"model":{"name":"one"},"soul":{"inline":"persona"}})),
        SendAction::Unchanged
    );
    assert_eq!(
        saved.action(&json!({"model":{"name":"one"}})),
        SendAction::Update
    );
    assert_eq!(
        saved.acknowledged.as_ref().unwrap()["spec"]["default"],
        true
    );
}
#[test]
fn uncertain_update_cannot_be_confirmed_by_unchanged_old_or_same_name_replacement() {
    let mut saved = binding();
    saved.intent = Some(json!({"model":{"name":"one"},"soul":{"inline":"persona"}}));
    saved
        .acknowledge(receipt(
            json!({"model":{"name":"one"},"soul":{"inline":"persona"}}),
            1,
        ))
        .unwrap();
    saved.intent = Some(json!({"model":{"name":"one"}}));
    let old = saved.acknowledged.clone().unwrap();
    assert!(saved.reconciled(&old).is_err());
    let mut replacement = receipt(json!({"model":{"name":"one"}}), 2);
    replacement["metadata"]["uid"] = json!("other");
    assert!(saved.reconciled(&replacement).is_err());
    assert!(saved.intent.is_some());
    let acknowledged = saved
        .reconciled(&receipt(json!({"model":{"name":"one"}}), 2))
        .unwrap();
    assert_eq!(acknowledged.uid, "agent-uid");
}
#[test]
fn lost_update_never_adopts_an_outside_edit_or_a_retained_removed_field() {
    let mut saved = binding();
    saved.intent = Some(json!({"model":{"name":"one"},"soul":{"inline":"persona"}}));
    saved
        .acknowledge(receipt(
            json!({"model":{"name":"one"},"soul":{"inline":"persona"},"default":true}),
            1,
        ))
        .unwrap();
    saved.intent = Some(json!({"model":{"name":"one"}}));
    // Our lost PUT never landed; an outside writer edited the omitted field.
    let outside = receipt(
        json!({"model":{"name":"one"},"soul":{"inline":"outside"},"default":true}),
        2,
    );
    assert!(saved.reconciled(&outside).is_err());
    let outside = receipt(
        json!({"model":{"name":"one"},"default":true,"outside":true}),
        2,
    );
    assert!(saved.reconciled(&outside).is_err());
    let applied = receipt(json!({"model":{"name":"one"},"default":true}), 2);
    assert!(saved.reconciled(&applied).is_ok());
}
#[test]
fn bindings_are_independent_of_details_and_rebinding_preserves_old_attempt_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let first = binding();
    store.save_binding(first.clone()).unwrap();
    let mut next = first.clone();
    next.name = "bridge-run-separate".into();
    next.attempt = Uuid::new_v4().to_string();
    store.save_binding(next.clone()).unwrap();
    assert_eq!(
        store
            .binding(DocumentId::SAMPLE, &target())
            .unwrap()
            .unwrap()
            .name,
        "bridge-run-separate"
    );
    assert_ne!(first.attempt, next.attempt);
    std::fs::write(dir.path().join("send-history-v1/history.json"), b"corrupt").unwrap();
    assert_eq!(
        store
            .binding(DocumentId::SAMPLE, &target())
            .unwrap()
            .unwrap()
            .name,
        "bridge-run-separate"
    );
}
#[test]
fn rejected_separate_creation_restores_the_previous_binding_without_losing_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut previous = binding();
    previous
        .acknowledge(receipt(previous.desired.clone(), 1))
        .unwrap();
    store.save_binding(previous.clone()).unwrap();
    let mut rejected = binding();
    rejected.name = "bridge-run-rejected".into();
    store.save_binding(rejected.clone()).unwrap();
    store.reject_creation(&rejected).unwrap();
    let current = store
        .binding(DocumentId::SAMPLE, &target())
        .unwrap()
        .unwrap();
    assert_eq!(current.name, previous.name);
    assert!(current.acknowledged.is_some());
    store.reject_creation(&current).unwrap();
    assert!(store
        .binding(DocumentId::SAMPLE, &target())
        .unwrap()
        .unwrap()
        .acknowledged
        .is_some());
}
#[test]
fn corrupt_binding_store_fails_closed_instead_of_forgetting_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    store.save_binding(binding()).unwrap();
    std::fs::write(dir.path().join("send-history-v1/bindings.json"), b"{}").unwrap();
    assert!(store.binding(DocumentId::SAMPLE, &target()).is_err());
    assert!(store.save_binding(binding()).is_err());
}
