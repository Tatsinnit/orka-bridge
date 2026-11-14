//! Native Send owner fixtures. A child of the forward fixture so it can count actual wire calls.
use super::*;
use bridge_engine::{
    agent_folder::AgentFolder,
    clusters::commands::ClusterSession,
    send::{SendOwner, SendState},
};
use bridge_runtime_protocol::SnapshotId;

#[path = "created_agent.rs"]
mod created_agent;

#[tokio::test]
async fn document_send_binds_preview_admission_stop_and_session_results() {
    let s = setup().await;
    let agent = AGENT.replace("Fetch only.", "Bound alpha instructions.");
    let expected = SnapshotId::of(agent.as_bytes(), TESTS.as_bytes());
    let a = created_agent::created(s.home.path(), "alpha", &agent, TESTS);
    let b = created_agent::created(s.home.path(), "beta", &agent, TESTS);
    let aid = a.document_id();
    let bid = b.document_id();
    let check = s
        .owner
        .preflight_document(b.clone(), &s.target, &s.provider)
        .await;
    assert_eq!(check.document_id, bid);
    assert_eq!(check.source, Some(expected.clone()));
    *s.api.wire.mode.lock().unwrap() = "hang";
    let started = s
        .owner
        .start_document(a, &s.target, &s.provider, expected.clone())
        .await;
    let first = serde_json::to_value(started.view.unwrap()).unwrap();
    until(|| s.api.wire.posts() == 1).await;
    let foreign = s
        .owner
        .start_document(b.clone(), &s.target, &s.provider, expected.clone())
        .await;
    assert!(foreign.view.is_none());
    assert_eq!(foreign.document_id, bid);
    assert_eq!(foreign.busy.unwrap().document_id, aid);
    assert!(s.owner.stop_document(bid, &id(&first)).await.is_err());
    assert_eq!(s.api.wire.posts(), 1);
    s.owner.stop_document(aid, &id(&first)).await.unwrap();
    assert!(s.owner.status_document(bid).view.is_none());
    assert_eq!(s.owner.status_document(aid).view.unwrap().document_id, aid);
    *s.api.wire.mode.lock().unwrap() = "ok";
    let started = s
        .owner
        .start_document(b, &s.target, &s.provider, expected)
        .await;
    let second = serde_json::to_value(started.view.unwrap()).unwrap();
    let done = s.finish(&id(&second)).await;
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["documentId"], serde_json::to_value(bid).unwrap());
    assert_eq!(
        s.agent_posts().last().unwrap()["spec"]["systemPrompt"]["inline"],
        "Bound alpha instructions.\n"
    );
    assert_eq!(
        s.owner.status_document(aid).view.unwrap().state,
        SendState::Unknown
    );
    let fresh = SendOwner::with_history_root(
        Arc::new(AgentFolder::new(Some(s.home.path().into()))),
        s.session.clone(),
        Some(s.home.path().join("app-local")),
    );
    assert!(fresh.status_document(bid).view.is_none());
    // Live document ownership remains session-only, while metadata-only history
    // retains both attempts, including the selected non-sample folder's hashes.
    let saved = fresh.history().unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0].id, id(&second));
    assert_eq!(
        saved[0].source,
        SnapshotId::of(agent.as_bytes(), TESTS.as_bytes())
    );
    assert_eq!(
        saved[0].verdict,
        bridge_engine::send_summary::AttemptVerdict::Passed
    );
    assert!(saved[0].agent_uid.is_some());
    assert_eq!(saved[1].id, id(&first));
    assert_eq!(
        saved[1].verdict,
        bridge_engine::send_summary::AttemptVerdict::Unknown
    );
    assert!(saved[1].agent_uid.is_none());
    assert_ne!(saved[0].planned_agent_name, saved[1].planned_agent_name);
    s.owner.shutdown().await;
}

#[tokio::test]
async fn history_recovery_is_get_only_offline_preserving_and_serialized() {
    let s = setup().await;
    let started = s.start().await;
    let send = id(&started);
    let done = s.finish(&send).await;
    assert_eq!(done["state"], "complete", "{done}");
    let saved = s.owner.history_details(&send).unwrap();
    assert_eq!(saved.detail_state, "saved");
    assert!(!saved.interrupted);
    assert!(saved.can_remove && saved.can_check);
    let before = s.api.wire.posts();
    // Historical reads are independent of today's edited source bytes.
    std::fs::write(
        s.agent_path().with_file_name("web-reader.agent.test.md"),
        TESTS.replace("expected answer", "changed expectation"),
    )
    .unwrap();
    let checked = s.owner.history_check(&send).await.unwrap();
    assert_eq!(checked.availability.as_deref(), Some("available"));
    assert_eq!(
        checked.report.unwrap().verdict,
        bridge_engine::engine::run::Verdict::Passed
    );
    assert_eq!(s.api.wire.posts(), before);
    s.idle_forwards().await;
    // The current selection/configuration cannot silently confer authority.
    s.fresh.lock().unwrap().clusters[0]
        .cluster
        .as_mut()
        .unwrap()
        .server = Some("https://different.invalid".into());
    let checked = s.owner.history_check(&send).await.unwrap();
    assert_eq!(checked.availability.as_deref(), Some("unavailable"));
    assert!(checked.report.unwrap().cases[0].answer.is_some());
    s.idle_forwards().await;
    let removed = s.owner.history_remove(&send).await.unwrap();
    assert_eq!(removed.detail_state, "removed");
    assert!(removed.report.is_none());
    assert_eq!(s.api.wire.posts(), before);
    assert_eq!(s.owner.history().unwrap().len(), 1);
}

