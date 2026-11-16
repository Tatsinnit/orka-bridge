use bridge_engine::{
    agent_folder::DocumentId,
    chat::{ChatError, ChatOwner, ChatSnapshot, ChatState},
};
use bridge_runtime_protocol::ChatMethod;
use std::sync::Arc;
#[path = "support/chat_fixture.rs"]
mod fixture;
use fixture::Fixture;

fn owner() -> Arc<ChatOwner> {
    ChatOwner::new(tokio::runtime::Handle::current())
}
async fn until(
    chat: &ChatOwner,
    id: DocumentId,
    predicate: impl Fn(&ChatSnapshot) -> bool,
) -> ChatSnapshot {
    for _ in 0..800 {
        let s = chat.status(id);
        if predicate(&s) {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let s = chat.status(id);
    panic!("Chat never reached expected state: {:?}", s);
}
async fn ready(chat: &ChatOwner, id: DocumentId) -> ChatSnapshot {
    until(chat, id, |s| s.can_send && s.state == ChatState::Ready).await
}

// A fresh Agent or Session on the second turn breaks native conversational context.
#[tokio::test]
async fn two_turns_reuse_frozen_agent_and_session_and_drain_cursor_events() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "First question".into())
        .unwrap();
    let first = ready(&chat, id).await;
    assert_eq!(
        first.versions[0].turns[0].answer.as_deref(),
        Some("A real observed answer.")
    );
    assert!(first.versions[0].turns[0].context_retained);
    assert_eq!(first.versions[0].turns[0].tools.len(), 2);
    chat.send(f.vm.clone(), f.folder.clone(), "Follow-up".into())
        .unwrap();
    let second = ready(&chat, id).await;
    assert_eq!(second.versions.len(), 1);
    assert_eq!(second.versions[0].turns.len(), 2);
    let g = f.guest.lock().unwrap();
    assert_eq!(g.posts("agents").len(), 1);
    let tasks = g.posts("tasks");
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        tasks[0]["spec"]["sessionRef"]["name"],
        tasks[1]["spec"]["sessionRef"]["name"]
    );
    assert_eq!(tasks[0]["spec"]["sessionRef"]["create"], true);
    assert!(tasks[1]["spec"]["sessionRef"].get("create").is_none());
    assert_eq!(tasks[0]["spec"]["sessionRef"]["maxMessages"], 50);
    assert_eq!(tasks[0]["spec"]["retryPolicy"]["maxRetries"], 0);
    assert_eq!(tasks[0]["spec"]["timeout"], "120s");
    assert_eq!(tasks[0]["spec"].as_object().unwrap().len(), 6);
    assert!(g.requests.iter().any(|r| r.after == Some(1)));
}

#[tokio::test]
async fn approval_tool_preflight_preserves_context_and_removal_restores_chat() {
    for (kind, name) in [
        ("builtin", "request_approval"),
        ("tool", "request_approval"),
        ("builtin", " request_approval "),
    ] {
        let f = Fixture::new().await;
        let chat = owner();
        let id = f.folder.document_id();
        chat.send(f.vm.clone(), f.folder.clone(), "First question".into())
            .unwrap();
        ready(&chat, id).await;
        let original = f.folder.open().unwrap().agent;
        let source = original.replace(
            "  tools:",
            &format!("  tools:\n    - kind: {kind}\n      name: '{name}'"),
        );
        assert_ne!(source, original);
        f.folder.save(source.clone()).unwrap();
        chat.send(f.vm.clone(), f.folder.clone(), "Unsupported".into())
            .unwrap();
        let rejected = until(&chat, id, |s| !s.busy).await;
        assert_eq!(
            serde_json::to_value(rejected.error).unwrap(),
            "unsupportedTool"
        );
        assert_eq!(rejected.state, ChatState::Ready);
        assert!(rejected.can_send);
        assert_eq!(rejected.versions.len(), 1);
        assert_eq!(rejected.versions[0].turns.len(), 1);
        assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
        assert_eq!(f.guest.lock().unwrap().posts("agents").len(), 1);
        assert_eq!(f.folder.open().unwrap().agent, source);
        f.folder.save(original).unwrap();
        chat.send(f.vm.clone(), f.folder.clone(), "Follow-up".into())
            .unwrap();
        let passed = ready(&chat, id).await;
        assert_eq!(passed.error, None);
        assert_eq!(passed.versions.len(), 1);
        assert_eq!(passed.versions[0].turns.len(), 2);
        assert_eq!(
            passed.versions[0].turns[1].answer.as_deref(),
            Some("A real observed answer.")
        );
        assert_eq!(f.guest.lock().unwrap().posts("agents").len(), 1);
        assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 2);
    }
}

