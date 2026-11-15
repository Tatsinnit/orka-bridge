//! Real fake-cluster HTTP/WebSocket boundaries; no cluster credentials.
use super::*;
#[path = "cluster_cancel_extra.rs"]
mod extra;
use extra::set_cases;

fn cancels(s: &Setup) -> usize {
    s.api
        .wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(p, _)| p.starts_with("POST ") && p.contains("/cancel?"))
        .count()
}
async fn running() -> (Setup, String) {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "running";
    let send = id(&s.start().await);
    until(|| s.view(Some(&send))["stage"] == "observe").await;
    (s, send)
}
async fn cancel_active(s: &Setup, send: &str) -> Value {
    let document = s.owner.status(Some(send)).unwrap().document_id;
    serde_json::to_value(s.owner.cancel_document(document, send).unwrap()).unwrap()
}

#[tokio::test]
async fn explicit_cancel_returns_promptly_and_persists_distinct_verdict() {
    let (s, send) = running().await;
    let response = cancel_active(&s, &send).await;
    assert_eq!(response["view"]["cancellation"], "cancelling");
    let done = s.finish(&send).await;
    assert_eq!(done["cancellation"], "confirmed", "{done}");
    assert_eq!(done["report"]["verdict"], "cancelled");
    assert!(done["report"]["cases"][0]["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["verdict"] == "unknown"));
    assert_eq!(cancels(&s), 1);
    assert_eq!(s.agent_posts().len(), 1);
    assert!(!done.to_string().contains("creator-fixture"));
    let saved = serde_json::to_value(s.owner.history_details(&send).unwrap()).unwrap();
    assert_eq!(saved["cancellation"], "confirmed");
    assert_eq!(saved["canCancel"], false);
    assert!(s
        .api
        .wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|(p, _)| !p.starts_with("DELETE") && !p.starts_with("PUT")));
    s.idle_forwards().await;
}

#[tokio::test]
async fn cancel_outcomes_are_honest_and_history_check_is_read_only() {
    for (mode, expected) in [
        ("cancel-running", "requested"),
        ("cancel-already", "requested"),
        ("cancel-denied", "denied"),
        ("cancel-missing", "unconfirmed"),
        ("cancel-unsupported", "unsupported"),
        ("cancel-lost", "unconfirmed"),
        ("cancel-conflict", "confirmed"),
    ] {
        let (s, send) = running().await;
        *s.api.wire.mode.lock().unwrap() = mode;
        cancel_active(&s, &send).await;
        let done = s.finish(&send).await;
        assert_eq!(done["cancellation"], expected, "{mode}: {done}");
        assert_eq!(cancels(&s), 1, "{mode}");
        let before = s.api.wire.posts();
        let checked = s.owner.history_check(&send).await.unwrap();
        assert_eq!(s.api.wire.posts(), before, "history check must be GET only");
        let view = s.view(Some(&send));
        assert_eq!(
            view["cancellation"],
            serde_json::to_value(checked.cancellation).unwrap()
        );
        if mode == "cancel-conflict" {
            assert_eq!(view["report"]["verdict"], "passed", "completion wins");
        } else if mode == "cancel-running" {
            assert_eq!(checked.report.as_ref().map(|r| r.verdict), None);
        }
        s.idle_forwards().await;
    }
}

#[tokio::test]
async fn shutdown_never_initiates_remote_cancel_and_late_cancel_keeps_completed_answer() {
    let (s, send) = running().await;
    s.owner.shutdown().await;
    assert_eq!(cancels(&s), 0);
    assert_eq!(s.view(Some(&send))["cancellation"], Value::Null);
    let s = setup().await;
    let send = id(&s.start().await);
    let before = s.finish(&send).await;
    let response = cancel_active(&s, &send).await;
    assert_eq!(response["view"], before);
    assert_eq!(cancels(&s), 0);
}

#[tokio::test]
async fn history_cancel_survives_lost_ipc_waiter_and_updates_live_view() {
    let (s, send) = running().await;
    s.owner.stop(&send).await;
    s.idle_forwards().await;
    *s.api.wire.mode.lock().unwrap() = "hold-history";
    let owner = s.owner.clone();
    let captured = send.clone();
    let waiter = tokio::spawn(async move { owner.history_cancel(&captured).await });
    until(|| s.api.wire.active.load(Ordering::SeqCst) != 0).await;
    waiter.abort();
    *s.api.wire.mode.lock().unwrap() = "";
    until(|| s.view(Some(&send))["cancellation"] == "confirmed").await;
    assert_eq!(cancels(&s), 1);
    assert_eq!(s.view(Some(&send))["report"]["verdict"], "cancelled");
    s.owner.shutdown().await;
    s.idle_forwards().await;
}

#[tokio::test]
async fn cancellation_refuses_replaced_task_but_continues_other_tasks() {
    let s = setup().await;
    set_cases(&s, 2);
    *s.api.wire.mode.lock().unwrap() = "running";
    let send = id(&s.start().await);
    until(|| s.view(Some(&send))["stage"] == "observe").await;
    {
        let mut objects = s.api.wire.objects.lock().unwrap();
        let task = objects.values_mut().find(|o| o["kind"] == "Task").unwrap();
        task["metadata"]["uid"] = json!("replacement");
    }
    cancel_active(&s, &send).await;
    let done = s.finish(&send).await;
    assert_eq!(done["cancellation"], "unconfirmed", "{done}");
    assert_eq!(cancels(&s), 1);
    assert_ne!(done["report"]["verdict"], "passed");
}

#[tokio::test]
async fn history_cancel_revalidates_exact_saved_destination_before_any_write() {
    let (s, send) = running().await;
    s.owner.stop(&send).await;
    *s.api.deployment_replacement.lock().unwrap() = true;
    let detail = serde_json::to_value(s.owner.history_cancel(&send).await.unwrap()).unwrap();
    assert_eq!(detail["cancellation"], "unconfirmed");
    assert_eq!(cancels(&s), 0);
}