/// Wait for the real reader thread without letting paused Tokio time auto-advance.
async fn reader_started(entered: &mut tokio::sync::oneshot::Receiver<()>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match entered.try_recv() {
            Ok(()) => return,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                assert!(std::time::Instant::now() < deadline, "reader did not start");
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("reader dropped its entry signal: {error}"),
        }
    }
}

fn block_next_read(
    s: &Setup,
    release: std::sync::mpsc::Receiver<()>,
) -> tokio::sync::oneshot::Receiver<()> {
    let (entered, started) = tokio::sync::oneshot::channel();
    *s.on_read.lock().unwrap() = Some(Box::new(move || {
        let _ = entered.send(());
        // Dropping the sender also releases this read, including on test failure.
        let _ = release.recv();
    }));
    started
}

#[tokio::test]
async fn history_blocked_reader_times_out_releases_gate_and_keeps_shared_limit() {
    let s = setup().await;
    let started = s.start().await;
    let send = id(&started);
    assert_eq!(s.finish(&send).await["state"], "complete");
    let posts = s.api.wire.posts();
    let (release, blocked) = std::sync::mpsc::channel();
    let mut entered = block_next_read(&s, blocked);
    tokio::time::pause();
    let check = tokio::spawn({
        let owner = s.owner.clone();
        let send = send.clone();
        async move { owner.history_check(&send).await }
    });
    reader_started(&mut entered).await;
    tokio::time::advance(Duration::from_secs(10)).await;
    // Real time also bounds RED: Tokio does not auto-advance past blocking-pool work.
    tokio::time::resume();
    let checked = tokio::time::timeout(Duration::from_secs(1), check)
        .await
        .expect("history must release its gate after the configuration read budget")
        .unwrap()
        .unwrap();
    assert_eq!(checked.availability.as_deref(), Some("unavailable"));
    assert_eq!(checked.report.unwrap().verdict, Verdict::Passed);

    let extra_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = extra_reads.clone();
    *s.on_read.lock().unwrap() = Some(Box::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    let checked = tokio::time::timeout(Duration::from_secs(1), s.owner.history_check(&send))
        .await
        .expect("another history check must not wait on the stuck read")
        .unwrap();
    assert_eq!(checked.availability.as_deref(), Some("unavailable"));
    let preview = tokio::time::timeout(
        Duration::from_secs(1),
        s.owner.preflight(&s.target, &s.provider),
    )
    .await
    .expect("preflight must share the outstanding read limit");
    assert!(!preview.can_send);
    let next = tokio::time::timeout(Duration::from_secs(1), s.start())
        .await
        .expect("history must not hold Send admission");
    let done = s.finish(&id(&next)).await;
    assert_eq!(done["error"], "access", "{done}");
    let removed = tokio::time::timeout(Duration::from_secs(1), s.owner.history_remove(&send))
        .await
        .expect("history must not hold local removal")
        .unwrap();
    assert_eq!(removed.detail_state, "removed");
    tokio::time::timeout(Duration::from_secs(1), s.owner.shutdown())
        .await
        .expect("shutdown must not wait on a stalled filesystem reader");
    assert!(
        tokio::time::timeout(Duration::from_secs(1), s.session.reload())
            .await
            .expect("reload must share the outstanding read limit")
            .is_err()
    );
    assert_eq!(extra_reads.load(Ordering::SeqCst), 0);
    assert_eq!(s.api.wire.posts(), posts);
    drop(release);
    // The limit opens only when the old read exits; its late result is discarded.
    tokio::time::timeout(Duration::from_secs(1), async {
        while s.session.reload().await.is_err() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a completed reader must release the shared limit");
    assert_eq!(extra_reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        s.owner.history_details(&send).unwrap().detail_state,
        "removed"
    );
}

#[test]
fn history_blocked_live_reader_stop_does_not_hold_runtime_shutdown() {
    let (release, blocked) = std::sync::mpsc::channel();
    let (closed, closing) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let s = setup().await;
            let mut entered = block_next_read(&s, blocked);
            let started = s.start().await;
            reader_started(&mut entered).await;
            let stopped = tokio::time::timeout(Duration::from_secs(1), s.owner.stop(&id(&started)))
                .await
                .expect("Stop must not wait for the filesystem read");
            assert_eq!(serde_json::to_value(stopped).unwrap()["stage"], "stopped");
            assert_eq!(s.api.wire.posts(), 0);
            s.owner.shutdown().await;
            s.session.shutdown().await;
        });
        drop(runtime);
        closed.send(()).unwrap();
    });
    let result = closing.recv_timeout(Duration::from_secs(5));
    // Always release the read before asserting/joining, even on RED or a worker panic.
    drop(release);
    let joined = worker.join();
    assert!(
        result.is_ok(),
        "a stopped reader held Tokio runtime shutdown"
    );
    joined.unwrap();
}

#[tokio::test]
async fn history_retains_cached_text_without_verifying_a_null_result() {
    let s = setup().await;
    let started = s.start().await;
    let send = id(&started);
    assert_eq!(s.finish(&send).await["report"]["verdict"], "passed");
    let posts = s.api.wire.posts();
    *s.api.wire.mode.lock().unwrap() = "empty-result";
    let checked = s.owner.history_check(&send).await.unwrap();
    let report = checked.report.unwrap();
    assert_eq!(report.cases[0].answer.as_deref(), Some("expected answer"));
    assert_eq!(
        report.verdict,
        Verdict::Unknown,
        "cached text is not a freshly verified answer"
    );
    assert_eq!(
        s.owner.history().unwrap()[0].verdict,
        bridge_engine::send_summary::AttemptVerdict::Unknown
    );
    assert_eq!(s.api.wire.posts(), posts);
}

