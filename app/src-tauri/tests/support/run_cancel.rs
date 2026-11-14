use super::*;
use bridge_engine::{agent_folder::DocumentId, engine::run::Verdict};
use std::sync::atomic::{AtomicU16, AtomicU64};

// A wire-level guest double: production VmClient, transport and Run owner execute.
// Mutating proof to accept a running pod, foreign UID or failed POST breaks these tests.
pub(super) struct Script {
    pub(super) trace: Mutex<Vec<(String, Option<Value>)>>,
    pub(super) phase: Mutex<String>,
    pub(super) cancel_phase: Mutex<Option<String>>,
    pub(super) replace_on_cancel: AtomicBool,
    pub(super) lose_create: AtomicBool,
    pub(super) read_failed: AtomicBool,
    pub(super) bad_task: Mutex<Option<String>>,
    pub(super) completed_task: Mutex<Option<String>>,
    pub(super) evidence_missing: AtomicBool,
    pub(super) deny_first: AtomicBool,
    pub(super) status: AtomicU16,
    pub(super) pods: AtomicU64,
    pub(super) replaced: AtomicBool,
    pub(super) worker_bad: AtomicBool,
    pub(super) fail_cancel: AtomicBool,
    pub(super) hook: Mutex<Option<Box<dyn Fn() + Send>>>,
    pub(super) model_hook: Mutex<Option<Box<dyn Fn() + Send>>>,
    pub(super) cancel_hook: Mutex<Option<Box<dyn Fn() + Send>>>,
    pub(super) evidence_hook: Mutex<Option<Box<dyn Fn() + Send>>>,
    pub(super) events_missing: AtomicBool,
    pub(super) release_hook: Mutex<Option<Box<dyn Fn() + Send>>>,
}
impl Default for Script {
    fn default() -> Self {
        Self {
            trace: Mutex::default(),
            phase: Mutex::new("Running".into()),
            cancel_phase: Mutex::default(),
            replace_on_cancel: AtomicBool::new(false),
            lose_create: AtomicBool::new(false),
            read_failed: AtomicBool::new(false),
            bad_task: Mutex::default(),
            completed_task: Mutex::default(),
            evidence_missing: AtomicBool::new(false),
            deny_first: AtomicBool::new(false),
            status: AtomicU16::new(200),
            pods: AtomicU64::new(0),
            replaced: AtomicBool::new(false),
            worker_bad: AtomicBool::new(false),
            fail_cancel: AtomicBool::new(false),
            hook: Mutex::default(),
            model_hook: Mutex::default(),
            cancel_hook: Mutex::default(),
            evidence_hook: Mutex::default(),
            events_missing: AtomicBool::new(false),
            release_hook: Mutex::default(),
        }
    }
}
impl Script {
    fn posts(&self) -> Vec<(String, Option<Value>)> {
        self.trace
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, body)| body.is_some())
            .cloned()
            .collect()
    }
}
async fn cancel_fixture(
    script: Arc<Script>,
    supported: bool,
) -> (Arc<LocalVm>, Arc<ExampleRun>, tempfile::TempDir) {
    let id = Uuid::new_v4();
    let client = VmClient::established(
        Box::new(Channel {
            input: std::io::Cursor::new(Vec::new()),
            objects: HashMap::new(),
            output: Vec::new(),
            missing_tools: false,
            posts: Arc::default(),
            lost_task: false,
            cleanup_reply: None,
            cancel: Some(script.clone()),
        }),
        id,
    )
    .unwrap();
    let vm = LocalVm::new(Arc::new(Factory(Mutex::new(Some(Session(
        client,
        Arc::default(),
        id,
        false,
        supported,
        Some(script),
    ))))));
    vm.start();
    for _ in 0..100 {
        if vm.status() == LocalVmStatus::Ready {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(vm.status(), LocalVmStatus::Ready);
    let home = tempfile::tempdir().unwrap();
    let run = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        Arc::new(AgentFolder::new(Some(home.path().into()))),
        Some(home.path().join("history")),
    );
    (vm, run, home)
}
async fn observing(script: &Script) {
    for _ in 0..500 {
        if script.trace.lock().unwrap().iter().any(|(p, b)| {
            b.is_none()
                && p.starts_with("tasks/")
                && !p.ends_with("/result")
                && !p.ends_with("/events")
        }) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("no Task observation");
}
#[tokio::test]
async fn cancel_before_capture_creates_nothing_and_is_not_a_pass() {
    let script = Arc::new(Script::default());
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    let requested = run.cancel(vm.clone(), DocumentId::SAMPLE);
    assert_eq!(
        serde_json::to_value(requested).unwrap()["cancellation"],
        "cancelling"
    );
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "beforeStart"
    );
    assert_eq!(done.report.unwrap().verdict, Verdict::Cancelled);
    assert!(script.posts().is_empty());
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn old_image_and_foreign_document_cannot_cancel_or_probe() {
    for supported in [false, true] {
        let script = Arc::new(Script::default());
        let (vm, run, _home) = cancel_fixture(script.clone(), supported).await;
        run.start(vm.clone());
        let foreign = if supported {
            serde_json::from_value(json!(Uuid::new_v4())).unwrap()
        } else {
            DocumentId::SAMPLE
        };
        run.cancel(vm.clone(), foreign);
        assert_eq!(
            serde_json::to_value(run.status(&vm)).unwrap()["cancellation"],
            "none"
        );
        observing(&script).await;
        assert!(script.posts().iter().all(|(p, _)| !p.ends_with("/cancel")));
        *script.phase.lock().unwrap() = "Succeeded".into();
        assert_eq!(finished(&run, &vm).await.state, RunState::Complete);
        vm.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn cancel_200_proves_worker_preserves_receipts_and_history() {
    let script = Arc::new(Script::default());
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    assert_eq!(
        serde_json::to_value(run.status(&vm)).unwrap()["canCancel"],
        true
    );
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    run.cancel(vm.clone(), DocumentId::SAMPLE); // no duplicate POST
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "cancelled"
    );
    let report = done.report.unwrap();
    assert_eq!(report.verdict, Verdict::Cancelled);
    assert_eq!(report.cases[0].phase, "Cancelled");
    assert!(report.cases[0].assertions.is_empty());
    assert_eq!(done.acknowledged.len(), 2);
    let posts = script.posts();
    assert_eq!(posts.len(), 3);
    assert_eq!(
        posts[2].1.as_ref().unwrap(),
        &json!({"uid":done.acknowledged[1].uid})
    );
    let history = run.local_results().unwrap();
    assert_eq!(
        history.results[0].report.as_ref().unwrap().verdict,
        Verdict::Cancelled
    );
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn conflict_requires_terminal_same_uid_and_keeps_finished_answer() {
    for (phase, replaced, confirmed) in [
        ("Succeeded", false, true),
        ("Running", false, false),
        ("Succeeded", true, false),
    ] {
        let script = Arc::new(Script::default());
        script.status.store(409, Ordering::SeqCst);
        let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
        run.start(vm.clone());
        observing(&script).await;
        *script.cancel_phase.lock().unwrap() = Some(phase.into());
        script.replace_on_cancel.store(replaced, Ordering::SeqCst);
        run.cancel(vm.clone(), DocumentId::SAMPLE);
        let done = finished(&run, &vm).await;
        assert_eq!(
            serde_json::to_value(&done).unwrap()["cancellation"],
            if confirmed {
                "cancelled"
            } else {
                "unconfirmed"
            }
        );
        if confirmed {
            let report = done.report.unwrap();
            assert_eq!(report.verdict, Verdict::Cancelled);
            assert_eq!(report.cases[0].verdict, Verdict::Passed);
            assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
        }
        vm.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn refusal_then_check_is_read_only_and_retry_posts_only_running_same_uid() {
    let script = Arc::new(Script::default());
    script.status.store(403, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let refused = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(refused).unwrap()["canRetryCancellation"],
        true
    );
    let before = script.posts().len();
    *script.phase.lock().unwrap() = "Succeeded".into();
    run.check_cancellation(vm.clone(), DocumentId::SAMPLE);
    let checked = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(checked).unwrap()["cancellation"],
        "cancelled"
    );
    assert_eq!(script.posts().len(), before);
    vm.shutdown().await.unwrap();

    let script = Arc::new(Script::default());
    script.status.store(403, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    finished(&run, &vm).await;
    script.status.store(200, Ordering::SeqCst);
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    assert_eq!(
        serde_json::to_value(finished(&run, &vm).await).unwrap()["cancellation"],
        "cancelled"
    );
    assert_eq!(script.posts().len(), 4); // two creates and two explicit cancel attempts
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn a_terminal_task_with_an_unfinished_worker_times_out_but_keeps_its_answer() {
    let script = Arc::new(Script::default());
    script.pods.store(1, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    script.status.store(409, Ordering::SeqCst);
    *script.cancel_phase.lock().unwrap() = Some("Succeeded".into());
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    for _ in 0..500 {
        if script
            .trace
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.starts_with("worker/"))
            .count()
            >= 2
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(
        script
            .trace
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.starts_with("worker/"))
            .count()
            >= 2
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(61)).await;
    tokio::time::resume();
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert!(
        !done.can_retry_cancellation,
        "terminal Tasks must never be cancelled again"
    );
    let report = done.report.unwrap();
    assert_eq!(
        report.verdict,
        Verdict::Unknown,
        "unconfirmed must not be a pass"
    );
    assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
    vm.shutdown().await.unwrap();
}

fn stop_on_task_create(script: &Script, run: &Arc<ExampleRun>, vm: &Arc<LocalVm>) {
    let run = Arc::downgrade(run);
    let vm = Arc::downgrade(vm);
    *script.hook.lock().unwrap() = Some(Box::new(move || {
        run.upgrade()
            .unwrap()
            .cancel(vm.upgrade().unwrap(), DocumentId::SAMPLE);
    }));
}

#[tokio::test]
async fn synchronous_create_ack_is_kept_and_later_tests_are_never_submitted() {
    let script = Arc::new(Script::default());
    let (vm, run, home) = cancel_fixture(script.clone(), true).await;
    let folder = AgentFolder::new(Some(home.path().into()));
    let pair = folder.open().unwrap();
    let tests = std::path::Path::new(pair.path.as_ref().unwrap())
        .with_file_name("web-reader.agent.test.md");
    std::fs::write(tests, format!("{}\n```yaml agent-test\nid: not-submitted\ninput: Never run\nassertions:\n - id: ignored\n   type: contains\n   value: unused\n```\n", pair.tests)).unwrap();
    stop_on_task_create(&script, &run, &vm);
    run.start(vm.clone());
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "cancelled"
    );
    assert_eq!(done.acknowledged.len(), 2);
    assert!(done.pending.is_none());
    assert_eq!(done.report.as_ref().unwrap().verdict, Verdict::Cancelled);
    assert_eq!(done.report.as_ref().unwrap().cases.len(), 1);
    let posts = script.posts();
    assert_eq!(posts.len(), 3); // Agent, first Task, first Task's cancel
    assert_eq!(posts.iter().filter(|(p, _)| p == "tasks").count(), 1);
    assert_eq!(
        posts[2].1.as_ref().unwrap(),
        &json!({"uid":done.acknowledged[1].uid})
    );
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_create_reply_retains_pending_name_and_never_adopts_or_cancels_its_uid() {
    let script = Arc::new(Script::default());
    script.lose_create.store(true, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    stop_on_task_create(&script, &run, &vm);
    run.start(vm.clone());
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert_eq!(done.acknowledged.len(), 1);
    assert_eq!(done.pending.unwrap().kind, "Task");
    assert!(!done.can_retry_cancellation);
    assert_eq!(script.posts().len(), 2);
    let before = script.posts().len();
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    assert!(!run.is_active());
    assert_eq!(script.posts().len(), before);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_cancel_is_unconfirmed_and_is_never_replayed_automatically() {
    let script = Arc::new(Script::default());
    script.fail_cancel.store(true, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert_eq!(done.acknowledged.len(), 2);
    assert_eq!(script.posts().len(), 3);
    assert!(done.pending.is_none()); // original create acknowledgment remains exact
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_cancelled_task_waits_for_worker_proof_without_reposting_cancel() {
    let script = Arc::new(Script::default());
    script.pods.store(1, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    for _ in 0..500 {
        if script
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p.starts_with("worker/"))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let waiting = run.status(&vm);
    assert_eq!(waiting.state, RunState::Running);
    assert_eq!(waiting.stage, Some("cancel"));
    assert_eq!(
        serde_json::to_value(waiting).unwrap()["cancellation"],
        "cancelling"
    );
    assert_eq!(script.posts().len(), 3);
    script.pods.store(0, Ordering::SeqCst);
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(done).unwrap()["cancellation"],
        "cancelled"
    );
    assert_eq!(script.posts().len(), 3);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn recovery_refuses_another_installation_without_any_guest_request() {
    let script = Arc::new(Script::default());
    script.status.store(403, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let previous = finished(&run, &vm).await;
    let other_script = Arc::new(Script::default());
    let (other_vm, _other_run, _other_home) = cancel_fixture(other_script.clone(), true).await;
    run.check_cancellation(other_vm.clone(), DocumentId::SAMPLE);
    let refused = finished(&run, &other_vm).await;
    assert_eq!(refused.error, Some("session"));
    assert_eq!(
        serde_json::to_value(&refused).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert!(!refused.can_check_cancellation);
    assert_eq!(refused.acknowledged[1].uid, previous.acknowledged[1].uid);
    assert!(other_script.trace.lock().unwrap().is_empty());
    assert!(
        run.local_results().unwrap().results[0].report.is_some(),
        "failed recovery must preserve its own saved summary"
    );
    vm.shutdown().await.unwrap();
    other_vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn check_read_failure_keeps_finished_answers_from_the_first_cancel() {
    let script = Arc::new(Script::default());
    script.worker_bad.store(true, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    script.status.store(409, Ordering::SeqCst);
    *script.cancel_phase.lock().unwrap() = Some("Succeeded".into());
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let first = finished(&run, &vm).await;
    assert_eq!(
        first.report.unwrap().cases[0].answer.as_deref(),
        Some(ANSWER)
    );
    script.read_failed.store(true, Ordering::SeqCst);
    run.check_cancellation(vm.clone(), DocumentId::SAMPLE);
    let checked = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&checked).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert_eq!(
        checked.report.unwrap().cases[0].answer.as_deref(),
        Some(ANSWER)
    );
    assert!(!checked.can_retry_cancellation);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn one_refused_cancel_does_not_skip_other_tasks_and_retry_rechecks_terminal_ones() {
    let script = Arc::new(Script::default());
    script.deny_first.store(true, Ordering::SeqCst);
    let (vm, run, home) = cancel_fixture(script.clone(), true).await;
    let folder = AgentFolder::new(Some(home.path().into()));
    let pair = folder.open().unwrap();
    let tests = std::path::Path::new(pair.path.as_ref().unwrap())
        .with_file_name("web-reader.agent.test.md");
    std::fs::write(tests, format!("{}\n```yaml agent-test\nid: second\ninput: Another question\nassertions:\n - id: answer\n   type: contains\n   value: unused\n```\n", pair.tests)).unwrap();
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let first = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&first).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert_eq!(first.acknowledged.len(), 3);
    let posts = script.posts();
    let cancels: Vec<_> = posts
        .iter()
        .filter(|(p, _)| p.ends_with("/cancel"))
        .collect();
    assert_eq!(
        cancels.len(),
        2,
        "refusal must not skip the next acknowledged Task"
    );
    assert_ne!(cancels[0].0, cancels[1].0);
    assert_eq!(
        cancels[0].1.as_ref().unwrap()["uid"],
        first.acknowledged[1].uid
    );
    assert_eq!(
        cancels[1].1.as_ref().unwrap()["uid"],
        first.acknowledged[2].uid
    );
    assert!(first.can_retry_cancellation);
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let retry = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(retry).unwrap()["cancellation"],
        "cancelled"
    );
    assert_eq!(
        script.posts().len(),
        posts.len(),
        "both Tasks finished in the meantime, no new cancel POST"
    );
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_during_synchronous_model_check_submits_nothing() {
    let script = Arc::new(Script::default());
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    let owner = Arc::downgrade(&run);
    let target = Arc::downgrade(&vm);
    *script.model_hook.lock().unwrap() = Some(Box::new(move || {
        owner
            .upgrade()
            .unwrap()
            .cancel(target.upgrade().unwrap(), DocumentId::SAMPLE);
    }));
    run.start(vm.clone());
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "beforeStart"
    );
    assert_eq!(done.report.unwrap().verdict, Verdict::Cancelled);
    assert!(done.acknowledged.is_empty());
    assert!(script.posts().is_empty());
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn recovery_refuses_a_new_boot_even_on_the_same_installation() {
    let script = Arc::new(Script::default());
    script.status.store(403, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    finished(&run, &vm).await;
    let installation = run.overview_observation().unwrap().installation;
    let other_script = Arc::new(Script::default());
    let client = VmClient::established(
        Box::new(Channel {
            input: std::io::Cursor::new(Vec::new()),
            objects: HashMap::new(),
            output: Vec::new(),
            missing_tools: false,
            posts: Arc::default(),
            lost_task: false,
            cleanup_reply: None,
            cancel: Some(other_script.clone()),
        }),
        Uuid::new_v4(),
    )
    .unwrap();
    let other_vm = LocalVm::new(Arc::new(Factory(Mutex::new(Some(Session(
        client,
        Arc::default(),
        installation,
        false,
        true,
        None,
    ))))));
    other_vm.start();
    for _ in 0..100 {
        if other_vm.status() == LocalVmStatus::Ready {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(other_vm.status(), LocalVmStatus::Ready);
    run.cancel(other_vm.clone(), DocumentId::SAMPLE);
    let refused = finished(&run, &other_vm).await;
    assert_eq!(refused.error, Some("session"));
    assert!(!refused.can_check_cancellation);
    assert!(
        other_script.trace.lock().unwrap().is_empty(),
        "boot mismatch must refuse before any guest I/O"
    );
    vm.shutdown().await.unwrap();
    other_vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancel_after_observation_has_finished_is_not_accepted_during_session_release() {
    let script = Arc::new(Script::default());
    *script.phase.lock().unwrap() = "Succeeded".into();
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    let owner = Arc::downgrade(&run);
    let target = Arc::downgrade(&vm);
    let offered = Arc::new(AtomicBool::new(true));
    let seen = offered.clone();
    *script.release_hook.lock().unwrap() = Some(Box::new(move || {
        let owner = owner.upgrade().unwrap();
        let target = target.upgrade().unwrap();
        seen.store(owner.status(&target).can_cancel, Ordering::SeqCst);
        owner.cancel(target, DocumentId::SAMPLE);
    }));
    run.start(vm.clone());
    let done = finished(&run, &vm).await;
    assert!(
        !offered.load(Ordering::SeqCst),
        "completion must atomically retire cancellation admission"
    );
    assert_eq!(done.state, RunState::Complete);
    assert_eq!(serde_json::to_value(&done).unwrap()["cancellation"], "none");
    assert_eq!(done.report.unwrap().verdict, Verdict::Passed);
    assert_eq!(script.posts().len(), 2);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_recovery_for_a_does_not_change_b_history_or_latest_observation() {
    let script = Arc::new(Script::default());
    script.status.store(403, Ordering::SeqCst);
    let (vm, run, home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    finished(&run, &vm).await;
    let pair = AgentFolder::new(Some(home.path().into())).open().unwrap();
    let b = created_agent::created(home.path(), "second", &pair.agent, &pair.tests);
    let bid = b.document_id();
    *script.phase.lock().unwrap() = "Succeeded".into();
    run.start_document(vm.clone(), b);
    for _ in 0..500 {
        if !run.is_active() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        run.status_document(&vm, bid).report.unwrap().verdict,
        Verdict::Passed
    );
    let other_script = Arc::new(Script::default());
    let (other_vm, _other_run, _other_home) = cancel_fixture(other_script.clone(), true).await;
    run.check_cancellation(other_vm.clone(), DocumentId::SAMPLE);
    let failed = finished(&run, &other_vm).await;
    assert_eq!(failed.error, Some("session"));
    let latest = run.overview_observation().unwrap();
    assert_eq!(latest.document_id, bid);
    assert_eq!(latest.state, RunState::Complete);
    assert_eq!(latest.report.unwrap().verdict, Verdict::Passed);
    let history = run.local_results().unwrap();
    assert_eq!(
        history
            .results
            .iter()
            .find(|r| r.document_id == bid)
            .unwrap()
            .report
            .as_ref()
            .unwrap()
            .verdict,
        Verdict::Passed
    );
    assert!(history
        .results
        .iter()
        .find(|r| r.document_id == DocumentId::SAMPLE)
        .unwrap()
        .report
        .is_some());
    vm.shutdown().await.unwrap();
    other_vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn initial_cancel_keeps_a_completed_case_when_a_later_task_read_fails() {
    let script = Arc::new(Script::default());
    let (vm, run, home) = cancel_fixture(script.clone(), true).await;
    let pair = AgentFolder::new(Some(home.path().into())).open().unwrap();
    let tests = std::path::Path::new(pair.path.as_ref().unwrap())
        .with_file_name("web-reader.agent.test.md");
    std::fs::write(tests, format!("{}\n```yaml agent-test\nid: unreadable\ninput: Another question\nassertions:\n - id: answer\n   type: contains\n   value: unused\n```\n", pair.tests)).unwrap();
    run.start(vm.clone());
    observing(&script).await;
    let second = script.posts()[2].1.as_ref().unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let changed = script.clone();
    *script.cancel_hook.lock().unwrap() = Some(Box::new(move || {
        *changed.bad_task.lock().unwrap() = Some(second.clone())
    }));
    script.status.store(409, Ordering::SeqCst);
    *script.cancel_phase.lock().unwrap() = Some("Succeeded".into());
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "unconfirmed"
    );
    let report = done
        .report
        .expect("keep the completed first case despite a later read failure");
    assert_eq!(report.verdict, Verdict::Unknown);
    assert_eq!(report.cases.len(), 1);
    assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
    assert_eq!(report.cases[0].verdict, Verdict::Passed);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn recheck_merges_missing_evidence_for_the_same_case_and_recomputes_assertions() {
    let script = Arc::new(Script::default());
    script.worker_bad.store(true, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    script.status.store(409, Ordering::SeqCst);
    *script.cancel_phase.lock().unwrap() = Some("Succeeded".into());
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let first = finished(&run, &vm).await;
    assert_eq!(first.report.unwrap().cases[0].verdict, Verdict::Passed);
    script.worker_bad.store(false, Ordering::SeqCst);
    script.evidence_missing.store(true, Ordering::SeqCst);
    run.check_cancellation(vm.clone(), DocumentId::SAMPLE);
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "cancelled"
    );
    let report = done.report.unwrap();
    assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
    assert_eq!(report.cases[0].completed_tools, vec!["web_fetch"]);
    assert_eq!(report.cases[0].verdict, Verdict::Passed);
    assert!(report.cases[0]
        .assertions
        .iter()
        .all(|a| a.verdict == Verdict::Passed));
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn ordinary_observation_keeps_a_finished_answer_when_cancellation_reads_fail() {
    let script = Arc::new(Script::default());
    let (vm, run, home) = cancel_fixture(script.clone(), true).await;
    let pair = AgentFolder::new(Some(home.path().into())).open().unwrap();
    let tests = std::path::Path::new(pair.path.as_ref().unwrap())
        .with_file_name("web-reader.agent.test.md");
    std::fs::write(tests, format!("{}\n```yaml agent-test\nid: still-running\ninput: Another question\nassertions:\n - id: answer\n   type: contains\n   value: unused\n```\n", pair.tests)).unwrap();
    run.start(vm.clone());
    observing(&script).await;
    let writes = script.posts();
    let first = writes[1].1.as_ref().unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = writes[2].1.as_ref().unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned();
    *script.completed_task.lock().unwrap() = Some(first);
    let second_path = format!("tasks/{second}");
    for _ in 0..500 {
        if script
            .trace
            .lock()
            .unwrap()
            .iter()
            .any(|(path, body)| path == &second_path && body.is_none())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    assert!(script
        .trace
        .lock()
        .unwrap()
        .iter()
        .any(|(path, body)| path == &second_path && body.is_none()));
    let changed = script.clone();
    *script.cancel_hook.lock().unwrap() = Some(Box::new(move || {
        changed.evidence_missing.store(true, Ordering::SeqCst);
        *changed.bad_task.lock().unwrap() = Some(second.clone());
    }));
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "unconfirmed"
    );
    let report = done.report.unwrap();
    assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
    assert_eq!(report.cases[0].verdict, Verdict::Passed);
    assert_eq!(report.cases[0].completed_tools, vec!["web_fetch"]);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn temporary_read_session_contention_keeps_both_recovery_controls_available() {
    let script = Arc::new(Script::default());
    script.status.store(403, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    let first = finished(&run, &vm).await;
    assert!(first.can_retry_cancellation && first.can_check_cancellation);
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let reading = vm.clone();
    let read = tokio::spawn(async move {
        reading
            .with_client(move |_, _| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .await
    });
    entered_rx.await.unwrap();
    let writes = script.posts().len();
    run.check_cancellation(vm.clone(), DocumentId::SAMPLE);
    let busy = finished(&run, &vm).await;
    release_tx.send(()).unwrap();
    read.await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&busy).unwrap()["cancellation"],
        "unconfirmed"
    );
    assert_eq!(busy.error, None);
    assert!(busy.can_retry_cancellation && busy.can_check_cancellation);
    assert_eq!(script.posts().len(), writes);
    script.status.store(200, Ordering::SeqCst);
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    assert_eq!(
        serde_json::to_value(finished(&run, &vm).await).unwrap()["cancellation"],
        "cancelled"
    );
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_evidence_grace_keeps_the_already_read_answer() {
    let script = Arc::new(Script::default());
    *script.phase.lock().unwrap() = "Succeeded".into();
    script.events_missing.store(true, Ordering::SeqCst);
    script.worker_bad.store(true, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    let owner = run.clone();
    let target = vm.clone();
    let changed = script.clone();
    *script.evidence_hook.lock().unwrap() = Some(Box::new(move || {
        owner.cancel(target.clone(), DocumentId::SAMPLE);
        changed.evidence_missing.store(true, Ordering::SeqCst);
    }));
    run.start(vm.clone());
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "unconfirmed"
    );
    let report = done.report.unwrap();
    assert_eq!(report.cases.len(), 1);
    assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
    assert!(report.cases[0]
        .assertions
        .iter()
        .any(|a| a.kind == "contains" && a.verdict == Verdict::Passed));
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn late_create_ack_keeps_its_uid_and_is_cancelled_after_the_submission_deadline() {
    let script = Arc::new(Script::default());
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    *script.hook.lock().unwrap() = Some(Box::new(move || {
        entered_tx.send(()).unwrap();
        release_rx.lock().unwrap().recv().unwrap();
    }));
    run.start(vm.clone());
    tokio::task::spawn_blocking(move || entered_rx.recv().unwrap())
        .await
        .unwrap();
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    tokio::time::pause();
    tokio::time::advance(
        bridge_engine::engine::run::RunLimits::default().operation_timeout
            + std::time::Duration::from_secs(1),
    )
    .await;
    tokio::time::resume();
    release_tx.send(()).unwrap();
    let done = finished(&run, &vm).await;
    assert_eq!(
        serde_json::to_value(&done).unwrap()["cancellation"],
        "cancelled"
    );
    assert_eq!(done.acknowledged.len(), 2);
    assert!(done.pending.is_none());
    let posts = script.posts();
    assert_eq!(
        posts.len(),
        3,
        "keep two create receipts and issue one UID-fenced cancel"
    );
    assert_eq!(
        posts[2].1.as_ref().unwrap()["uid"],
        done.acknowledged[1].uid
    );
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn malformed_worker_reply_never_confirms_cancellation() {
    let script = Arc::new(Script::default());
    script.worker_bad.store(true, Ordering::SeqCst);
    let (vm, run, _home) = cancel_fixture(script.clone(), true).await;
    run.start(vm.clone());
    observing(&script).await;
    run.cancel(vm.clone(), DocumentId::SAMPLE);
    assert_eq!(
        serde_json::to_value(finished(&run, &vm).await).unwrap()["cancellation"],
        "unconfirmed"
    );
    vm.shutdown().await.unwrap();
}