// Status/remount and a dropped send waiter may not create duplicate remote work.
#[tokio::test]
async fn status_is_read_only_and_workers_outlive_callers() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    assert!(chat.status(id).versions.is_empty());
    assert!(f.guest.lock().unwrap().requests.is_empty());
    drop(
        chat.send(f.vm.clone(), f.folder.clone(), "Keep running".into())
            .unwrap(),
    );
    for _ in 0..20 {
        chat.status(id);
    }
    ready(&chat, id).await;
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
}

// Rejecting invalid user input must occur before admission, capture or guest writes.
#[tokio::test]
async fn invalid_messages_leave_no_state_or_mutations() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    for message in [" \n".to_string(), "x".repeat(65537)] {
        assert_eq!(
            chat.send(f.vm.clone(), f.folder.clone(), message)
                .unwrap_err(),
            ChatError::InvalidMessage
        );
    }
    assert!(chat.status(id).versions.is_empty());
    assert!(f.vm.try_authoring().is_some());
    assert!(f.guest.lock().unwrap().requests.is_empty());
}

// Test-file edits (including invalid tests) do not alter the saved Agent's context key.
#[tokio::test]
async fn tests_are_ignored_but_saved_source_changes_archive_prior_turns() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "One".into())
        .unwrap();
    ready(&chat, id).await;
    let (path, _) = f.folder.source().unwrap();
    std::fs::write(
        path.join("web-reader.agent.test.md"),
        "not valid test syntax",
    )
    .unwrap();
    chat.send(f.vm.clone(), f.folder.clone(), "Two".into())
        .unwrap();
    ready(&chat, id).await;
    let saved = f.folder.open().unwrap().agent;
    f.folder
        .save(format!("{saved}\nNew saved instructions.\n"))
        .unwrap();
    chat.send(f.vm.clone(), f.folder.clone(), "Three".into())
        .unwrap();
    let s = ready(&chat, id).await;
    assert_eq!(s.versions.len(), 2);
    assert_eq!(s.versions[0].turns.len(), 2);
    assert_eq!(f.guest.lock().unwrap().posts("agents").len(), 2);
}

// An uncertain POST is reconciled by stable-name GET, never a repeated create.
#[tokio::test]
async fn lost_create_replies_get_adopt_without_post_replay() {
    for path in ["agents", "tasks"] {
        let f = Fixture::new().await;
        f.guest.lock().unwrap().lose_create = Some(path);
        let chat = owner();
        let id = f.folder.document_id();
        chat.send(f.vm.clone(), f.folder.clone(), "Adopt my task".into())
            .unwrap();
        ready(&chat, id).await;
        let g = f.guest.lock().unwrap();
        assert_eq!(g.posts(path).len(), 1);
        let name = g.posts(path)[0]["name"].as_str().unwrap().to_string();
        assert!(g
            .requests
            .iter()
            .any(|r| r.method == ChatMethod::Get && r.path == format!("{path}/{name}")));
    }
}