#[tokio::test]
async fn history_tool_only_checks_use_fresh_events_when_result_is_missing() {
    let s = setup().await;
    std::fs::write(
        s.agent_path().with_file_name("web-reader.agent.test.md"),
        TESTS.replace(
            "  - id: answer\n    type: contains\n    value: expected answer\n",
            "",
        ),
    )
    .unwrap();
    let started = s.start().await;
    let send = id(&started);
    assert_eq!(s.finish(&send).await["report"]["verdict"], "passed");
    let posts = s.api.wire.posts();
    *s.api.wire.mode.lock().unwrap() = "result-404";
    let report = s.owner.history_check(&send).await.unwrap().report.unwrap();
    assert_eq!(report.cases[0].completed_tools, ["web_fetch"]);
    assert_eq!(report.cases[0].verdict, Verdict::Passed);
    assert_eq!(
        report.verdict,
        Verdict::Passed,
        "tool-only checks do not require answer text"
    );
    *s.api.wire.mode.lock().unwrap() = "events-404";
    assert_eq!(
        s.owner
            .history_check(&send)
            .await
            .unwrap()
            .report
            .unwrap()
            .verdict,
        Verdict::Unknown,
        "cached tool names cannot become fresh proof"
    );
    assert_eq!(s.api.wire.posts(), posts);
}

#[tokio::test]
async fn history_keeps_a_fresh_assertion_failure_when_optional_events_are_missing() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "wrong-result";
    let started = s.start().await;
    let send = id(&started);
    assert_eq!(s.finish(&send).await["report"]["verdict"], "failed");
    let posts = s.api.wire.posts();
    *s.api.wire.mode.lock().unwrap() = "wrong-result-events-missing";
    let checked = s.owner.history_check(&send).await.unwrap();
    assert_eq!(checked.availability.as_deref(), Some("missing"));
    let report = checked.report.unwrap();
    assert_eq!(report.cases[0].verdict, Verdict::Failed);
    assert_eq!(
        report.verdict,
        Verdict::Failed,
        "fresh failing answer evidence is decisive"
    );
    assert_eq!(
        s.owner.history().unwrap()[0].verdict,
        bridge_engine::send_summary::AttemptVerdict::Failed
    );
    *s.api.wire.mode.lock().unwrap() = "result-403";
    assert_eq!(
        s.owner
            .history_check(&send)
            .await
            .unwrap()
            .report
            .unwrap()
            .verdict,
        Verdict::Unknown,
        "cached-only failure must not become freshly verified"
    );
    assert_eq!(s.api.wire.posts(), posts);
}

#[tokio::test]
async fn history_records_terminal_phase_and_tools_without_answer_text() {
    for (mode, tool_only) in [("failed-empty", false), ("empty-result", true)] {
        let s = setup().await;
        if tool_only {
            std::fs::write(
                s.agent_path().with_file_name("web-reader.agent.test.md"),
                TESTS.replace(
                    "  - id: answer\n    type: contains\n    value: expected answer\n",
                    "",
                ),
            )
            .unwrap();
        }
        *s.api.wire.mode.lock().unwrap() = "running";
        let started = s.start().await;
        until(|| s.view(None)["stage"] == "observe").await;
        s.owner.stop(&id(&started)).await;
        let root = s.home.path().join("app-local");
        let send = id(&started);
        let mut detail: bridge_engine::send_summary::AttemptDetails = serde_json::from_slice(
            &std::fs::read(root.join(format!("send-history-v1/{send}.json"))).unwrap(),
        )
        .unwrap();
        detail.captured_at = 1;
        bridge_engine::send_summary::SummaryStore::new(Some(root))
            .checkpoint(s.owner.history().unwrap()[0].clone(), detail)
            .unwrap();
        *s.api.wire.mode.lock().unwrap() = mode;
        let view = s.owner.history_check(&send).await.unwrap();
        assert!(
            view.report.is_some(),
            "{mode}: availability {:?}, calls {:?}",
            view.availability,
            s.api
                .wire
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|(p, _)| p)
                .collect::<Vec<_>>()
        );
        let report = view.report.unwrap();
        assert_eq!(
            report.cases[0].phase,
            if tool_only { "Succeeded" } else { "Failed" }
        );
        assert!(report.cases[0].answer.is_none());
        assert_eq!(report.cases[0].completed_tools, ["web_fetch"]);
        assert_eq!(
            report.cases[0].verdict,
            if tool_only {
                Verdict::Passed
            } else {
                Verdict::Failed
            }
        );
        assert_eq!(
            report.verdict,
            if tool_only {
                Verdict::Passed
            } else {
                Verdict::Failed
            }
        );
        assert!(
            view.captured_at.unwrap() > 1,
            "new phase and tool evidence needs its own capture time"
        );
        assert!(!view.interrupted);
    }
}

#[tokio::test]
async fn history_preserves_capture_time_when_only_cached_answers_remain() {
    use bridge_engine::send_summary::{AttemptDetails, SummaryStore};
    let s = setup().await;
    let started = s.start().await;
    let send = id(&started);
    s.finish(&send).await;
    let root = s.home.path().join("app-local");
    let mut detail: AttemptDetails = serde_json::from_slice(
        &std::fs::read(root.join(format!("send-history-v1/{send}.json"))).unwrap(),
    )
    .unwrap();
    detail.captured_at = 1;
    SummaryStore::new(Some(root))
        .checkpoint(s.owner.history().unwrap()[0].clone(), detail)
        .unwrap();
    *s.api.wire.mode.lock().unwrap() = "result-404";
    let checked = s.owner.history_check(&send).await.unwrap();
    assert_eq!(checked.captured_at, Some(1));
    assert_eq!(
        checked.report.unwrap().cases[0].answer.as_deref(),
        Some("expected answer")
    );
}

