use super::*;
use serde_json::json;

fn target() -> TargetUids {
    TargetUids {
        cluster_identity: Some("a".repeat(64)),
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
fn unpinned_current_ownership_refuses_lookup_without_rewriting_pending_intent() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    let mut legacy = serde_json::to_value(binding()).unwrap();
    legacy["target"]
        .as_object_mut()
        .unwrap()
        .remove("clusterIdentity");
    let legacy: Binding = serde_json::from_value(legacy).unwrap();
    store.save_binding(legacy.clone()).unwrap();
    let before = std::fs::read(dir.path().join("send-history-v1/bindings.json")).unwrap();
    assert!(store.binding(DocumentId::SAMPLE, &target()).is_err());
    assert_eq!(
        std::fs::read(dir.path().join("send-history-v1/bindings.json")).unwrap(),
        before
    );
    let retained = store.attempt_binding(&legacy.attempt).unwrap().unwrap();
    assert!(retained.unfinished && retained.intent.is_some());
}
#[test]
fn copied_uids_at_a_different_destination_do_not_reuse_owned_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = SummaryStore::new(Some(dir.path().into()));
    store.save_binding(binding()).unwrap();
    let mut other = target();
    other.cluster_identity = Some("b".repeat(64));
    assert!(store.binding(DocumentId::SAMPLE, &other).unwrap().is_none());
    assert!(store
        .binding(DocumentId::SAMPLE, &target())
        .unwrap()
        .is_some());
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

use crate::engine::run::{RunFuture, RunLimits};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{sync::Notify, time::Instant};

/// Only the conditional update is scripted; every other request is unsupported.
struct Updates {
    ready: bool,
    puts: AtomicUsize,
    polled: Notify,
    /// A Stop that arrives in the same poll as the response.
    latch: Option<watch::Sender<bool>>,
    /// The PUT is rejected as missing, so the update falls back to a diagnostic GET.
    put_missing: bool,
    /// None leaves the diagnostic GET pending; Some answers it with that result.
    diagnostic: Option<Result<(), ErrorKind>>,
    gets: AtomicUsize,
    got: Notify,
    /// A Stop that arrives in the same poll as the diagnostic response.
    diagnostic_latch: Option<watch::Sender<bool>>,
}
impl Updates {
    fn new(ready: bool) -> Self {
        Self {
            ready,
            puts: AtomicUsize::new(0),
            polled: Notify::new(),
            latch: None,
            put_missing: false,
            diagnostic: Some(Err(ErrorKind::Unsupported)),
            gets: AtomicUsize::new(0),
            got: Notify::new(),
            diagnostic_latch: None,
        }
    }
}
impl RunTransport for Updates {
    fn namespace(&self) -> &str {
        "agents"
    }
    fn provider(&self) -> &str {
        ""
    }
    fn limits(&self) -> RunLimits {
        RunLimits::default()
    }
    fn request<'a>(
        &'a self,
        _: &'a str,
        _: Option<u64>,
        _: Option<&'a Value>,
        _: Instant,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            self.gets.fetch_add(1, Ordering::SeqCst);
            self.got.notify_one();
            if let Some(latch) = &self.diagnostic_latch {
                latch.send_replace(true);
            }
            match self.diagnostic {
                Some(result) => result.map(|()| Value::Null),
                None => std::future::pending().await,
            }
        })
    }
    fn update_agent<'a>(&'a self, _: &'a str, body: &'a Value, _: Instant) -> RunFuture<'a> {
        Box::pin(async move {
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.polled.notify_one();
            if self.put_missing {
                return Err(ErrorKind::NotFound);
            }
            if let Some(latch) = &self.latch {
                latch.send_replace(true);
            }
            if self.ready {
                Ok(receipt(body["spec"].clone(), 2))
            } else {
                std::future::pending().await
            }
        })
    }
}
/// A saved owned agent, plus the reserved next attempt that the update would replace it with.
fn reserved(store: &SummaryStore) -> (Binding, Binding, Value) {
    let mut prior = binding();
    prior
        .acknowledge(receipt(prior.desired.clone(), 1))
        .unwrap();
    prior.unfinished = false;
    store.save_binding(prior.clone()).unwrap();
    let desired = json!({"model":{"name":"two"}});
    let mut next = prior.clone();
    next.attempt = Uuid::new_v4().to_string();
    next.unfinished = true;
    next.intent = Some(desired.clone());
    store.save_binding(next.clone()).unwrap();
    (prior, next, desired)
}
fn saved(store: &SummaryStore) -> Value {
    serde_json::to_value(
        store
            .binding(DocumentId::SAMPLE, &target())
            .unwrap()
            .unwrap(),
    )
    .unwrap()
}
fn owner(dir: &tempfile::TempDir) -> Arc<SendOwner> {
    SendOwner::with_history_root(
        Arc::new(AgentFolder::new(None)),
        ClusterSession::default(),
        Some(dir.path().into()),
    )
}
#[tokio::test]
async fn stop_before_the_put_restores_the_prior_binding_and_never_polls_it() {
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(&dir);
    let (prior, mut next, desired) = reserved(&owner.summary);
    let (_stop, latched) = watch::channel(true);
    let transport = Updates::new(true);
    let result = owner
        .update_deployment(&mut next, &transport, desired, &latched, &prior)
        .await;
    assert!(matches!(result, Err(Stage::Stopped)));
    assert_eq!(transport.puts.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&owner.summary), serde_json::to_value(&prior).unwrap());
    assert_eq!(
        serde_json::to_value(&next).unwrap(),
        serde_json::to_value(&prior).unwrap()
    );
}
#[tokio::test]
async fn failing_to_restore_the_prior_binding_is_storage_not_a_restored_stop() {
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(&dir);
    let (mut prior, mut next, desired) = reserved(&owner.summary);
    // The reserved binding saves; the unsaveable "prior" stands in for a failed restore write.
    prior.attempt = "not-a-send".into();
    let (_stop, latched) = watch::channel(true);
    let transport = Updates::new(true);
    let result = owner
        .update_deployment(&mut next, &transport, desired.clone(), &latched, &prior)
        .await;
    assert!(matches!(result, Err(Stage::Storage)));
    assert_eq!(transport.puts.load(Ordering::SeqCst), 0);
    assert_eq!(saved(&owner.summary)["intent"], desired);
}
#[tokio::test]
async fn stop_while_the_put_is_pending_keeps_the_intent_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(&dir);
    let (prior, mut next, desired) = reserved(&owner.summary);
    let (stop, rx) = watch::channel(false);
    let transport = Updates::new(false);
    let stopper = async {
        transport.polled.notified().await;
        stop.send_replace(true);
    };
    // Bounded so a regression fails instead of hanging on the never-ready PUT.
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            owner.update_deployment(&mut next, &transport, desired.clone(), &rx, &prior),
            stopper
        )
    })
    .await
    .expect("pending PUT and Stop must finish together");
    assert!(matches!(result, Err(Stage::UpdateUnconfirmed)));
    assert_eq!(transport.puts.load(Ordering::SeqCst), 1);
    let binding = saved(&owner.summary);
    assert_eq!(binding["intent"], desired);
    assert_eq!(binding["acknowledged"]["metadata"]["generation"], 1);
}
#[tokio::test]
async fn a_ready_receipt_wins_a_simultaneous_stop() {
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(&dir);
    let (prior, mut next, desired) = reserved(&owner.summary);
    let (stop, rx) = watch::channel(false);
    let mut transport = Updates::new(true);
    transport.latch = Some(stop);
    let accepted = owner
        .update_deployment(&mut next, &transport, desired.clone(), &rx, &prior)
        .await
        .ok()
        .unwrap();
    assert!(*rx.borrow());
    assert_eq!(
        (accepted.uid.as_str(), accepted.generation),
        ("agent-uid", 2)
    );
    let binding = saved(&owner.summary);
    assert_eq!(binding["intent"], Value::Null);
    assert_eq!(binding["desired"], desired);
    assert_eq!(binding["acknowledged"]["metadata"]["generation"], 2);
}
#[tokio::test]
async fn stop_while_the_diagnostic_get_is_pending_keeps_the_intent_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(&dir);
    let (prior, mut next, desired) = reserved(&owner.summary);
    let (stop, rx) = watch::channel(false);
    let mut transport = Updates::new(false);
    transport.put_missing = true;
    transport.diagnostic = None;
    let stopper = async {
        transport.got.notified().await;
        stop.send_replace(true);
    };
    // Bounded so a regression fails instead of hanging on the never-ready GET.
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(
            owner.update_deployment(&mut next, &transport, desired.clone(), &rx, &prior),
            stopper
        )
    })
    .await
    .expect("pending diagnostic GET and Stop must finish together");
    assert!(matches!(result, Err(Stage::UpdateUnconfirmed)));
    assert_eq!(transport.gets.load(Ordering::SeqCst), 1);
    let binding = saved(&owner.summary);
    assert_eq!(binding["intent"], desired);
    assert_eq!(binding["acknowledged"]["metadata"]["generation"], 1);
}
#[tokio::test]
async fn a_ready_diagnostic_wins_a_simultaneous_stop() {
    let dir = tempfile::tempdir().unwrap();
    let owner = owner(&dir);
    let (prior, mut next, desired) = reserved(&owner.summary);
    let (stop, rx) = watch::channel(false);
    let mut transport = Updates::new(false);
    transport.put_missing = true;
    transport.diagnostic = Some(Err(ErrorKind::NotFound));
    transport.diagnostic_latch = Some(stop);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        owner.update_deployment(&mut next, &transport, desired, &rx, &prior),
    )
    .await
    .expect("ready diagnostic must finish");
    assert!(*rx.borrow());
    assert!(matches!(result, Err(Stage::UpdateMissing)));
    let binding = saved(&owner.summary);
    assert_eq!(binding["intent"], Value::Null);
    assert_eq!(binding["unfinished"], false);
}