// Agent UID/configuration replacement cannot silently change follow-up execution.
#[tokio::test]
async fn replaced_agent_refuses_continuation_without_new_task() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "One".into())
        .unwrap();
    ready(&chat, id).await;
    {
        let mut g = f.guest.lock().unwrap();
        let key = g
            .objects
            .keys()
            .find(|k| k.starts_with("agents/"))
            .unwrap()
            .clone();
        g.objects.get_mut(&key).unwrap()["metadata"]["uid"] = serde_json::json!("foreign-uid");
    }
    chat.send(f.vm.clone(), f.folder.clone(), "Two".into())
        .unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.state, ChatState::ContextUnavailable);
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
}

// Task completion alone does not prove that the Session lock/transcript has finalized.
#[tokio::test]
async fn terminal_task_waits_for_session_release_and_current_transcript_ids() {
    let f = Fixture::new().await;
    f.guest.lock().unwrap().release = false;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Wait".into())
        .unwrap();
    until(&chat, id, |s| s.state == ChatState::WaitingForSession).await;
    assert!(f.vm.try_authoring().is_none());
    assert!(chat.new_conversation(id).is_err());
    f.guest.lock().unwrap().release = true;
    ready(&chat, id).await;
    assert!(f.vm.try_authoring().is_some());
}

// Stop retains observed evidence, and continuation requires both deletion and release.
#[tokio::test]
async fn acknowledged_stop_is_not_termination_and_notice_survives_follow_up() {
    let f = Fixture::new().await;
    {
        let mut g = f.guest.lock().unwrap();
        g.phase = "Running".into();
        g.release = false;
    }
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Work".into())
        .unwrap();
    until(&chat, id, |s| {
        !s.versions.is_empty() && s.versions[0].turns[0].tools.len() == 2
    })
    .await;
    chat.stop(f.vm.clone(), id).unwrap();
    until(&chat, id, |s| s.state == ChatState::StopRequested).await;
    assert!(!chat.status(id).can_send);
    assert!(f.vm.try_authoring().is_none());
    f.guest.lock().unwrap().release = true;
    let s = ready(&chat, id).await;
    assert!(s.versions[0].turns[0].termination_unconfirmed);
    assert!(!s.versions[0].turns[0].context_retained);
    f.guest.lock().unwrap().phase = "Succeeded".into();
    chat.send(f.vm.clone(), f.folder.clone(), "Continue".into())
        .unwrap();
    let s = ready(&chat, id).await;
    assert!(s.versions[0].turns[0].termination_unconfirmed);
    assert_eq!(s.versions[0].turns[0].tools.len(), 2);
    assert_eq!(f.guest.lock().unwrap().deletes(), 1);
}

// A lost DELETE acknowledgement is reconciled by GET; it is not replayed by reconnect.
#[tokio::test]
async fn lost_stop_reply_waits_for_observed_deletion() {
    let f = Fixture::new().await;
    {
        let mut g = f.guest.lock().unwrap();
        g.phase = "Running".into();
        g.delete_status = 500;
        g.delete_applies = false;
    }
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Work".into())
        .unwrap();
    until(&chat, id, |s| s.state == ChatState::Working).await;
    chat.stop(f.vm.clone(), id).unwrap();
    until(&chat, id, |s| s.state == ChatState::StopUnconfirmed).await;
    assert!(f.vm.try_authoring().is_none());
    {
        let mut g = f.guest.lock().unwrap();
        g.objects.retain(|k, _| !k.starts_with("tasks/"));
    }
    let s = ready(&chat, id).await;
    assert!(s.versions[0].turns[0].termination_unconfirmed);
    assert_eq!(f.guest.lock().unwrap().deletes(), 1);
}

// A missing Session after known Task deletion is unavailable context, not a forever gate.
#[tokio::test]
async fn deleted_task_and_missing_session_allow_only_fresh_conversation() {
    let f = Fixture::new().await;
    {
        let mut g = f.guest.lock().unwrap();
        g.phase = "Running".into();
        g.missing_session = true;
    }
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Work".into())
        .unwrap();
    until(&chat, id, |s| s.state == ChatState::Working).await;
    chat.stop(f.vm.clone(), id).unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.state, ChatState::ContextUnavailable);
    assert!(f.vm.try_authoring().is_some());
    assert!(chat.new_conversation(id).is_ok());
}