#[tokio::test]
async fn history_shutdown_waits_for_read_cleanup_and_rejects_later_checks() {
    let s = setup().await;
    let started = s.start().await;
    let send = id(&started);
    s.finish(&send).await;
    let before = s.api.wire.calls.lock().unwrap().len();
    *s.api.wire.mode.lock().unwrap() = "hold-history";
    let owner = s.owner.clone();
    let check_id = send.clone();
    let check = tokio::spawn(async move { owner.history_check(&check_id).await });
    until(|| s.api.wire.calls.lock().unwrap().len() > before).await;
    let mut closing = Box::pin(s.owner.shutdown());
    let pending =
        std::future::poll_fn(|cx| std::task::Poll::Ready(closing.as_mut().poll(cx).is_pending()))
            .await;
    *s.api.wire.mode.lock().unwrap() = "";
    check.await.unwrap().unwrap();
    if pending {
        closing.await;
    }
    assert!(pending, "shutdown must await the historical read owner");
    s.idle_forwards().await;
    let calls = s.api.wire.calls.lock().unwrap().len();
    assert!(s.owner.history_check(&send).await.is_err());
    assert_eq!(s.api.wire.calls.lock().unwrap().len(), calls);
}

#[tokio::test]
async fn storage_failure_after_agent_ack_keeps_remote_uncertainty_and_bars_tasks() {
    let s = setup().await;
    let root = s.home.path().join("app-local/send-history-v1");
    *s.api.wire.after_post.lock().unwrap() = Some(Box::new(move || {
        std::fs::create_dir(root.join("saving")).unwrap();
    }));
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["state"], "unknown");
    assert_eq!(
        view["error"], "submit",
        "remote creation must not be labelled nothing sent: {view}"
    );
    assert_eq!(view["storageError"], true);
    assert_eq!(view["acknowledged"].as_array().unwrap().len(), 1);
    assert_eq!(
        s.api.wire.posts(),
        1,
        "no Task may follow a failed durable acknowledgement"
    );
}

#[tokio::test]
async fn omitted_answer_cache_does_not_stop_observing_the_remaining_tests() {
    let s = setup().await;
    let block = &TESTS[TESTS.find("```yaml agent-test").unwrap()..];
    let cases: String = (0..6)
        .map(|i| block.replace("id: fetch\n", &format!("id: fetch-{i}\n")))
        .collect();
    std::fs::write(
        s.agent_path().with_file_name("web-reader.agent.test.md"),
        format!("---\nschemaVersion: 1\n---\n{cases}"),
    )
    .unwrap();
    *s.api.wire.mode.lock().unwrap() = "large-result";
    let started = s.start().await;
    let send = id(&started);
    let view = s.finish(&send).await;
    assert_eq!(
        view["state"], "complete",
        "reduced cache is not a storage failure: {view}"
    );
    assert_eq!(view["report"]["cases"].as_array().unwrap().len(), 6);
    assert_eq!(view["report"]["verdict"], "passed");
    assert_eq!(view["storageError"], false);
    assert_eq!(s.api.wire.posts(), 7);
    let saved = s.owner.history_details(&send).unwrap();
    assert_eq!(saved.detail_state, "omitted");
    assert!(!saved.interrupted);
    let cached = saved.report.unwrap();
    assert_eq!(cached.cases.len(), 6);
    assert!(cached
        .cases
        .iter()
        .all(|c| c.answer.is_none() && c.phase == "Succeeded"));
}

#[tokio::test]
async fn history_preserves_answers_on_404_denial_and_agent_revision_change() {
    let s = setup().await;
    let started = s.start().await;
    let send = id(&started);
    s.finish(&send).await;
    let before = s.api.wire.posts();
    for (mode, availability) in [
        ("agent-replaced", "missing"),
        ("agent-generation", "missing"),
        ("result-404", "missing"),
        ("task-404", "missing"),
        ("result-403", "unavailable"),
        ("events-404", "missing"),
    ] {
        *s.api.wire.mode.lock().unwrap() = mode;
        let view = s.owner.history_check(&send).await.unwrap();
        assert_eq!(view.availability.as_deref(), Some(availability), "{mode}");
        let report = view.report.unwrap();
        assert_eq!(
            report.cases[0].answer.as_deref(),
            Some("expected answer"),
            "{mode}"
        );
        assert_ne!(report.verdict, Verdict::Passed, "{mode}");
        s.idle_forwards().await;
    }
    assert_eq!(s.api.wire.posts(), before);
}

#[path = "cluster_preflight.rs"]
mod preflight;

const AGENT: &str = "---\nname: reader\ndescription: inert\nmodel: 'model:exact'\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    - kind: builtin\n      name: web_fetch\n---\nFetch only.\n";
const TESTS: &str = "---\nschemaVersion: 1\n---\n```yaml agent-test\nid: fetch\ninput: go\nassertions:\n  - id: answer\n    type: contains\n    value: expected answer\n  - id: fetched\n    type: toolCalled\n    tool: web_fetch\n```\n";

