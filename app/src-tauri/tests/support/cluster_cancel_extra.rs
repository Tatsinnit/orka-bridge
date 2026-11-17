//! Durable, partial-submission and admission regressions for the cancellation owner.
use super::*;

pub(super) fn set_cases(s: &Setup, count: usize) {
    let source = ParsedPair::parse(AGENT.as_bytes(), TESTS.as_bytes()).unwrap();
    let first = &source.cases()[0].id;
    let block = &TESTS[TESTS.find("```yaml agent-test").unwrap()..];
    let mut tests = TESTS.to_owned();
    for n in 1..count {
        tests.push_str(&format!(
            "\n{}",
            block.replace(&format!("id: {first}\n"), &format!("id: extra-{n}\n"))
        ));
    }
    std::fs::write(
        s.agent_path().with_file_name("web-reader.agent.test.md"),
        tests,
    )
    .unwrap();
}

async fn mixed_terminal_failure(explicit: bool) {
    let s = setup().await;
    set_cases(&s, 3);
    *s.api.wire.mode.lock().unwrap() = "running";
    let send = id(&s.start().await);
    until(|| s.view(Some(&send))["stage"] == "observe").await;
    let path = s
        .home
        .path()
        .join("app-local/send-history-v1")
        .join(format!("{send}.json"));
    let saved: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let names: Vec<_> = saved["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "Task")
        .map(|r| r["name"].as_str().unwrap().to_owned())
        .collect();
    {
        let mut objects = s.api.wire.objects.lock().unwrap();
        for (name, phase) in names.iter().zip([
            "Failed",
            "Succeeded",
            if explicit { "Running" } else { "Cancelled" },
        ]) {
            objects.get_mut(&format!("/api/v1/tasks/{name}")).unwrap()["status"]["phase"] =
                json!(phase);
        }
    }
    *s.api.wire.mode.lock().unwrap() = "result-404";
    if explicit {
        cancel_active(&s, &send).await;
    }
    let done = s.finish(&send).await;
    assert_eq!(done["report"]["cases"][0]["verdict"], "failed", "{done}");
    assert_eq!(done["report"]["cases"][1]["verdict"], "unknown", "{done}");
    assert_eq!(done["report"]["cases"][2]["verdict"], "cancelled", "{done}");
    assert_eq!(done["report"]["verdict"], "failed", "{done}");
    let detail = serde_json::to_value(s.owner.history_check(&send).await.unwrap()).unwrap();
    assert_eq!(detail["report"]["verdict"], "failed", "{detail}");
    assert_eq!(
        s.owner.history().unwrap()[0].verdict,
        bridge_engine::send_summary::AttemptVerdict::Failed
    );
}

#[tokio::test]
async fn cluster_cancel_keeps_a_verified_failure_beside_unknown_and_cancelled_cases() {
    mixed_terminal_failure(true).await;
}

#[tokio::test]
async fn external_cancel_does_not_hide_a_cluster_failure() {
    mixed_terminal_failure(false).await;
}

#[tokio::test]
async fn terminal_receipts_do_not_promote_a_partial_cancelled_report() {
    let s = setup().await;
    set_cases(&s, 2);
    *s.api.wire.mode.lock().unwrap() = "running";
    let send = id(&s.start().await);
    until(|| s.view(Some(&send))["stage"] == "observe").await;
    cancel_active(&s, &send).await;
    s.finish(&send).await;
    let path = s
        .home
        .path()
        .join("app-local/send-history-v1")
        .join(format!("{send}.json"));
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let unread = saved["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == "Task")
        .nth(1)
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();
    // Reproduce a partial recovery: terminal receipts survived, but one outcome did not.
    saved["report"]["cases"].as_array_mut().unwrap().truncate(1);
    saved["report"]["verdict"] = json!("unknown");
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    s.api
        .wire
        .objects
        .lock()
        .unwrap()
        .remove(&format!("/api/v1/tasks/{unread}"));
    let detail = serde_json::to_value(s.owner.history_check(&send).await.unwrap()).unwrap();
    assert_eq!(detail["cancellation"], "confirmed");
    assert_eq!(detail["report"]["cases"].as_array().unwrap().len(), 1);
    assert_eq!(detail["report"]["verdict"], "unknown", "{detail}");
    assert_eq!(
        s.owner.history().unwrap()[0].verdict,
        bridge_engine::send_summary::AttemptVerdict::Unknown
    );
}

#[tokio::test]
async fn denied_partial_submission_recovers_not_run_rows_without_replaying_cancel() {
    let s = setup().await;
    set_cases(&s, 3);
    *s.api.wire.mode.lock().unwrap() = "mixed-pending";
    let send = id(&s.start().await);
    until(|| s.api.wire.posts() == 3).await;
    until(|| {
        s.api
            .wire
            .objects
            .lock()
            .unwrap()
            .values()
            .filter(|o| o["kind"] == "Task")
            .count()
            == 2
    })
    .await;
    *s.api.forward_status.lock().unwrap() = Some(403);
    cancel_active(&s, &send).await;
    assert_eq!(s.finish(&send).await["cancellation"], "denied");
    *s.api.forward_status.lock().unwrap() = None;
    *s.api.wire.mode.lock().unwrap() = "";
    for task in s
        .api
        .wire
        .objects
        .lock()
        .unwrap()
        .values_mut()
        .filter(|o| o["kind"] == "Task")
    {
        task["status"]["phase"] = json!("Cancelled");
    }
    let before = s.api.wire.posts();
    let detail = serde_json::to_value(s.owner.history_check(&send).await.unwrap()).unwrap();
    let rows = detail["report"]["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{detail}");
    assert_eq!(rows[2]["phase"], "NotRun");
    assert_eq!(detail["report"]["verdict"], "cancelled");
    assert_eq!(detail["canCancel"], false);
    assert_eq!(
        s.api.wire.posts(),
        before,
        "status recovery must stay GET-only"
    );
}

#[tokio::test]
async fn control_open_denial_remains_denied_with_unfinished_work() {
    let (s, send) = running().await;
    *s.api.forward_status.lock().unwrap() = Some(403);
    cancel_active(&s, &send).await;
    let done = s.finish(&send).await;
    assert_eq!(done["cancellation"], "denied", "{done}");
    assert_eq!(done["state"], "unknown");
    assert_eq!(cancels(&s), 0);
    assert!(s.owner.history_details(&send).unwrap().interrupted);
}

#[tokio::test]
async fn newly_created_task_with_missing_or_empty_phase_is_cancelled() {
    for phase in [None, Some("")] {
        let (s, send) = running().await;
        {
            let mut objects = s.api.wire.objects.lock().unwrap();
            let task = objects.values_mut().find(|o| o["kind"] == "Task").unwrap();
            if let Some(phase) = phase {
                task["status"]["phase"] = json!(phase);
            } else {
                task.as_object_mut().unwrap().remove("status");
            }
        }
        cancel_active(&s, &send).await;
        let done = s.finish(&send).await;
        assert_eq!(cancels(&s), 1, "{phase:?}: {done}");
        assert_eq!(done["cancellation"], "confirmed");
        assert_eq!(done["report"]["verdict"], "cancelled");
    }
}

#[tokio::test]
async fn confirmed_control_with_failed_read_forward_never_exports_an_empty_report() {
    let (s, send) = running().await;
    let api = s.api.clone();
    *s.api.wire.before_cancel.lock().unwrap() = Some(Box::new(move || {
        *api.forward_status.lock().unwrap() = Some(503);
    }));
    cancel_active(&s, &send).await;
    let done = s.finish(&send).await;
    assert_eq!(cancels(&s), 1);
    assert_eq!(done["cancellation"], "confirmed");
    assert_eq!(done["report"], Value::Null, "{done}");
    assert!(s.owner.history_details(&send).unwrap().report.is_none());
}

#[tokio::test]
async fn cancel_pending_create_adopts_uid_before_post_keeps_answer_and_marks_unsubmitted_not_run() {
    let s = setup().await;
    set_cases(&s, 3);
    *s.api.wire.mode.lock().unwrap() = "mixed-pending";
    let send = id(&s.start().await);
    until(|| s.api.wire.posts() == 3).await;
    // The second Task exists but its create acknowledgement is blocked.
    until(|| {
        s.api
            .wire
            .objects
            .lock()
            .unwrap()
            .values()
            .filter(|o| o["kind"] == "Task")
            .count()
            == 2
    })
    .await;
    let sidecar = s
        .home
        .path()
        .join("app-local/send-history-v1")
        .join(format!("{send}.json"));
    let checked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let proof = checked.clone();
    *s.api.wire.before_cancel.lock().unwrap() = Some(Box::new(move || {
        let saved: Value = serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
        assert_eq!(saved["cancellation"], "cancelling");
        assert!(saved["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "Task")
            .all(|r| r["uid"].is_string()));
        proof.store(true, Ordering::SeqCst);
    }));
    cancel_active(&s, &send).await;
    let done = s.finish(&send).await;
    assert!(checked.load(Ordering::SeqCst));
    assert_eq!(done["cancellation"], "confirmed", "{done}");
    let rows = done["report"]["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["phase"], "Succeeded");
    assert_eq!(rows[0]["answer"], "expected answer");
    assert_eq!(rows[0]["verdict"], "passed");
    assert_eq!(rows[1]["phase"], "Cancelled");
    assert_eq!(rows[1]["verdict"], "cancelled");
    assert_eq!(rows[2]["phase"], "NotRun");
    assert_eq!(rows[2]["verdict"], "unknown");
    assert!(rows[1..].iter().all(|r| r["assertions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["verdict"] == "unknown")));
    assert_eq!(done["report"]["verdict"], "cancelled");
    assert_eq!(
        s.api.wire.posts(),
        4,
        "Agent, two Tasks and one cancel only"
    );
    assert_eq!(
        s.owner.history().unwrap()[0].verdict,
        bridge_engine::send_summary::AttemptVerdict::Cancelled
    );
}

#[tokio::test]
async fn uncertain_create_missing_or_changed_spec_is_never_cancelled_or_replayed() {
    for fault in ["missing", "spec", "namespace", "agent"] {
        let s = setup().await;
        s.api.wire.lose_task_reply.store(1, Ordering::SeqCst);
        let send = id(&s.start().await);
        s.finish(&send).await;
        {
            let mut objects = s.api.wire.objects.lock().unwrap();
            let key = objects
                .iter()
                .find(|(_, o)| o["kind"] == "Task")
                .unwrap()
                .0
                .clone();
            match fault {
                "missing" => {
                    objects.remove(&key);
                }
                "spec" => {
                    objects.get_mut(&key).unwrap()["spec"]["prompt"] = json!("foreign work");
                }
                "namespace" => {
                    objects.get_mut(&key).unwrap()["metadata"]["namespace"] = json!("other");
                }
                "agent" => {
                    objects.values_mut().find(|o| o["kind"] == "Agent").unwrap()["metadata"]
                        ["uid"] = json!("foreign-agent");
                }
                _ => unreachable!(),
            }
        }
        let before = s.api.wire.posts();
        let detail = serde_json::to_value(s.owner.history_cancel(&send).await.unwrap()).unwrap();
        assert_eq!(detail["cancellation"], "unconfirmed", "{fault}: {detail}");
        assert_eq!(cancels(&s), 0);
        assert_eq!(s.api.wire.posts(), before, "no automatic replay: {fault}");
        assert!(detail["canCancel"].as_bool().unwrap());
        assert_ne!(s.view(Some(&send))["report"]["verdict"], "passed");
    }
}

#[tokio::test]
async fn shutdown_after_intent_waits_for_control_and_no_new_send_is_admitted() {
    let (s, send) = running().await;
    *s.api.wire.mode.lock().unwrap() = "hold-history";
    let before = s.api.wire.forwards.load(Ordering::SeqCst);
    cancel_active(&s, &send).await;
    // The independent control stream has opened and is blocked on GET.
    until(|| s.api.wire.forwards.load(Ordering::SeqCst) > before).await;
    let admitted = s.start().await;
    assert_eq!(id(&admitted), send, "cancellation retains admission");
    let owner = s.owner.clone();
    let shutdown = tokio::spawn(async move { owner.shutdown().await });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    *s.api.wire.mode.lock().unwrap() = "";
    shutdown.await.unwrap();
    assert_eq!(s.view(Some(&send))["cancellation"], "confirmed");
    assert_eq!(cancels(&s), 1);
    s.idle_forwards().await;
}

#[tokio::test]
async fn requested_is_not_replayed_until_explicit_retry_and_completed_answers_stay_visible() {
    let s = setup().await;
    set_cases(&s, 2);
    *s.api.wire.mode.lock().unwrap() = "running";
    let send = id(&s.start().await);
    until(|| s.view(Some(&send))["stage"] == "observe").await;
    // Let the first observed Task finish. Its checkpoint must publish during observation.
    let first = s
        .api
        .wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|(p, _)| p.starts_with("POST /api/v1/tasks?"))
        .unwrap()
        .1["name"]
        .as_str()
        .unwrap()
        .to_string();
    s.api
        .wire
        .objects
        .lock()
        .unwrap()
        .get_mut(&format!("/api/v1/tasks/{first}"))
        .unwrap()["status"]["phase"] = json!("Succeeded");
    until(|| {
        s.view(Some(&send))["report"]["cases"]
            .as_array()
            .is_some_and(|r| r.len() == 1)
    })
    .await;
    *s.api.wire.mode.lock().unwrap() = "cancel-running";
    cancel_active(&s, &send).await;
    let done = s.finish(&send).await;
    assert_eq!(done["cancellation"], "requested");
    assert_eq!(done["report"]["verdict"], "unknown");
    assert_eq!(done["report"]["cases"][0]["answer"], "expected answer");
    assert_eq!(cancels(&s), 1);
    s.owner.history_check(&send).await.unwrap();
    assert_eq!(cancels(&s), 1);
    *s.api.wire.mode.lock().unwrap() = "";
    s.owner.history_cancel(&send).await.unwrap();
    assert_eq!(cancels(&s), 2);
    let view = s.view(Some(&send));
    assert_eq!(view["cancellation"], "confirmed");
    assert_eq!(view["report"]["cases"][0]["answer"], "expected answer");
    assert_eq!(view["report"]["verdict"], "cancelled");
}

#[tokio::test]
async fn cancel_requires_matching_document_and_storage_before_first_write() {
    let (s, send) = running().await;
    let other = created_agent::created(s.home.path(), "other", AGENT, TESTS);
    assert!(s.owner.cancel_document(other.document_id(), &send).is_err());
    assert_eq!(s.view(Some(&send))["cancellation"], Value::Null);
    // Replacing a sidecar by a directory is a storage error, not cancellation authority.
    let path = s
        .home
        .path()
        .join("app-local/send-history-v1")
        .join(format!("{send}.json"));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    cancel_active(&s, &send).await;
    let view = s.finish(&send).await;
    assert_eq!(cancels(&s), 0);
    assert!(view["storageError"].as_bool().unwrap());
    assert_eq!(view["cancellation"], "unconfirmed", "{view}");
    assert_ne!(view["report"]["verdict"], "passed");
}