// A malformed transcript cannot be claimed as retained context or reused for follow-ups.
#[tokio::test]
async fn malformed_transcript_disables_continuation_but_releases_known_session() {
    let f = Fixture::new().await;
    f.guest.lock().unwrap().transcript = Some("{bad jsonl".into());
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Work".into())
        .unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.state, ChatState::ContextUnavailable);
    assert!(!s.versions[0].turns[0].context_retained);
    assert!(f.vm.try_authoring().is_some());
    assert!(chat.new_conversation(id).is_ok());
}

// Busy is known pre-dispatch and must not turn a safe retry into an uncertain POST.
#[tokio::test]
async fn client_busy_retries_only_the_borrow_and_allows_controls_between_polls() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    let (entered, wait) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let vm = f.vm.clone();
    let blocked = tokio::spawn(async move {
        vm.with_client(move |_, _| {
            entered.send(()).unwrap();
            held.recv().unwrap();
        })
        .await
        .unwrap()
    });
    wait.await.unwrap();
    chat.send(f.vm.clone(), f.folder.clone(), "Wait for client".into())
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(f.guest.lock().unwrap().requests.is_empty());
    assert!(chat.status(id).busy);
    release.send(()).unwrap();
    blocked.await.unwrap();
    ready(&chat, id).await;
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
    assert!(f
        .vm
        .with_client(|_, c| c.model_status())
        .await
        .unwrap()
        .is_ok());
}

// Native admission cannot be bypassed by another document or a repeated send.
#[tokio::test]
async fn shared_authoring_lease_rejects_chat_without_losing_prior_history() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    let lease = f.vm.try_authoring().unwrap();
    assert_eq!(
        chat.send(f.vm.clone(), f.folder.clone(), "Busy".into())
            .unwrap_err(),
        ChatError::Busy
    );
    assert!(chat.status(id).versions.is_empty());
    drop(lease);
    f.guest.lock().unwrap().phase = "Running".into();
    chat.send(f.vm.clone(), f.folder.clone(), "First".into())
        .unwrap();
    assert_eq!(
        chat.send(f.vm.clone(), f.folder.clone(), "Duplicate".into())
            .unwrap_err(),
        ChatError::Busy
    );
    until(&chat, id, |s| s.state == ChatState::Working).await;
    assert!(f.vm.try_authoring().is_none());
    chat.stop(f.vm.clone(), id).unwrap();
    ready(&chat, id).await;
}

// Malformed/unattributable event pages cannot be treated as known progress.
#[tokio::test]
async fn invalid_events_preserve_evidence_and_reconnect_is_read_only() {
    for cursor in [false, true] {
        let f = Fixture::new().await;
        {
            let mut g = f.guest.lock().unwrap();
            g.wrong_events = !cursor;
            g.bad_cursor = cursor;
        }
        let chat = owner();
        let id = f.folder.document_id();
        chat.send(f.vm.clone(), f.folder.clone(), "Observe".into())
            .unwrap();
        let s = until(&chat, id, |s| !s.busy).await;
        assert_eq!(s.state, ChatState::ObservationLost);
        assert!(f.vm.try_authoring().is_none());
        let before = f.guest.lock().unwrap().requests.len();
        {
            let mut g = f.guest.lock().unwrap();
            g.wrong_events = false;
            g.bad_cursor = false;
        }
        chat.reconnect(f.vm.clone(), id).unwrap();
        ready(&chat, id).await;
        let g = f.guest.lock().unwrap();
        assert!(g.requests[before..]
            .iter()
            .all(|r| r.method == ChatMethod::Get));
        assert_eq!(g.posts("tasks").len(), 1);
    }
}