type Hook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;
struct Setup {
    api: Arc<ApiFixture>,
    session: ClusterSession,
    owner: Arc<SendOwner>,
    home: tempfile::TempDir,
    fresh: Arc<Mutex<Kubeconfig>>,
    on_read: Hook,
    target: String,
    provider: String,
}
impl Setup {
    fn agent_path(&self) -> std::path::PathBuf {
        self.home
            .path()
            .join("Orka/agents/web-reader/web-reader.agent.md")
    }
    fn view(&self, id: Option<&str>) -> Value {
        serde_json::to_value(self.owner.status(id)).unwrap()
    }
    fn source(&self) -> SnapshotId {
        SnapshotId::of(
            &std::fs::read(self.agent_path()).unwrap(),
            &std::fs::read(self.agent_path().with_file_name("web-reader.agent.test.md")).unwrap(),
        )
    }
    async fn start(&self) -> Value {
        serde_json::to_value(
            self.owner
                .start(&self.target, &self.provider, self.source())
                .await,
        )
        .unwrap()
    }
    async fn finish(&self, id: &str) -> Value {
        until(|| self.view(Some(id))["state"] != "running").await;
        self.view(Some(id))
    }
    async fn idle_forwards(&self) {
        until(|| self.api.wire.active.load(Ordering::SeqCst) == 0).await;
    }
    fn agent_posts(&self) -> Vec<Value> {
        self.api
            .wire
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0.starts_with("POST /api/v1/agents"))
            .map(|c| c.1.clone())
            .collect()
    }
}
async fn until(done: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition was not reached");
}
fn id(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}
async fn setup_with(agent: &str) -> Setup {
    setup_with_auth(agent, json!({"token":"creator-fixture"})).await
}
async fn setup_with_auth(agent: &str, auth: Value) -> Setup {
    setup_with_auth_and_history(agent, auth, None).await
}
async fn setup_with_auth_and_history(
    agent: &str,
    auth: Value,
    history_root: Option<std::path::PathBuf>,
) -> Setup {
    let api = Arc::new(ApiFixture::default());
    let cfg = config(&api.start(), auth);
    let fresh = Arc::new(Mutex::new(cfg.clone()));
    let on_read: Hook = Arc::default();
    let reader = {
        let (fresh, on_read) = (fresh.clone(), on_read.clone());
        Arc::new(move || {
            // Release the hook slot first; a hook may read configuration again.
            let hook = on_read.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
            Ok(fresh.lock().unwrap().clone())
        })
    };
    let session =
        ClusterSession::with_reader(ClusterClient::new(cfg, Arc::new(PodOrkaRead)), reader);
    let contexts = serde_json::to_value(session.contexts().await.unwrap()).unwrap();
    let found = serde_json::to_value(session.discover(&id(&contexts[0])).await.unwrap()).unwrap();
    let target = id(&found[0]);
    let access = serde_json::to_value(session.use_target(&target, None).await).unwrap();
    assert_eq!(access["status"], "connected");
    let provider = id(&access["providers"][0]);
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    folder.save(agent.into()).unwrap();
    std::fs::write(
        home.path()
            .join("Orka/agents/web-reader/web-reader.agent.test.md"),
        TESTS,
    )
    .unwrap();
    let root = history_root.unwrap_or_else(|| home.path().join("app-local"));
    let owner = SendOwner::with_history_root(folder, session.clone(), Some(root));
    Setup {
        owner,
        session,
        api,
        home,
        fresh,
        on_read,
        target,
        provider,
    }
}
async fn setup() -> Setup {
    setup_with(AGENT).await
}

#[tokio::test]
async fn exec_send_keeps_frozen_identity_between_writes() {
    let dir = tempfile::tempdir().unwrap();
    let count = dir.path().join("calls");
    let version = "client.authentication.k8s.io/v1";
    let mut response = crate::credential_fixture::response(version);
    response["status"]["token"] = json!("creator-fixture");
    let mut auth = crate::credential_fixture::exec(version, response);
    auth["exec"]["env"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"RECORD_FILE","value":count}));
    let s = setup_with_auth(AGENT, auth).await;
    assert_eq!(
        std::fs::read(&count).unwrap().len(),
        2,
        "one resolution per discovery and Use"
    );
    let started = s.start().await;
    let done = s.finish(&id(&started)).await;
    assert_eq!(done["state"], "complete");
    assert_eq!(done["report"]["verdict"], "passed");
    assert_eq!(s.agent_posts().len(), 1);
    assert_eq!(
        std::fs::read(&count).unwrap().len(),
        2,
        "admission and writes must not rerun the plugin"
    );
    s.idle_forwards().await;
}

#[tokio::test]
async fn send_click_runs_captured_file() {
    let s = setup().await;
    assert!(s.owner.status(None).is_none());
    let started = s.start().await;
    assert_eq!(started["state"], "running");
    assert_eq!(started["targetId"], s.target);
    assert_eq!(started["targetName"], Value::Null);
    assert_eq!(started["providerId"], s.provider);
    let view = s.finish(&id(&started)).await;
    assert_eq!(view["state"], "complete", "{view}");
    assert_eq!(view["targetName"], "selected");
    assert_eq!(view["stage"], "done");
    assert_eq!(view["error"], Value::Null);
    assert_eq!(view["report"]["verdict"], "passed");
    assert_eq!(view["model"], "model:exact");
    assert_eq!(
        view["source"],
        serde_json::to_value(SnapshotId::of(AGENT.as_bytes(), TESTS.as_bytes())).unwrap()
    );
    let receipts = view["acknowledged"].as_array().unwrap();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0]["kind"], "Agent");
    assert_eq!(receipts[0]["name"], view["agentName"]);
    assert_eq!(view["pending"], Value::Null);
    assert_eq!(view["storageError"], false);
    assert_eq!(s.api.wire.posts(), 2);
    assert_eq!(
        s.agent_posts()[0]["spec"]["systemPrompt"]["inline"],
        "Fetch only.\n"
    );
    assert_eq!(s.view(None), view);
    let text = view.to_string();
    for secret in ["creator-fixture", "http://", "Bearer"] {
        assert!(!text.contains(secret), "credential marker reached the view");
    }
    s.idle_forwards().await;
}

