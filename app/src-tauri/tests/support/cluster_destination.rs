//! Destination identity and upgrade behavior through real native Send/forward owners.
use super::*;

#[tokio::test]
async fn legacy_linked_ownership_blocks_preflight_send_and_separate_without_mutation() {
    let s = setup().await;
    s.finish(&id(&s.start().await)).await;
    let path = s
        .home
        .path()
        .join("app-local/send-history-v1/bindings.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved[0]["target"]
        .as_object_mut()
        .unwrap()
        .remove("clusterIdentity");
    saved[0]["unfinished"] = json!(true);
    // Retain a pending intent across refusal; do not make a duplicate Agent a recovery path.
    saved[0]["intent"] = saved[0]["desired"].clone();
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let writes = s.api.wire.posts();
    let preflight = reply(s.owner.preflight(&s.target, &s.provider).await);
    assert_eq!(preflight["canSend"], false, "{preflight}");
    assert_eq!(preflight["canSeparate"], false);
    assert!(preflight["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["message"]
            .as_str()
            .is_some_and(|s| s.contains("predates destination verification"))));
    let send = s.finish(&id(&s.start().await)).await;
    assert_eq!(send["error"], "legacyDestination", "{send}");
    let separate = s.finish(&id(&update::separate(&s).await)).await;
    assert_eq!(separate["error"], "legacyDestination", "{separate}");
    assert_eq!(s.api.wire.posts(), writes);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    s.idle_forwards().await;
}

#[tokio::test]
async fn unpinned_or_foreign_history_cannot_check_or_cancel_identical_live_uids() {
    for identity in [None, Some("b".repeat(64))] {
        let s = setup().await;
        *s.api.wire.mode.lock().unwrap() = "running";
        let send = id(&s.start().await);
        until(|| s.view(Some(&send))["stage"] == "observe").await;
        s.owner.stop(&send).await;
        s.idle_forwards().await;
        let path = s.home.path().join("app-local/send-history-v1/history.json");
        let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let target = saved["attempts"][0]["targetUids"].as_object_mut().unwrap();
        target.remove("clusterIdentity");
        if let Some(identity) = identity {
            target.insert("clusterIdentity".into(), json!(identity));
        }
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let calls = s.api.wire.calls.lock().unwrap().len();
        let forwards = s.api.wire.forwards.load(Ordering::SeqCst);
        s.owner.history_check(&send).await.unwrap();
        let cancelled = reply(s.owner.history_cancel(&send).await.unwrap());
        assert_eq!(cancelled["cancellation"], "unconfirmed", "{cancelled}");
        assert_eq!(s.api.wire.calls.lock().unwrap().len(), calls);
        assert_eq!(s.api.wire.forwards.load(Ordering::SeqCst), forwards);
    }
}

#[tokio::test]
async fn foreign_pod_reports_destination_refusal_in_send_preflight() {
    let s = setup().await;
    *s.api.response_hook.lock().unwrap() = Some(Box::new(|path, value| {
        if path.ends_with("/pods") {
            for pod in value["items"].as_array_mut().unwrap() {
                pod["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("ownerReferences");
            }
        }
    }));
    let preflight = reply(s.owner.preflight(&s.target, &s.provider).await);
    assert_eq!(preflight["canSend"], false);
    assert!(
        preflight["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["message"]
                .as_str()
                .is_some_and(|s| s.contains("did not send the Orka credential"))),
        "{preflight}"
    );
    let send = s.finish(&id(&s.start().await)).await;
    assert_eq!(send["error"], "destinationMismatch", "{send}");
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn fresh_trust_change_blocks_send_without_using_frozen_creator_or_orka() {
    let s = setup().await;
    s.fresh.lock().unwrap().clusters[0]
        .cluster
        .as_mut()
        .unwrap()
        .insecure_skip_tls_verify = Some(true);
    let calls = s.api.wire.calls.lock().unwrap().len();
    let cluster_calls = s.api.calls.lock().unwrap().len();
    let preflight = reply(s.owner.preflight(&s.target, &s.provider).await);
    assert_eq!(preflight["canSend"], false);
    let send = s.finish(&id(&s.start().await)).await;
    assert_eq!(send["error"], "preflight");
    assert_eq!(s.api.calls.lock().unwrap().len(), cluster_calls);
    assert_eq!(s.api.wire.calls.lock().unwrap().len(), calls);
    assert_eq!(s.api.wire.posts(), 0);
}