// A new workspace must never receive reads/deletes for an unresolved old-boot Task.
#[tokio::test]
async fn reconnect_to_different_workspace_archives_without_sending_old_names() {
    let old = Fixture::new().await;
    let new = Fixture::new().await;
    {
        let mut g = old.guest.lock().unwrap();
        g.wrong_events = true;
    }
    let chat = owner();
    let id = old.folder.document_id();
    chat.send(old.vm.clone(), old.folder.clone(), "Old workspace".into())
        .unwrap();
    until(&chat, id, |s| !s.busy).await;
    assert!(old.vm.try_authoring().is_none());
    assert!(new.vm.try_authoring().is_some());
    chat.reconnect(new.vm.clone(), id).unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.state, ChatState::ContextUnavailable);
    assert!(old.vm.try_authoring().is_some());
    assert!(new.guest.lock().unwrap().requests.is_empty());
    chat.new_conversation(id).unwrap();
    chat.send(new.vm.clone(), old.folder.clone(), "Fresh workspace".into())
        .unwrap();
    ready(&chat, id).await;
    assert_eq!(new.guest.lock().unwrap().posts("tasks").len(), 1);
}

// A dead same-boot client cannot erase unresolved work, even after reconnect/Stop.
#[tokio::test]
async fn disconnected_same_boot_keeps_gate_and_uncertainty() {
    let f = Fixture::new().await;
    f.guest.lock().unwrap().phase = "Running".into();
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Work".into())
        .unwrap();
    until(&chat, id, |s| s.state == ChatState::Working).await;
    f.guest.lock().unwrap().broken_wire = true;
    until(&chat, id, |s| !s.busy).await;
    chat.reconnect(f.vm.clone(), id).unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.state, ChatState::ObservationLost);
    assert!(chat.new_conversation(id).is_err());
    assert!(f.vm.try_authoring().is_none());
    chat.stop(f.vm.clone(), id).unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.state, ChatState::StopUnconfirmed);
    assert!(f.vm.try_authoring().is_none());
}

// Explicit New conversation keeps old visible turns, but must generate new context.
#[tokio::test]
async fn explicit_new_conversation_and_provider_change_split_versions() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "One".into())
        .unwrap();
    ready(&chat, id).await;
    chat.new_conversation(id).unwrap();
    chat.send(f.vm.clone(), f.folder.clone(), "Two".into())
        .unwrap();
    ready(&chat, id).await;
    f.guest.lock().unwrap().provider = "other-provider".into();
    chat.send(f.vm.clone(), f.folder.clone(), "Three".into())
        .unwrap();
    let s = ready(&chat, id).await;
    assert_eq!(s.versions.len(), 3);
    let g = f.guest.lock().unwrap();
    assert_eq!(g.posts("agents").len(), 3);
    assert_eq!(
        g.posts("agents")[2]["spec"]["providerRef"]["name"],
        "other-provider"
    );
}

// Failed execution has no guaranteed result, but must retain its phase and confirmed context.
#[tokio::test]
async fn failed_task_without_result_can_finalize_from_real_session_evidence() {
    let f = Fixture::new().await;
    {
        let mut g = f.guest.lock().unwrap();
        g.phase = "Failed".into();
        g.no_result = true;
    }
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Fail safely".into())
        .unwrap();
    let s = ready(&chat, id).await;
    assert_eq!(s.versions[0].turns[0].phase, "Failed");
    assert!(s.versions[0].turns[0].context_retained);
}

// A reconnect/Stop needs the native workspace identity, not a still-connected model route.
#[tokio::test]
async fn stop_after_model_route_disconnect_still_deletes_only_known_task() {
    let f = Fixture::new().await;
    {
        let mut g = f.guest.lock().unwrap();
        g.wrong_events = true;
    }
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Work".into())
        .unwrap();
    until(&chat, id, |s| !s.busy).await;
    f.guest.lock().unwrap().model_ready = false;
    chat.stop(f.vm.clone(), id).unwrap();
    ready(&chat, id).await;
    assert_eq!(f.guest.lock().unwrap().deletes(), 1);
}