#[tokio::test]
async fn edit_after_capture_does_not_change_bytes() {
    let s = setup().await;
    let path = s.agent_path();
    // The fresh kubeconfig is read after capture, before any network wait or write.
    *s.on_read.lock().unwrap() = Some(Box::new(move || {
        std::fs::write(path, AGENT.replace("Fetch only.", "Edited later.")).unwrap();
    }));
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["state"], "complete", "{view}");
    assert!(std::fs::read_to_string(s.agent_path())
        .unwrap()
        .contains("Edited later."));
    assert_eq!(
        s.agent_posts()[0]["spec"]["systemPrompt"]["inline"],
        "Fetch only.\n"
    );
    assert_eq!(
        view["source"],
        serde_json::to_value(SnapshotId::of(AGENT.as_bytes(), TESTS.as_bytes())).unwrap()
    );
}

#[tokio::test]
async fn duplicate_active_start_observes() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "hang";
    let first = s.start().await;
    until(|| s.api.wire.posts() == 1).await;
    let forwards = s.api.wire.forwards.load(Ordering::SeqCst);
    let second = s.start().await;
    assert_eq!(second["id"], first["id"]);
    assert_eq!(second["state"], "running");
    assert_eq!(second["stage"], "submit");
    assert_eq!(s.api.wire.posts(), 1);
    assert_eq!(s.api.wire.forwards.load(Ordering::SeqCst), forwards);
    // In-flight Stop keeps the possibly-created Agent name and awaits forward cleanup.
    let stopped = serde_json::to_value(s.owner.stop(&id(&first)).await).unwrap();
    assert_eq!(stopped["state"], "unknown");
    assert_eq!(stopped["stage"], "stopped");
    assert_eq!(stopped["error"], "stopped");
    assert_eq!(stopped["pending"]["kind"], "Agent");
    assert_eq!(stopped["pending"]["name"], stopped["agentName"]);
    // The client side has joined its drivers; the fixture thread notices the close shortly after.
    s.idle_forwards().await;
    assert_eq!(s.api.wire.posts(), 1);
}

#[tokio::test]
async fn stop_returns_receipts() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "running";
    let started = s.start().await;
    until(|| s.view(None)["stage"] == "observe").await;
    let stopped = serde_json::to_value(s.owner.stop(&id(&started)).await).unwrap();
    let calls = s.api.wire.calls.lock().unwrap().len();
    assert_eq!(stopped["state"], "unknown");
    assert_eq!(stopped["stage"], "stopped");
    assert_eq!(stopped["acknowledged"].as_array().unwrap().len(), 2);
    assert_eq!(stopped["pending"], Value::Null);
    assert_eq!(stopped["report"], Value::Null);
    // The client side has joined its drivers; the fixture thread notices the close shortly after.
    s.idle_forwards().await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(s.api.wire.calls.lock().unwrap().len(), calls);
    assert_eq!(s.api.wire.posts(), 2);
    // A finished attempt's Stop is observation only.
    assert_eq!(
        serde_json::to_value(s.owner.stop(&id(&started)).await).unwrap(),
        stopped
    );
}

