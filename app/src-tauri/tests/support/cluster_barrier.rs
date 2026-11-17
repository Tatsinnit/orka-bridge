//! Task submission barriers survive lost replies independently of evictable history.
use super::*;

fn restart(s: &Setup) -> Arc<SendOwner> {
    SendOwner::with_history_root(
        Arc::new(AgentFolder::new(Some(s.home.path().into()))),
        s.session.clone(),
        Some(s.home.path().join("app-local")),
    )
}
fn writes(s: &Setup) -> Vec<(String, Value)> {
    s.api
        .wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.0.starts_with("POST") || c.0.starts_with("PUT"))
        .cloned()
        .collect()
}
async fn discard_history(s: &Setup, send: &str, state: &str) {
    let root = s.home.path().join("app-local/send-history-v1");
    let path = root.join(format!("{send}.json"));
    match state {
        "missing" => std::fs::remove_file(path).unwrap(),
        "corrupt" => std::fs::write(path, b"{").unwrap(),
        "removed" => {
            let mut details: Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            // Model the Agent receipt checkpoint immediately before a crash:
            // no Task barrier has run, and the Agent-only history can be removed.
            let bindings: Value =
                serde_json::from_slice(&std::fs::read(root.join("bindings.json")).unwrap())
                    .unwrap();
            details["resources"][0]["uid"] = bindings[0]["acknowledged"]["metadata"]["uid"].clone();
            details["resources"][0]["generation"] =
                bindings[0]["acknowledged"]["metadata"]["generation"].clone();
            std::fs::write(path, serde_json::to_vec(&details).unwrap()).unwrap();
            assert_eq!(
                s.owner.history_remove(send).await.unwrap().detail_state,
                "removed"
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn acknowledged_agent_without_task_barrier_does_not_depend_on_history() {
    for state in ["missing", "corrupt", "removed"] {
        let s = setup().await;
        let root = s.home.path().join("app-local/send-history-v1");
        let blocked = root.clone();
        *s.api.wire.after_post.lock().unwrap() = Some(Box::new(move || {
            // Fail the Agent receipt checkpoint after durable acknowledgement,
            // before the runner can reach Before(Task).
            std::fs::create_dir(blocked.join("saving")).unwrap();
        }));
        let send = id(&s.start().await);
        let failed = s.finish(&send).await;
        assert_eq!(failed["storageError"], true, "{failed}");
        assert_eq!(s.api.wire.posts(), 1, "no Task was submitted");
        std::fs::remove_dir(root.join("saving")).unwrap();
        discard_history(&s, &send, state).await;
        let before = writes(&s);
        let fresh = restart(&s);
        let check = reply(fresh.preflight(&s.target, &s.provider).await);
        assert_eq!(check["canSend"], true, "{state}: {check}");
        assert_eq!(check["action"], "unchanged");
        assert_eq!(writes(&s), before, "recovery must be GET-only");
        fresh.shutdown().await;
    }
}

#[tokio::test]
async fn reconciled_agent_without_tasks_does_not_depend_on_history() {
    for state in ["missing", "corrupt", "checkpoint-failure"] {
        let s = setup().await;
        *s.api.wire.mode.lock().unwrap() = "lose-agent";
        let send = id(&s.start().await);
        assert_eq!(s.finish(&send).await["error"], "submit");
        assert_eq!(s.api.wire.posts(), 1);
        let root = s.home.path().join("app-local/send-history-v1");
        if state == "checkpoint-failure" {
            std::fs::create_dir(root.join("saving")).unwrap();
        } else {
            discard_history(&s, &send, state).await;
        }
        *s.api.wire.mode.lock().unwrap() = "";
        let before = writes(&s);
        let check = reply(s.owner.preflight(&s.target, &s.provider).await);
        assert_eq!(check["canSend"], true, "{state}: {check}");
        if state == "checkpoint-failure" {
            std::fs::remove_dir(root.join("saving")).unwrap();
            discard_history(&s, &send, "missing").await;
        }
        let bindings: Value =
            serde_json::from_slice(&std::fs::read(root.join("bindings.json")).unwrap()).unwrap();
        assert!(bindings[0]["intent"].is_null());
        assert_eq!(bindings[0]["unfinished"], false);
        let fresh = restart(&s);
        let check = reply(fresh.preflight(&s.target, &s.provider).await);
        assert_eq!(check["canSend"], true, "{state}: {check}");
        assert_eq!(writes(&s), before, "intent reconciliation must be GET-only");
        fresh.shutdown().await;
    }
}

#[tokio::test]
async fn rejected_first_task_releases_binding_before_history_can_be_evicted() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "reject-task403";
    let send = id(&s.start().await);
    assert_eq!(s.finish(&send).await["error"], "submitRejected");
    let details = s.owner.history_details(&send).unwrap();
    assert!(!details.interrupted && details.can_remove);
    s.owner.history_remove(&send).await.unwrap();
    // Removed details are evictable; completion cannot depend on repairing them.
    discard_history(&s, &send, "missing").await;
    *s.api.wire.mode.lock().unwrap() = "";
    let before = s.agent_posts().len();
    let fresh = restart(&s);
    let check = reply(fresh.preflight(&s.target, &s.provider).await);
    assert_eq!(check["canSend"], true, "{check}");
    let started = reply(fresh.start(&s.target, &s.provider, s.source()).await);
    until(|| fresh.status(Some(&id(&started))).unwrap().state != SendState::Running).await;
    let done = reply(fresh.status(Some(&id(&started))).unwrap());
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["agentAction"], "unchanged");
    assert_eq!(s.agent_posts().len(), before);
    fresh.shutdown().await;
}

#[tokio::test]
async fn terminal_checkpoint_failure_keeps_binding_until_receipt_is_durable() {
    let s = setup().await;
    let root = s.home.path().join("app-local/send-history-v1");
    let blocked = root.clone();
    *s.api.wire.before_result.lock().unwrap() = Some(Box::new(move || {
        std::fs::create_dir(blocked.join("saving")).unwrap();
    }));
    let send = id(&s.start().await);
    assert_eq!(s.finish(&send).await["storageError"], true);
    std::fs::remove_dir(root.join("saving")).unwrap();
    std::fs::write(
        s.agent_path(),
        AGENT.replace("Fetch only.", "Changed instructions."),
    )
    .unwrap();
    let fresh = restart(&s);
    let before = writes(&s);
    let check = reply(fresh.preflight(&s.target, &s.provider).await);
    assert_eq!(
        check["canSend"], false,
        "terminal proof was not saved: {check}"
    );
    assert_eq!(writes(&s), before);
    let recovered = fresh.history_check(&send).await.unwrap();
    assert_eq!(recovered.report.unwrap().verdict, Verdict::Passed);
    let next = reply(fresh.start(&s.target, &s.provider, s.source()).await);
    until(|| fresh.status(Some(&id(&next))).unwrap().state != SendState::Running).await;
    assert_eq!(
        reply(fresh.status(Some(&id(&next))).unwrap())["state"],
        "complete"
    );
    assert_eq!(
        fresh
            .history_check(&send)
            .await
            .unwrap()
            .report
            .unwrap()
            .verdict,
        Verdict::Passed
    );
    fresh.shutdown().await;
}

#[tokio::test]
async fn terminal_receipt_survives_eviction_until_binding_completion() {
    use bridge_engine::send_summary::SummaryStore;
    for remove in [false, true] {
        let s = setup().await;
        let send = id(&s.start().await);
        assert_eq!(s.finish(&send).await["state"], "complete");
        let root = s.home.path().join("app-local/send-history-v1");
        let path = root.join("bindings.json");
        let mut bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // Crash after terminal checkpoint, before releasing the binding barrier.
        bindings[0]["unfinished"] = json!(true);
        std::fs::write(&path, serde_json::to_vec(&bindings).unwrap()).unwrap();
        let store = SummaryStore::new(Some(s.home.path().join("app-local")));
        let mut summary = s.owner.history().unwrap().remove(0);
        summary.time = 0;
        store.begin(summary.clone()).unwrap();
        for _ in 0..256 {
            summary.id = bridge_runtime_protocol::Uuid::new_v4().to_string();
            summary.time = 1;
            store.begin(summary.clone()).unwrap();
        }
        let fresh = restart(&s);
        assert!(
            fresh.history().unwrap().iter().any(|a| a.id == send),
            "keep the summary needed to load protected terminal evidence"
        );
        assert_eq!(
            fresh
                .history_details(&send)
                .unwrap()
                .report
                .unwrap()
                .verdict,
            Verdict::Passed
        );
        if remove {
            std::fs::rename(&path, root.join("bindings-backup")).unwrap();
            std::fs::create_dir(&path).unwrap();
            assert!(fresh.history_remove(&send).await.is_err());
            assert!(fresh.history_details(&send).unwrap().report.is_some());
            std::fs::remove_dir(&path).unwrap();
            std::fs::rename(root.join("bindings-backup"), &path).unwrap();
            fresh.history_remove(&send).await.unwrap();
        } else {
            let before = writes(&s);
            let check = reply(fresh.preflight(&s.target, &s.provider).await);
            assert_eq!(
                check["canSend"], true,
                "saved terminal proof resolves the barrier: {check}"
            );
            assert_eq!(writes(&s), before);
        }
        let bindings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(bindings[0]["unfinished"], false);
        summary.id = bridge_runtime_protocol::Uuid::new_v4().to_string();
        store.begin(summary).unwrap();
        assert!(
            !fresh.history().unwrap().iter().any(|a| a.id == send),
            "completed history remains evictable"
        );
        fresh.shutdown().await;
    }
}

#[tokio::test]
async fn lost_second_task_reply_keeps_restart_blocked_with_or_without_history() {
    for state in ["saved", "missing", "corrupt"] {
        let s = setup().await;
        let block = &TESTS[TESTS.find("```yaml agent-test").unwrap()..];
        std::fs::write(
            s.agent_path().with_file_name("web-reader.agent.test.md"),
            format!("{TESTS}\n{}", block.replace("id: fetch\n", "id: second\n")),
        )
        .unwrap();
        *s.api.wire.mode.lock().unwrap() = "mixed-pending";
        let send = id(&s.start().await);
        until(|| s.api.wire.posts() == 3).await;
        s.owner.stop(&send).await.unwrap();
        let saved = s.owner.history_details(&send).unwrap();
        assert!(saved.interrupted && saved.can_cancel);
        if state != "saved" {
            discard_history(&s, &send, state).await;
        }
        *s.api.wire.mode.lock().unwrap() = "";
        std::fs::write(
            s.agent_path(),
            AGENT.replace("Fetch only.", "Changed instructions."),
        )
        .unwrap();
        let before = writes(&s);
        let fresh = restart(&s);
        let started = reply(fresh.start(&s.target, &s.provider, s.source()).await);
        until(|| fresh.status(Some(&id(&started))).unwrap().state != SendState::Running).await;
        let blocked = reply(fresh.status(Some(&id(&started))).unwrap());
        assert_eq!(blocked["error"], "earlierWork", "{state}: {blocked}");
        assert_eq!(
            writes(&s),
            before,
            "unresolved Task must prevent Agent update"
        );
        if state == "saved" {
            assert!(
                fresh.history_details(&send).unwrap().can_cancel,
                "keep control receipts"
            );
        }
        fresh.shutdown().await;
    }
}