// A completed old workspace automatically starts a new version on the next send.
#[tokio::test]
async fn completed_context_moves_to_new_workspace_without_old_resource_names() {
    let old = Fixture::new().await;
    let new = Fixture::new().await;
    let chat = owner();
    let id = old.folder.document_id();
    chat.send(old.vm.clone(), old.folder.clone(), "One".into())
        .unwrap();
    ready(&chat, id).await;
    chat.send(new.vm.clone(), old.folder.clone(), "Two".into())
        .unwrap();
    let s = ready(&chat, id).await;
    assert_eq!(s.versions.len(), 2);
    let g = new.guest.lock().unwrap();
    assert_eq!(g.posts("agents").len(), 1);
    assert_eq!(g.posts("tasks").len(), 1);
}

// An admitted-but-not-yet-polled worker must not use a restarted workspace's new gate.
#[test]
fn workspace_epoch_change_before_first_dispatch_refuses_without_guest_mutations() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let f = runtime.block_on(Fixture::new());
    let worker_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let chat = ChatOwner::new(worker_runtime.handle().clone());
    let id = f.folder.document_id();
    chat.send(
        f.vm.clone(),
        f.folder.clone(),
        "Queued before restart".into(),
    )
    .unwrap();
    runtime.block_on(async {
        f.vm.cancel().await.unwrap();
        f.guest.lock().unwrap().boot = bridge_runtime_protocol::Uuid::new_v4();
        f.vm.start();
        for _ in 0..500 {
            if f.vm.status() == bridge_engine::local_vm::LocalVmStatus::Ready {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(f.vm.status(), bridge_engine::local_vm::LocalVmStatus::Ready);
    });
    let s = worker_runtime.block_on(until(&chat, id, |s| !s.busy));
    assert_eq!(s.state, ChatState::ContextUnavailable);
    assert_eq!(s.error, Some(ChatError::WorkspaceChanged));
    assert!(f.guest.lock().unwrap().requests.is_empty());
    assert!(f.vm.try_authoring().is_some());
}

// A guest rejecting the Chat wire action must never trigger a Run or host fallback.
#[tokio::test]
async fn old_guest_rejects_without_authority_fallback() {
    let f = Fixture::new().await;
    f.guest.lock().unwrap().old_guest = true;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Unsupported".into())
        .unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert!(matches!(
        s.state,
        ChatState::ObservationLost | ChatState::ContextUnavailable
    ));
    assert!(f.guest.lock().unwrap().posts("tasks").is_empty());
    assert!(
        f.vm.try_authoring().is_some(),
        "Agent-only failure is not active execution"
    );
    assert!(chat.new_conversation(id).is_ok());
}

// Task status is absent until the controller first reconciles the created object.
#[tokio::test]
async fn not_yet_reconciled_task_keeps_polling_without_manual_reconnect() {
    let f = Fixture::new().await;
    f.guest.lock().unwrap().omit_phase = true;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "Wait for controller".into())
        .unwrap();
    let waiting = until(&chat, id, |s| {
        !s.busy
            || s.versions
                .last()
                .is_some_and(|v| v.turns.last().is_some_and(|t| !t.tools.is_empty()))
    })
    .await;
    assert!(
        waiting.busy,
        "uninitialized status is not a failed observation: {waiting:?}"
    );
    assert!(waiting.error.is_none());
    f.guest.lock().unwrap().omit_phase = false;
    ready(&chat, id).await;
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
}