#[tokio::test]
async fn stop_during_preflight_closes_forward_before_release() {
    let s = setup().await;
    s.idle_forwards().await;
    *s.api.wire.mode.lock().unwrap() = "hang-tools";
    let started = s.start().await;
    let tools = || {
        let calls = s.api.wire.calls.lock().unwrap();
        calls.iter().any(|c| c.0.starts_with("GET /api/v1/tools?"))
    };
    until(tools).await;
    let (owner, api, send_id) = (s.owner.clone(), s.api.clone(), id(&started));
    // Runs on the one test-runtime thread. Driver cleanup that was only
    // scheduled when admission was released is queued behind this task.
    let probe = tokio::spawn(async move {
        let mut stop = Box::pin(owner.stop(&send_id));
        std::future::poll_fn(|cx| {
            assert!(stop.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        drop(stop);
        std::future::poll_fn(|cx| {
            if owner.status(Some(&send_id)).unwrap().state == SendState::Running {
                cx.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
            std::task::Poll::Ready(())
        })
        .await;
        // Hold the thread: only a forward already closed by the job can reach the server now.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while api.wire.active.load(Ordering::SeqCst) != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        api.wire.active.load(Ordering::SeqCst)
    });
    assert_eq!(
        probe.await.unwrap(),
        0,
        "admission released before forward close"
    );
    let view = s.view(Some(&id(&started)));
    assert_eq!(view["stage"], "stopped");
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn ipc_waiter_loss_keeps_native_owner() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "running";
    let started = s.start().await;
    until(|| s.view(None)["stage"] == "observe").await;
    let send_id = id(&started);
    let mut waiter = Box::pin(s.owner.stop(&send_id));
    std::future::poll_fn(|cx| {
        assert!(waiter.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    drop(waiter);
    let view = s.finish(&id(&started)).await;
    assert_eq!(view["stage"], "stopped");
    assert_eq!(view["acknowledged"].as_array().unwrap().len(), 2);
    s.idle_forwards().await;
}

#[tokio::test]
async fn unknown_explicit_resend_fresh_agent() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "broken";
    let first = s.finish(&id(&s.start().await)).await;
    assert_eq!(first["state"], "unknown");
    assert_eq!(first["stage"], "submit");
    assert_eq!(first["error"], "submit");
    assert_eq!(first["pending"]["name"], first["agentName"]);
    assert!(first["acknowledged"].as_array().unwrap().is_empty());
    *s.api.wire.mode.lock().unwrap() = "";
    let second = s.finish(&id(&s.start().await)).await;
    assert_ne!(second["id"], first["id"]);
    assert_eq!(second["state"], "complete");
    assert_ne!(second["agentName"], first["agentName"]);
    let posts = s.agent_posts();
    assert_eq!(posts.len(), 2);
    assert_ne!(posts[0]["name"], posts[1]["name"]);
    // The old unknown attempt keeps its possibly-created identity; nothing is adopted or replayed.
    assert_eq!(s.view(Some(&id(&first))), first);
    assert_eq!(s.api.wire.posts(), 3);
}

#[tokio::test]
async fn replaced_installation_or_kubeconfig_blocks_before_post() {
    let s = setup().await;
    *s.api.replacement.lock().unwrap() = true;
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["state"], "unknown");
    assert_eq!(view["error"], "preflight");
    *s.api.replacement.lock().unwrap() = false;
    let server = {
        let mut fresh = s.fresh.lock().unwrap();
        let cluster = fresh.clusters[0].cluster.as_mut().unwrap();
        let server = cluster.server.clone().unwrap();
        cluster.server = Some(format!("{server}/different-api-root"));
        server
    };
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["error"], "preflight", "{view}");
    assert!(!view.to_string().contains(&server));
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn provider_readiness_and_name_are_rechecked_before_writes() {
    let s = setup().await;
    // Current readiness, not cached Use readiness, decides admission.
    *s.api.wire.mode.lock().unwrap() = "unready";
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["error"], "preflight", "{view}");
    assert_eq!(s.api.wire.posts(), 0);
    // The selected configured Provider was removed/renamed after Use.
    *s.api.wire.mode.lock().unwrap() = "paged";
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["error"], "preflight");
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn missing_builtin_blocks_before_post() {
    let s = setup_with(&AGENT.replace("name: web_fetch", "name: absent_tool")).await;
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["state"], "unknown");
    assert_eq!(view["stage"], "preflight");
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn unknown_ids_and_missing_access_never_write() {
    let s = setup().await;
    let view =
        serde_json::to_value(s.owner.start(&s.target, "not-a-provider", s.source()).await).unwrap();
    let view = s.finish(&id(&view)).await;
    assert_eq!(view["error"], "access");
    assert_eq!(view["providerId"], "");
    let view = serde_json::to_value(
        s.owner
            .start("https://example.com", &s.provider, s.source())
            .await,
    )
    .unwrap();
    let view = s.finish(&id(&view)).await;
    assert_eq!(view["error"], "access");
    assert_eq!(view["targetName"], Value::Null);
    assert!(s.owner.status(Some("unknown")).is_none());
    assert!(s.owner.stop("unknown").await.is_none());
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn shutdown_stops_and_refuses_new_admission() {
    let s = setup().await;
    *s.api.wire.mode.lock().unwrap() = "running";
    let started = s.start().await;
    until(|| s.view(None)["stage"] == "observe").await;
    s.owner.shutdown().await;
    assert_eq!(s.view(Some(&id(&started)))["stage"], "stopped");
    // The client side has joined its drivers; the fixture thread notices the close shortly after.
    s.idle_forwards().await;
    let refused = s.start().await;
    assert_ne!(refused["state"], "running");
    assert_eq!(s.api.wire.posts(), 2);
}

#[tokio::test]
async fn planned_agent_saved_before_post_and_crash_midcreate_unknown_with_no_uid() {
    use bridge_engine::send_summary::{AttemptVerdict, SummaryStore};
    let history = tempfile::tempdir().unwrap();
    let s = setup_with_auth_and_history(
        AGENT,
        json!({"token":"creator-fixture"}),
        Some(history.path().into()),
    )
    .await;
    *s.api.wire.mode.lock().unwrap() = "hang";
    let first = s.start().await;
    until(|| s.api.wire.posts() == 1).await;
    // The Agent request is in flight, with no ACK. Moving begin() after submit
    // would leave this real request without its planned identity on disk.
    let saved = SummaryStore::new(Some(history.path().into()))
        .list()
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].planned_agent_name, s.view(None)["agentName"]);
    assert_eq!(saved[0].verdict, AttemptVerdict::Running);
    assert!(saved[0].agent_uid.is_none());
    let reopened = SendOwner::with_history_root(
        Arc::new(AgentFolder::new(None)),
        ClusterSession::default(),
        Some(history.path().into()),
    );
    let calls = s.api.wire.calls.lock().unwrap().len();
    let restored = reopened.history().unwrap();
    assert_eq!(restored[0].verdict, AttemptVerdict::Unknown);
    assert!(restored[0].agent_uid.is_none());
    assert!(reopened.status(None).is_none());
    assert_eq!(s.api.wire.calls.lock().unwrap().len(), calls);
    s.owner.stop(&id(&first)).await;
    s.idle_forwards().await;
}

#[tokio::test]
async fn explicit_resend_new_attempt_preserves_old_unknown() {
    use bridge_engine::send_summary::AttemptVerdict;
    let history = tempfile::tempdir().unwrap();
    let s = setup_with_auth_and_history(
        AGENT,
        json!({"token":"creator-fixture"}),
        Some(history.path().into()),
    )
    .await;
    *s.api.wire.mode.lock().unwrap() = "hang";
    let first = s.start().await;
    until(|| s.api.wire.posts() == 1).await;
    s.owner.stop(&id(&first)).await;
    s.idle_forwards().await;
    let old_name = s.view(None)["agentName"].clone();
    *s.api.wire.mode.lock().unwrap() = "normal";
    let second = s.start().await;
    let done = s.finish(&id(&second)).await;
    assert_ne!(first["id"], second["id"]);
    assert_ne!(old_name, done["agentName"]);
    assert_eq!(done["report"]["verdict"], "passed");
    let reopened = SendOwner::with_history_root(
        Arc::new(AgentFolder::new(None)),
        ClusterSession::default(),
        Some(history.path().into()),
    );
    let saved = reopened.history().unwrap();
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0].id, id(&second));
    assert_eq!(saved[0].verdict, AttemptVerdict::Passed);
    assert!(saved[0].agent_uid.is_some());
    assert_eq!(saved[1].id, id(&first));
    assert_eq!(saved[1].verdict, AttemptVerdict::Unknown);
    assert!(saved[1].agent_uid.is_none());
    let bytes = std::fs::read(history.path().join("send-history-v1/history.json")).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains(AGENT));
    assert!(!text.contains(TESTS));
    assert!(!text.contains("creator-fixture"));
    assert!(!text.contains("bridge-task-"));
    assert!(!text.contains("expected answer"));
}

