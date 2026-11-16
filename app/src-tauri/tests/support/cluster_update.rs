//! B3 exercises the actual native port-forward, conditional HTTP and durable owner.
use super::*;

fn edit(s: &Setup) {
    std::fs::write(
        s.agent_path(),
        AGENT.replace("Fetch only.", "Changed instructions."),
    )
    .unwrap();
}
pub(super) async fn separate(s: &Setup) -> Value {
    let folder = Arc::new(AgentFolder::new(Some(s.home.path().into())));
    serde_json::to_value(
        s.owner
            .start_document_separate(folder, &s.target, &s.provider, s.source(), true)
            .await
            .view
            .unwrap(),
    )
    .unwrap()
}
fn puts(s: &Setup) -> Vec<(String, Value)> {
    s.api
        .wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.0.starts_with("PUT"))
        .cloned()
        .collect()
}
#[tokio::test]
async fn repeated_send_reuses_uid_updates_conditionally_and_preserves_archived_answers() {
    let s = setup().await;
    assert_eq!(
        reply(s.owner.preflight(&s.target, &s.provider).await)["action"],
        "create"
    );
    let first = s.finish(&id(&s.start().await)).await;
    assert_eq!(first["agentAction"], "create");
    let uid = first["acknowledged"][0]["uid"].clone();
    edit(&s);
    let check = reply(s.owner.preflight(&s.target, &s.provider).await);
    assert_eq!(check["action"], "update");
    assert_eq!(check["canSeparate"], true);
    let updated = s.finish(&id(&s.start().await)).await;
    assert_eq!(updated["state"], "complete", "{updated}");
    assert_eq!(updated["agentAction"], "update");
    assert_eq!(updated["acknowledged"][0]["uid"], uid);
    let updates = puts(&s);
    assert_eq!(updates.len(), 1);
    assert!(updates[0].0.contains("/spec?namespace=agents"));
    assert_eq!(updates[0].1["expectedUID"], uid);
    assert_eq!(updates[0].1["expectedGeneration"], 1);
    assert_eq!(
        updates[0].1["spec"]["systemPrompt"]["inline"],
        "Changed instructions.\n"
    );
    let old = s.owner.history_check(&id(&first)).await.unwrap();
    assert_eq!(old.report.unwrap().verdict, Verdict::Passed);
    let folder = Arc::new(AgentFolder::new(Some(s.home.path().into())));
    let fresh = SendOwner::with_history_root(
        folder,
        s.session.clone(),
        Some(s.home.path().join("app-local")),
    );
    s.owner.history_remove(&id(&updated)).await.unwrap();
    assert_eq!(
        reply(fresh.preflight(&s.target, &s.provider).await)["action"],
        "unchanged"
    );
    let next = reply(fresh.start(&s.target, &s.provider, s.source()).await);
    until(|| fresh.status(Some(&id(&next))).unwrap().state != SendState::Running).await;
    let next = reply(fresh.status(Some(&id(&next))).unwrap());
    assert_eq!(next["agentAction"], "unchanged");
    assert_eq!(next["acknowledged"][0]["uid"], uid);
    assert_ne!(
        next["acknowledged"][1]["uid"],
        first["acknowledged"][1]["uid"]
    );
    assert_eq!(s.agent_posts().len(), 1);
    assert_eq!(puts(&s).len(), 1);
}
#[tokio::test]
async fn archived_answers_recover_after_update_and_transient_read_failure() {
    let s = setup().await;
    let first = s.finish(&id(&s.start().await)).await;
    edit(&s);
    assert_eq!(s.finish(&id(&s.start().await)).await["state"], "complete");
    let writes = (s.api.wire.posts(), puts(&s).len());
    *s.api.wire.mode.lock().unwrap() = "result-403";
    let unavailable = s.owner.history_check(&id(&first)).await.unwrap();
    assert_eq!(unavailable.report.unwrap().verdict, Verdict::Unknown);
    *s.api.wire.mode.lock().unwrap() = "";
    let recovered = s.owner.history_check(&id(&first)).await.unwrap();
    assert_eq!(recovered.availability.as_deref(), Some("available"));
    assert_eq!(recovered.report.unwrap().verdict, Verdict::Passed);
    assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
}
#[tokio::test]
async fn failed_revision_check_cannot_promote_a_task_into_archived_mode() {
    let s = setup().await;
    let first = s.finish(&id(&s.start().await)).await;
    let send = id(&first);
    let path = s
        .home
        .path()
        .join(format!("app-local/send-history-v1/{send}.json"));
    use bridge_engine::send_summary::{AttemptDetails, AttemptVerdict, SummaryStore};
    let mut detail: AttemptDetails =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    detail.report = None;
    for resource in &mut detail.resources {
        if resource.kind == "Task" {
            resource.terminal = false;
        }
    }
    let mut summary = s.owner.history().unwrap().remove(0);
    summary.verdict = AttemptVerdict::Unknown;
    SummaryStore::new(Some(s.home.path().join("app-local")))
        .checkpoint(summary, detail)
        .unwrap();
    let api = s.api.clone();
    *s.api.wire.before_result.lock().unwrap() = Some(Box::new(move || {
        *api.wire.mode.lock().unwrap() = "agent-generation";
    }));
    let writes = (s.api.wire.posts(), puts(&s).len());
    let rejected = s.owner.history_check(&send).await.unwrap();
    assert!(
        rejected.interrupted,
        "failed final Agent verification cannot prove terminal receipt ownership"
    );
    assert_ne!(rejected.report.map(|r| r.verdict), Some(Verdict::Passed));
    let again = s.owner.history_check(&send).await.unwrap();
    assert_ne!(again.report.map(|r| r.verdict), Some(Verdict::Passed));
    assert!(!s.owner.history_details(&send).unwrap().can_remove);
    assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
}
#[tokio::test]
async fn acknowledged_binding_repairs_crash_gap_in_preflight_and_history_check() {
    for (preflight, missing) in [(true, false), (false, false), (true, true), (false, true)] {
        let s = setup().await;
        let first = s.finish(&id(&s.start().await)).await;
        let send = id(&first);
        let root = s.home.path().join("app-local/send-history-v1");
        let path = root.join(format!("{send}.json"));
        let mut detail: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let tasks = detail["resources"].as_array().unwrap()[1..].to_vec();
        if missing {
            detail["resources"].as_array_mut().unwrap().remove(0);
        } else {
            detail["resources"][0]["uid"] = Value::Null;
            detail["resources"][0]["generation"] = json!(0);
        }
        std::fs::write(&path, serde_json::to_vec(&detail).unwrap()).unwrap();
        let fresh = SendOwner::with_history_root(
            Arc::new(AgentFolder::new(Some(s.home.path().into()))),
            s.session.clone(),
            Some(s.home.path().join("app-local")),
        );
        let writes = (s.api.wire.posts(), puts(&s).len());
        if preflight {
            let check = reply(fresh.preflight(&s.target, &s.provider).await);
            assert_eq!(check["action"], "unchanged");
            assert_eq!(check["canSend"], true);
        } else {
            let recovered = fresh.history_check(&send).await.unwrap();
            assert!(!recovered.interrupted);
            assert_eq!(recovered.report.unwrap().verdict, Verdict::Passed);
        }
        let repaired: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            repaired["resources"][0]["uid"],
            first["acknowledged"][0]["uid"]
        );
        assert_eq!(&repaired["resources"].as_array().unwrap()[1..], tasks);
        assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
    }
}
#[tokio::test]
async fn initial_checkpoint_failure_restores_binding_before_any_mutation() {
    for action in ["create", "unchanged", "update", "separate"] {
        let s = setup().await;
        if action != "create" {
            s.finish(&id(&s.start().await)).await;
        }
        if action == "update" {
            edit(&s);
        }
        let root = s.home.path().join("app-local/send-history-v1");
        if action == "create" {
            // Let the store create its private directory/Windows ACL; an ambient
            // mkdir would test an invalid store on ordinary contributor umasks.
            use bridge_engine::send_summary::{
                AttemptSummary, AttemptVerdict, SummaryStore, TargetUids,
            };
            SummaryStore::new(Some(s.home.path().join("app-local")))
                .begin(AttemptSummary {
                    id: bridge_runtime_protocol::Uuid::new_v4().to_string(),
                    source: s.source(),
                    target_display: "earlier · agents".into(),
                    target_uids: TargetUids {
                        cluster_identity: Some("a".repeat(64)),
                        service_namespace: "control".into(),
                        service_uid: "old-service".into(),
                        deployment_namespace: "control".into(),
                        deployment_uid: "old-deployment".into(),
                        watched_namespace: "agents".into(),
                    },
                    provider: "earlier-model".into(),
                    planned_agent_name: "bridge-run-earlier".into(),
                    agent_uid: None,
                    verdict: AttemptVerdict::Unknown,
                    time: 1,
                })
                .unwrap();
        }
        let bindings = || -> Value {
            std::fs::read(root.join("bindings.json"))
                .map(|bytes| serde_json::from_slice(&bytes).unwrap())
                .unwrap_or(json!([]))
        };
        let before = bindings();
        let writes = (s.api.wire.posts(), puts(&s).len());
        std::fs::create_dir(root.join("saving")).unwrap();
        let started = if action == "separate" {
            separate(&s).await
        } else {
            s.start().await
        };
        let failed = s.finish(&id(&started)).await;
        assert_eq!(failed["storageError"], true, "{action}: {failed}");
        assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
        std::fs::remove_dir(root.join("saving")).unwrap();
        assert_eq!(bindings(), before, "{action}");
        let check = reply(s.owner.preflight(&s.target, &s.provider).await);
        assert_eq!(check["canSend"], true, "{action}: {check}");
        assert_eq!(s.finish(&id(&s.start().await)).await["state"], "complete");
    }
}
#[tokio::test]
async fn terminal_details_wait_for_durable_completion_before_removal_and_next_send() {
    for refresh in [false, true] {
        let s = setup().await;
        let root = s.home.path().join("app-local/send-history-v1");
        let blocked = root.clone();
        let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
            std::fs::rename(
                blocked.join("bindings.json"),
                blocked.join("bindings-backup"),
            )
            .unwrap();
            std::fs::create_dir(blocked.join("bindings.json")).unwrap();
        });
        let send = if refresh {
            let send = id(&s.start().await);
            s.finish(&send).await;
            let path = root.join(format!("{send}.json"));
            let mut detail: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            detail["resources"][1]["terminal"] = json!(false);
            std::fs::write(path, serde_json::to_vec(&detail).unwrap()).unwrap();
            let path = root.join("bindings.json");
            let mut bindings: Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            bindings[0]["unfinished"] = json!(true);
            std::fs::write(path, serde_json::to_vec(&bindings).unwrap()).unwrap();
            *s.api.wire.before_result.lock().unwrap() = Some(hook);
            let _ = s.owner.history_check(&send).await;
            send
        } else {
            *s.api.wire.before_result.lock().unwrap() = Some(hook);
            let send = id(&s.start().await);
            s.finish(&send).await;
            send
        };
        assert!(
            s.owner.history_details(&send).unwrap().interrupted,
            "refresh={refresh}"
        );
        assert!(s.owner.history_remove(&send).await.is_err());
        std::fs::remove_dir(root.join("bindings.json")).unwrap();
        std::fs::rename(root.join("bindings-backup"), root.join("bindings.json")).unwrap();
        let recovered = s.owner.history_check(&send).await.unwrap();
        assert!(!recovered.interrupted);
        let bindings: Value =
            serde_json::from_slice(&std::fs::read(root.join("bindings.json")).unwrap()).unwrap();
        assert_eq!(bindings[0]["unfinished"], false);
        s.owner.history_remove(&send).await.unwrap();
        assert_eq!(
            reply(s.owner.preflight(&s.target, &s.provider).await)["action"],
            "unchanged"
        );
        assert_eq!(s.finish(&id(&s.start().await)).await["state"], "complete");
        assert_eq!(s.agent_posts().len(), 1);
    }
}
#[tokio::test]
async fn definite_create_rejections_do_not_leave_uncertain_bindings_or_tasks() {
    for mode in [
        "reject-agent400",
        "reject-agent403",
        "reject-agent409",
        "reject-task403",
    ] {
        let s = setup().await;
        let reused = mode.starts_with("reject-task");
        if reused {
            s.finish(&id(&s.start().await)).await;
        }
        *s.api.wire.mode.lock().unwrap() = mode;
        let rejected = s.finish(&id(&s.start().await)).await;
        assert_eq!(rejected["state"], "unknown");
        assert_eq!(rejected["error"], "submitRejected");
        *s.api.wire.mode.lock().unwrap() = "";
        let check = reply(s.owner.preflight(&s.target, &s.provider).await);
        assert_eq!(
            check["action"],
            if reused { "unchanged" } else { "create" },
            "{mode}: {check}"
        );
        assert_eq!(check["canSend"], true);
        assert!(!s.owner.history_details(&id(&rejected)).unwrap().interrupted);
        let retry = s.finish(&id(&s.start().await)).await;
        assert_eq!(retry["state"], "complete");
    }
}
#[tokio::test]
async fn conditional_rejections_never_create_tasks_or_fallback_agents() {
    for (mode, stage) in [
        ("update409", "updateConflict"),
        ("update404", "updateUnsupported"),
        ("update405", "updateUnsupported"),
        ("update400", "updateInvalid"),
        ("update403", "updateDenied"),
        ("update-missing", "updateMissing"),
    ] {
        let s = setup().await;
        s.finish(&id(&s.start().await)).await;
        edit(&s);
        let before = s.api.wire.posts();
        *s.api.wire.mode.lock().unwrap() = mode;
        let failed = s.finish(&id(&s.start().await)).await;
        assert_eq!(failed["error"], stage, "{mode}: {failed}");
        assert_eq!(s.api.wire.posts(), before);
        assert_eq!(s.agent_posts().len(), 1);
        assert_eq!(puts(&s).len(), 1);
        let history = s.owner.history_details(&id(&failed)).unwrap();
        assert!(!history.interrupted);
        *s.api.wire.mode.lock().unwrap() = "";
        let separated = s.finish(&id(&separate(&s).await)).await;
        assert_eq!(separated["state"], "complete");
        assert_eq!(s.agent_posts().len(), 2);
    }
}
#[tokio::test]
async fn outside_edits_and_replacements_block_before_put() {
    for mode in ["agent-generation", "agent-replaced"] {
        let s = setup().await;
        s.finish(&id(&s.start().await)).await;
        edit(&s);
        *s.api.wire.mode.lock().unwrap() = mode;
        let failed = s.finish(&id(&s.start().await)).await;
        assert_eq!(failed["error"], "updateConflict");
        assert!(puts(&s).is_empty());
        assert_eq!(s.api.wire.posts(), 2);
    }
}
#[tokio::test]
async fn lost_agent_responses_reconcile_read_only_but_old_or_missing_never_authorize_replay() {
    for mode in ["lose-agent", "update-lost", "update-lost-old", "broken"] {
        let s = setup().await;
        let known_uid = if mode.starts_with("update") {
            let first = s.finish(&id(&s.start().await)).await;
            edit(&s);
            Some(
                first["acknowledged"][0]["uid"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            )
        } else {
            None
        };
        *s.api.wire.mode.lock().unwrap() = mode;
        let failed = s.finish(&id(&s.start().await)).await;
        assert_eq!(failed["state"], "unknown");
        let history = s.owner.history().unwrap();
        let attempted = history.iter().find(|row| row.id == id(&failed)).unwrap();
        assert_eq!(
            attempted.agent_uid, known_uid,
            "an uncertain update must supersede the earlier UID's passing association"
        );
        let writes = (s.api.wire.posts(), puts(&s).len());
        *s.api.wire.mode.lock().unwrap() = "";
        let check = reply(s.owner.preflight(&s.target, &s.provider).await);
        assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
        if mode == "lose-agent" || mode == "update-lost" {
            assert_eq!(check["action"], "unchanged", "{mode}: {check}");
            let next = s.finish(&id(&s.start().await)).await;
            assert_eq!(next["state"], "complete");
            assert_eq!(s.agent_posts().len(), 1);
            assert_eq!(puts(&s).len(), writes.1);
        } else {
            assert_eq!(check["action"], "blocked");
            assert_eq!(check["canSeparate"], true);
            let retry = s.finish(&id(&s.start().await)).await;
            assert_eq!(retry["error"], "updateUnconfirmed");
            assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
            let old = s.owner.history_details(&id(&failed)).unwrap();
            assert!(old.interrupted);
            let separated = s.finish(&id(&separate(&s).await)).await;
            assert_eq!(separated["state"], "complete");
            assert!(s.owner.history_details(&id(&failed)).unwrap().interrupted);
        }
    }
}
#[tokio::test]
async fn named_patch_review_is_bearer_only_and_never_gates_unchanged_reruns() {
    let s = setup().await;
    let first = s.finish(&id(&s.start().await)).await;
    *s.api.wire.mode.lock().unwrap() = "token-review";
    std::fs::write(
        s.agent_path().with_file_name("web-reader.agent.test.md"),
        TESTS.replace("expected answer", "changed expectation"),
    )
    .unwrap();
    s.api.calls.lock().unwrap().clear();
    let again = s.finish(&id(&s.start().await)).await;
    assert_eq!(again["state"], "complete");
    assert!(!s
        .api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/selfsubjectaccessreviews")));
    edit(&s);
    *s.api.deny.lock().unwrap() = true;
    let denied = s.finish(&id(&s.start().await)).await;
    assert_eq!(denied["error"], "updateDenied");
    assert!(puts(&s).is_empty());
    let calls = s.api.calls.lock().unwrap();
    let review = calls
        .iter()
        .find(|c| c.1.ends_with("/selfsubjectaccessreviews"))
        .unwrap();
    assert_eq!(review.2, "Bearer creator-fixture");
    assert_eq!(
        review.3["spec"]["resourceAttributes"],
        json!({"group":"core.orka.ai","resource":"agents","verb":"patch","name":first["agentName"],"namespace":"agents"})
    );
}
#[tokio::test]
async fn reused_agent_lost_task_response_recovers_after_restart_without_posting_again() {
    let s = setup().await;
    let first = s.finish(&id(&s.start().await)).await;
    s.api.wire.lose_task_reply.store(1, Ordering::SeqCst);
    let lost = s.finish(&id(&s.start().await)).await;
    assert_eq!(lost["state"], "unknown");
    assert_eq!(
        lost["acknowledged"][0]["uid"],
        first["acknowledged"][0]["uid"]
    );
    s.api.wire.lose_task_reply.store(0, Ordering::SeqCst);
    let fresh = SendOwner::with_history_root(
        Arc::new(AgentFolder::new(Some(s.home.path().into()))),
        s.session.clone(),
        Some(s.home.path().join("app-local")),
    );
    let before = s.api.wire.posts();
    let checked = fresh.history_check(&id(&lost)).await.unwrap();
    assert!(!checked.interrupted);
    assert_eq!(checked.report.unwrap().verdict, Verdict::Passed);
    assert_eq!(s.api.wire.posts(), before);
    assert_eq!(s.agent_posts().len(), 1);
    assert_eq!(
        reply(fresh.preflight(&s.target, &s.provider).await)["action"],
        "unchanged"
    );
}
#[tokio::test]
async fn valid_update_receipt_can_keep_the_generation_after_server_normalization() {
    let s = setup().await;
    let first = s.finish(&id(&s.start().await)).await;
    edit(&s);
    *s.api.wire.mode.lock().unwrap() = "update-noop";
    let updated = s.finish(&id(&s.start().await)).await;
    assert_eq!(updated["state"], "complete", "{updated}");
    assert_eq!(
        updated["acknowledged"][0]["uid"],
        first["acknowledged"][0]["uid"]
    );
    assert_eq!(puts(&s).len(), 1);
}
#[tokio::test]
async fn unfinished_bound_tasks_block_normal_send_but_separate_preserves_control_receipts() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "running";
    let first = s.start().await;
    until(|| s.view(None)["stage"] == "observe").await;
    s.owner.stop(&id(&first)).await;
    let old = s.owner.history_details(&id(&first)).unwrap();
    assert!(old.interrupted);
    *s.api.wire.mode.lock().unwrap() = "";
    edit(&s);
    let blocked = s.finish(&id(&s.start().await)).await;
    assert_eq!(blocked["error"], "earlierWork");
    assert!(puts(&s).is_empty());
    let separate = s.finish(&id(&separate(&s).await)).await;
    assert_eq!(separate["state"], "complete");
    assert!(s.owner.history_details(&id(&first)).unwrap().can_cancel);
    let same = s.finish(&id(&s.start().await)).await;
    assert_eq!(same["state"], "complete");
    assert_eq!(s.agent_posts().len(), 2);
}
#[tokio::test]
async fn stop_or_quit_during_the_agent_update_returns_without_tasks_and_stays_uncertain() {
    for quit in [false, true] {
        let s = setup().await;
        s.finish(&id(&s.start().await)).await;
        edit(&s);
        *s.api.wire.mode.lock().unwrap() = "update-hang";
        let started = id(&s.start().await);
        until(|| puts(&s).len() == 1).await;
        let writes = s.api.wire.posts();
        // A pending PUT used to ignore Stop until the operation deadline.
        tokio::time::timeout(Duration::from_secs(5), async {
            if quit {
                s.owner.shutdown().await
            } else {
                s.owner.stop(&started).await;
            }
        })
        .await
        .expect("Stop must interrupt the pending update");
        let stopped = s.view(Some(&started));
        assert_eq!(stopped["state"], "unknown", "quit={quit}: {stopped}");
        assert_eq!(stopped["error"], "updateUnconfirmed");
        assert_eq!((s.api.wire.posts(), puts(&s).len()), (writes, 1));
        *s.api.wire.mode.lock().unwrap() = "";
        let check = reply(s.owner.preflight(&s.target, &s.provider).await);
        assert_eq!(check["action"], "blocked", "quit={quit}: {check}");
        assert_eq!(check["canSeparate"], true);
        assert!(s.owner.history_details(&started).unwrap().interrupted);
        assert_eq!((s.api.wire.posts(), puts(&s).len()), (writes, 1));
    }
}
#[tokio::test]
async fn see_in_overview_reads_only_the_recorded_agent_by_send_id() {
    let s = setup().await;
    let first = id(&s.start().await);
    let done = s.finish(&first).await;
    let uid = done["acknowledged"][0]["uid"].as_str().unwrap().to_string();
    let focus = s.owner.agent_focus(&first).unwrap();
    assert_eq!(
        (focus.uid.as_str(), focus.namespace.as_str()),
        (uid.as_str(), "agents")
    );
    assert_eq!(focus.name, done["agentName"].as_str().unwrap());
    let writes = (s.api.wire.posts(), puts(&s).len());
    assert_eq!(
        s.owner.agent_check(&first).await.unwrap(),
        AgentCheck::Present
    );
    s.idle_forwards().await;
    // A same-named replacement is another agent; the recorded one is gone.
    let path = format!("/api/v1/agents/{}", focus.name);
    let kept = s
        .api
        .wire
        .objects
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap();
    s.api.wire.objects.lock().unwrap().get_mut(&path).unwrap()["metadata"]["uid"] =
        json!("replacement");
    assert_eq!(s.owner.agent_check(&first).await.unwrap(), AgentCheck::Gone);
    s.idle_forwards().await;
    s.api.wire.objects.lock().unwrap().remove(&path);
    assert_eq!(s.owner.agent_check(&first).await.unwrap(), AgentCheck::Gone);
    s.idle_forwards().await;
    // Unconfirmed access is never read as deletion.
    s.api
        .wire
        .objects
        .lock()
        .unwrap()
        .insert(path.clone(), kept.clone());
    // A malformed answer (no UID, or another name) proves nothing about deletion.
    s.api.wire.objects.lock().unwrap().get_mut(&path).unwrap()["metadata"]["uid"] = Value::Null;
    assert_eq!(
        s.owner.agent_check(&first).await.unwrap(),
        AgentCheck::CannotCheck
    );
    s.idle_forwards().await;
    s.api
        .wire
        .objects
        .lock()
        .unwrap()
        .insert(path.clone(), kept.clone());
    s.api.wire.objects.lock().unwrap().get_mut(&path).unwrap()["metadata"]["name"] =
        json!("someone-else");
    assert_eq!(
        s.owner.agent_check(&first).await.unwrap(),
        AgentCheck::CannotCheck
    );
    s.idle_forwards().await;
    s.api.wire.objects.lock().unwrap().insert(path, kept);
    s.fresh.lock().unwrap().clusters[0]
        .cluster
        .as_mut()
        .unwrap()
        .server = Some("https://different.invalid".into());
    assert_eq!(
        s.owner.agent_check(&first).await.unwrap(),
        AgentCheck::CannotCheck
    );
    assert_eq!((s.api.wire.posts(), puts(&s).len()), writes);
    assert!(s.owner.agent_focus("not-a-send").is_err());
}
#[tokio::test]
async fn an_unacknowledged_first_create_has_no_agent_to_open_but_a_stopped_update_keeps_its_owner()
{
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "reject-agent403";
    let rejected = id(&s.start().await);
    s.finish(&rejected).await;
    assert!(s.owner.agent_focus(&rejected).is_err());
    *s.api.wire.mode.lock().unwrap() = "";
    let first = id(&s.start().await);
    let owned = s.finish(&first).await["acknowledged"][0]["uid"].clone();
    edit(&s);
    *s.api.wire.mode.lock().unwrap() = "update-hang";
    let stopped = id(&s.start().await);
    until(|| puts(&s).len() == 1).await;
    s.owner.stop(&stopped).await;
    // The view may lack the receipt, but the attempt still records the agent it was updating.
    assert_eq!(
        s.owner.agent_focus(&stopped).unwrap().uid,
        owned.as_str().unwrap()
    );
}