// A locally oversized encoded message must not force discarding an established Session.
#[tokio::test]
async fn rejected_preflight_keeps_existing_conversation_available_for_retry() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "First".into())
        .unwrap();
    ready(&chat, id).await;
    chat.send(f.vm.clone(), f.folder.clone(), "x".repeat(65500))
        .unwrap();
    let rejected = until(&chat, id, |s| !s.busy).await;
    assert_eq!(rejected.error, Some(ChatError::InvalidMessage));
    assert!(
        rejected.can_send,
        "local rejection must retain context: {rejected:?}"
    );
    chat.send(f.vm.clone(), f.folder.clone(), "Retry".into())
        .unwrap();
    let retried = ready(&chat, id).await;
    assert_eq!(retried.versions.len(), 1);
    assert_eq!(retried.versions[0].turns.len(), 2);
    let g = f.guest.lock().unwrap();
    let tasks = g.posts("tasks");
    assert_eq!(tasks.len(), 2);
    assert_eq!(
        tasks[0]["spec"]["sessionRef"]["name"],
        tasks[1]["spec"]["sessionRef"]["name"]
    );
}

#[tokio::test]
async fn deleted_session_refuses_followup_without_recreating_empty_context() {
    let f = Fixture::new().await;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "First".into())
        .unwrap();
    ready(&chat, id).await;
    f.guest.lock().unwrap().missing_session = true;
    chat.send(f.vm.clone(), f.folder.clone(), "Followup".into())
        .unwrap();
    let s = until(&chat, id, |s| !s.busy).await;
    assert_eq!(s.error, Some(ChatError::ContextUnavailable));
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
    assert!(chat.new_conversation(id).is_ok());
}

#[tokio::test]
async fn missing_uncertain_task_keeps_admission_until_session_is_released() {
    let f = Fixture::new().await;
    {
        let mut g = f.guest.lock().unwrap();
        g.lose_create = Some("tasks");
        g.vanish_task = true;
        g.release = false;
    }
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(
        f.vm.clone(),
        f.folder.clone(),
        "Lost acknowledgement".into(),
    )
    .unwrap();
    let waiting = until(&chat, id, |s| {
        !s.busy || f.guest.lock().unwrap().session_reads > 0
    })
    .await;
    assert!(
        waiting.busy,
        "Task absence is not Session release: {waiting:?}"
    );
    assert!(f.vm.try_authoring().is_none());
    f.guest.lock().unwrap().release = true;
    let ended = until(&chat, id, |s| !s.busy).await;
    assert_eq!(ended.state, ChatState::ContextUnavailable);
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 1);
    assert!(f.vm.try_authoring().is_some());
}

#[test]
fn stop_before_preflight_dispatch_preserves_existing_context() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let f = runtime.block_on(Fixture::new());
    let chat = ChatOwner::new(runtime.handle().clone());
    let id = f.folder.document_id();
    chat.send(f.vm.clone(), f.folder.clone(), "First".into())
        .unwrap();
    runtime.block_on(ready(&chat, id));
    chat.send(f.vm.clone(), f.folder.clone(), "Never dispatched".into())
        .unwrap();
    chat.stop(f.vm.clone(), id).unwrap();
    let stopped = runtime.block_on(until(&chat, id, |s| !s.busy));
    assert_eq!(stopped.state, ChatState::Ready);
    assert_eq!(stopped.versions[0].turns.len(), 1);
    chat.send(f.vm.clone(), f.folder.clone(), "Retry".into())
        .unwrap();
    runtime.block_on(ready(&chat, id));
    assert_eq!(f.guest.lock().unwrap().posts("tasks").len(), 2);
}

#[tokio::test]
async fn valid_events_continue_beyond_sixteen_pages_without_reconnect() {
    let f = Fixture::new().await;
    f.guest.lock().unwrap().event_total = 18;
    let chat = owner();
    let id = f.folder.document_id();
    chat.send(
        f.vm.clone(),
        f.folder.clone(),
        "Read all observed activity".into(),
    )
    .unwrap();
    let s = ready(&chat, id).await;
    assert_eq!(s.versions[0].turns[0].tools.len(), 18);
    assert!(f
        .guest
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|r| r.after == Some(17)));
}