#[tokio::test]
async fn storage_failure_visible_and_never_passes() {
    let history = tempfile::tempdir().unwrap();
    let blocked = history.path().join("history-root");
    std::fs::write(&blocked, b"not-a-directory").unwrap();
    let s =
        setup_with_auth_and_history(AGENT, json!({"token":"creator-fixture"}), Some(blocked)).await;
    let view = s.finish(&id(&s.start().await)).await;
    assert_eq!(view["storageError"], true);
    assert_eq!(view["state"], "unknown");
    assert_eq!(view["report"], Value::Null);
    assert!(s.owner.history().is_err());
    assert_eq!(view["error"], "storage");
    assert_eq!(s.api.wire.posts(), 0);
}

#[tokio::test]
async fn send_start_future_is_send_for_desktop_dispatch() {
    let s = setup().await;
    let owner = s.owner.clone();
    let (target, provider) = (s.target.clone(), s.provider.clone());
    let source = s.source();
    let view = tokio::spawn(async move { owner.start(&target, &provider, source).await })
        .await
        .unwrap();
    s.finish(&id(&serde_json::to_value(view).unwrap())).await;
}

#[tokio::test]
async fn reload_during_send_keeps_its_frozen_target_and_requires_use_again() {
    let s = setup().await;
    // Reload invalidates selection, but cannot start a second configuration read
    // inside this attempt's reader. The already-admitted Send keeps frozen access.
    let (session, handle) = (s.session.clone(), tokio::runtime::Handle::current());
    *s.on_read.lock().unwrap() = Some(Box::new(move || {
        assert!(handle.block_on(session.reload()).is_err());
    }));
    let first = s.start().await;
    let done = s.finish(&id(&first)).await;
    assert_eq!(done["state"], "complete");
    assert_eq!(done["report"]["verdict"], "passed");
    assert_eq!(s.agent_posts().len(), 1);
    s.idle_forwards().await;
    // Future admission cannot reuse the old target or access.
    let next = s.start().await;
    let next = s.finish(&id(&next)).await;
    assert_eq!(next["error"], "access");
    assert_eq!(s.agent_posts().len(), 1);
}

#[tokio::test]
async fn reload_immediately_after_running_admission_keeps_its_access() {
    let s = setup().await;
    let started = s.start().await;
    assert_eq!(started["state"], "running");
    assert_eq!(started["stage"], "capture");
    s.session.reload().await.unwrap();
    let done = s.finish(&id(&started)).await;
    assert_eq!(done["report"]["verdict"], "passed", "{done}");
    assert_eq!(s.agent_posts().len(), 1);
    s.idle_forwards().await;
    let next = s.start().await;
    assert_eq!(s.finish(&id(&next)).await["error"], "access");
    assert_eq!(s.agent_posts().len(), 1);
}

#[tokio::test]
async fn admission_does_not_wait_for_discovery_lock() {
    let s = setup().await;
    let (release, gate) = std::sync::mpsc::channel();
    *s.api.gate.lock().unwrap() = Some(gate);
    let contexts = serde_json::to_value(s.session.contexts().await.unwrap()).unwrap();
    let services = || {
        let calls = s.api.calls.lock().unwrap();
        calls.iter().filter(|c| c.1.ends_with("/services")).count()
    };
    let before = services();
    let session = s.session.clone();
    let discovery = tokio::spawn(async move { session.discover(&id(&contexts[0])).await.is_ok() });
    until(|| services() > before).await;
    // Discovery now holds the session lock across a Kubernetes read.
    let quick = Duration::from_secs(1);
    let first = tokio::time::timeout(quick, s.start()).await.unwrap();
    assert_eq!(first["state"], "running");
    let second = tokio::time::timeout(quick, s.start()).await.unwrap();
    assert_eq!(second["id"], first["id"]);
    assert_eq!(s.view(None)["state"], "running");
    release.send(()).unwrap();
    assert!(discovery.await.unwrap());
    // Rediscovery cleared the earlier Use, so this attempt never reaches a write.
    let view = s.finish(&id(&first)).await;
    assert_eq!(view["error"], "access");
    assert_eq!(s.api.wire.posts(), 0);
}
