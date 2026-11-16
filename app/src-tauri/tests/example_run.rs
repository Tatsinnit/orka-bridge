use bridge_engine::{
    agent_folder::AgentFolder,
    example_run::{ExampleRun, RunState},
    local_vm::{LocalFactory, LocalSession, LocalVm, LocalVmStatus, StartFuture},
    runtime::{client::VmClient, types::RuntimeError},
};
use bridge_runtime_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::watch;
#[path = "support/run_cancel.rs"]
mod cancellation;
#[path = "support/created_agent.rs"]
mod created_agent;

#[tokio::test]
async fn first_test_rejects_files_edited_after_renderer_open_but_manual_run_accepts_them() {
    for leaf in ["web-reader.agent.md", "web-reader.agent.test.md"] {
        let (vm, posts, _) = fixture(false, false).await;
        let home = tempfile::tempdir().unwrap();
        let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
        let viewed = folder.open().unwrap(); // Renderer has already compared this pair.
        let root = std::path::Path::new(viewed.path.as_ref().unwrap())
            .parent()
            .unwrap();
        let file = root.join(leaf);
        let edited = format!(
            "{}\nEdited after the renderer check.\n",
            std::fs::read_to_string(&file).unwrap()
        );
        std::fs::write(&file, &edited).unwrap();
        let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
        run.start_first_test(vm.clone(), viewed.document_id)
            .unwrap();
        let rejected = finished(&run, &vm).await;
        assert_eq!(rejected.state, RunState::Unknown);
        assert_eq!(rejected.error, Some("source"));
        assert!(rejected.report.is_none());
        assert!(rejected.acknowledged.is_empty());
        assert!(
            posts.lock().unwrap().is_empty(),
            "automatic test must not submit Agent or Task"
        );
        assert!(!run.is_active());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
        run.start(vm.clone()); // The explicit manual path retains its existing semantics.
        assert_eq!(finished(&run, &vm).await.state, RunState::Complete);
        assert_eq!(posts.lock().unwrap().len(), 2);
        vm.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn legacy_untitled_sample_is_preserved_and_remains_manually_runnable() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let pair = folder.open().unwrap();
    let path = std::path::Path::new(pair.path.as_ref().unwrap())
        .parent()
        .unwrap()
        .join("web-reader.agent.test.md");
    let legacy = pair
        .tests
        .replace("title: \"Copies the checksum from the page\"\n", "");
    assert_ne!(legacy, pair.tests);
    std::fs::write(&path, &legacy).unwrap();
    let prepared = folder.prepare_example_model("gpt-4.1").unwrap();
    assert_eq!(prepared.tests, legacy);
    assert_eq!(prepared.agent, pair.agent);
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
    run.start_first_test(vm.clone(), pair.document_id).unwrap();
    assert_eq!(finished(&run, &vm).await.error, Some("source"));
    assert!(posts.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), legacy);
    run.start(vm.clone());
    let done = finished(&run, &vm).await;
    assert_eq!(done.state, RunState::Complete);
    assert!(done.report.unwrap().cases[0].title.is_none());
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn first_test_accepts_bundled_lf_crlf_and_mixed_line_endings() {
    for ending in ["lf", "crlf", "mixed"] {
        let (vm, posts, _) = fixture(false, false).await;
        let home = tempfile::tempdir().unwrap();
        let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
        let pair = folder.open().unwrap();
        let root = std::path::Path::new(pair.path.as_ref().unwrap())
            .parent()
            .unwrap();
        for (leaf, text) in [
            ("web-reader.agent.md", pair.agent),
            ("web-reader.agent.test.md", pair.tests),
        ] {
            let normal = text.replace("\r\n", "\n");
            let content = match ending {
                "crlf" => normal.replace('\n', "\r\n"),
                "mixed" => normal.replacen('\n', "\r\n", 1),
                _ => normal,
            };
            std::fs::write(root.join(leaf), content).unwrap();
        }
        let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
        run.start_first_test(vm.clone(), pair.document_id).unwrap();
        assert_eq!(
            finished(&run, &vm).await.state,
            RunState::Complete,
            "{ending}"
        );
        assert_eq!(posts.lock().unwrap().len(), 2);
        vm.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn first_test_requires_the_expected_template_and_current_connection() {
    for (saved, expected, connected, error) in [
        ("gpt-4.1", "gpt-4.1", "gpt-4.1", None),
        (
            "claude-sonnet-5-5",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5",
            None,
        ),
        (
            "gpt-4.1",
            "claude-sonnet-5-5",
            "claude-sonnet-5-5",
            Some("source"),
        ),
        ("gpt-4.1", "gpt-4.1", "gpt-5.6-terra", Some("model")),
    ] {
        let (vm, posts, _) = fixture_with_model(false, false, false, None, connected).await;
        let home = tempfile::tempdir().unwrap();
        let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
        let pair = folder.prepare_example_model(saved).unwrap();
        let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
        run.start_first_test_with_model(vm.clone(), pair.document_id, expected.into())
            .unwrap();
        let done = finished(&run, &vm).await;
        assert_eq!(done.error, error, "{saved}/{expected}/{connected}");
        if error.is_none() {
            assert_eq!(
                serde_json::to_value(&done).unwrap()["vendor"],
                if connected == "gpt-4.1" {
                    "openai"
                } else {
                    "anthropic"
                }
            );
        }
        assert_eq!(
            posts.lock().unwrap().len(),
            if error.is_none() { 2 } else { 0 }
        );
        vm.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn first_test_cannot_select_another_document_or_claim_admission() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let sample = Arc::new(AgentFolder::new(Some(home.path().into())));
    let pair = sample.open().unwrap();
    let other = created_agent::created(home.path(), "other", &pair.agent, &pair.tests);
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, sample);
    assert!(run
        .start_first_test(vm.clone(), other.document_id())
        .is_err());
    assert!(run
        .start_first_test_with_model(vm.clone(), pair.document_id, "arbitrary-model".into())
        .is_err());
    assert!(!run.is_active());
    assert!(posts.lock().unwrap().is_empty());
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn documents_keep_results_and_share_admission_not_hash_ownership() {
    use bridge_engine::agent_folder::DocumentId;
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let sample = Arc::new(AgentFolder::new(Some(home.path().into())));
    let pair = sample.open().unwrap();
    let agent = format!("{}\nAlpha-bound instructions.\n", pair.agent);
    let a = created_agent::created(home.path(), "alpha", &agent, &pair.tests);
    let b = created_agent::created(home.path(), "beta", &agent, &pair.tests);
    let aid = a.document_id();
    let bid = b.document_id();
    let history_root = Some(home.path().join("local-data"));
    let run = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        sample.clone(),
        history_root.clone(),
    );
    assert_eq!(run.start_document(vm.clone(), a).state, RunState::Running);
    let foreign = run.start_document(vm.clone(), b);
    assert_eq!(foreign.document_id, bid);
    assert!(!foreign.can_run);
    assert_eq!(foreign.busy.unwrap().document_id, aid);
    assert!(foreign.report.is_none());
    assert!(foreign.acknowledged.is_empty());
    for _ in 0..500 {
        if run.status_document(&vm, aid).state != RunState::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let done = run.status_document(&vm, aid);
    assert_eq!(
        done.report.unwrap().verdict,
        bridge_engine::engine::run::Verdict::Passed
    );
    assert_eq!(done.acknowledged.len(), 2);
    assert_eq!(posts.lock().unwrap().len(), 2);
    assert!(posts.lock().unwrap()[0]["spec"]["systemPrompt"]["inline"]
        .as_str()
        .unwrap()
        .contains("Alpha-bound instructions."));
    assert!(run.status_document(&vm, bid).report.is_none());
    assert!(run.status(&vm).report.is_none());
    assert_eq!(run.status(&vm).document_id, DocumentId::SAMPLE);
    let observation = run.overview_observation().unwrap();
    assert_eq!(observation.document_id, aid);
    assert_eq!(observation.state, RunState::Complete);
    assert_eq!(
        observation.source,
        run.status_document(&vm, aid).source.unwrap()
    );
    assert_eq!(observation.agent_uid, done.acknowledged[0].uid);
    // One coherent observation globally, not a document-keyed history.
    let b = owner_folder_for_test(home.path(), "beta");
    assert_eq!(run.start_document(vm.clone(), b).state, RunState::Running);
    assert!(run.is_active());
    for _ in 0..500 {
        if !run.is_active() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(run.overview_observation().unwrap().document_id, bid);
    let fresh = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        sample,
        history_root,
    );
    let history = fresh.local_results().unwrap();
    assert_eq!(history.results.len(), 2);
    assert_eq!(history.run_agents.len(), 2);
    assert!(history.results.iter().all(|r| r
        .report
        .as_ref()
        .is_some_and(|s| s.passed == 1 && s.total == 1)));
    let agents = bridge_engine::agents::Agents::new(
        Some(home.path().into()),
        Some(home.path().join("fixture-data")),
    );
    let files = bridge_engine::overview::saved_files_with_history(&agents, &fresh);
    assert_eq!(files.history_state, "complete");
    assert_eq!(files.run_agents.len(), 2);
    assert_eq!(
        files
            .files
            .iter()
            .filter(|f| f.local_result.is_some())
            .count(),
        2
    );
    assert!(fresh.status_document(&vm, aid).report.is_none());
    assert!(fresh.overview_observation().is_none());
    #[cfg(not(target_os = "macos"))]
    {
        let live = bridge_engine::overview::live_agents(vm.clone(), run.clone()).await;
        assert_eq!(live.state, "complete");
        assert_eq!(live.agents.len(), 2); // Every retained Agent, not just latest Run.
        assert_eq!(live.observation.unwrap().document_id, bid);
    }
    vm.shutdown().await.unwrap();
}

fn owner_folder_for_test(home: &std::path::Path, name: &str) -> Arc<AgentFolder> {
    let agents =
        bridge_engine::agents::Agents::new(Some(home.into()), Some(home.join("fixture-data")));
    let id = agents
        .list()
        .unwrap()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap()
        .id;
    agents.get(id).unwrap()
}

#[tokio::test]
async fn session_failure_after_report_updates_only_current_observation_to_unknown() {
    let (vm, _, _) = fixture_with_outcome(false, false, true).await;
    let home = tempfile::tempdir().unwrap();
    let history_root = Some(home.path().join("local-data"));
    let run = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        Arc::new(AgentFolder::new(Some(home.path().into()))),
        history_root.clone(),
    );
    run.start(vm.clone());
    assert_eq!(finished(&run, &vm).await.state, RunState::Unknown);
    let observed = run.overview_observation().unwrap();
    assert_eq!(observed.state, RunState::Unknown);
    assert!(observed.report.is_none());
    let saved = bridge_engine::local_results::LocalResults::new(history_root)
        .list()
        .unwrap();
    assert_eq!(saved.results[0].state, RunState::Unknown);
    assert!(saved.results[0].report.is_none());
    assert_eq!(saved.run_agents.len(), 1);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn sweep_removes_only_superseded_agents_and_keeps_reports_even_when_cleanup_is_busy() {
    for removed in [false, true] {
        let (vm, posts, _) = fixture_with_cleanup(false, false, false, Some(removed)).await;
        let home = tempfile::tempdir().unwrap();
        let sample = Arc::new(AgentFolder::new(Some(home.path().into())));
        let pair = sample.open().unwrap();
        let other = created_agent::created(home.path(), "other", &pair.agent, &pair.tests);
        let run = ExampleRun::with_history_root(
            tokio::runtime::Handle::current(),
            true,
            sample.clone(),
            Some(home.path().join("data")),
        );
        run.start(vm.clone());
        let first = finished(&run, &vm).await;
        let old_uid = first.acknowledged[0].uid.clone();
        assert_eq!(
            first.report.unwrap().cases[0].answer.as_deref(),
            Some(ANSWER)
        );
        run.start_document(vm.clone(), other.clone());
        for _ in 0..500 {
            if !run.is_active() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(!run.is_active(), "other document's run must finish");
        run.start(vm.clone());
        let latest = finished(&run, &vm).await;
        assert_eq!(latest.state, RunState::Complete);
        assert_eq!(
            latest.report.unwrap().cases[0].answer.as_deref(),
            Some(ANSWER)
        );
        assert_eq!(
            run.status_document(&vm, other.document_id())
                .report
                .unwrap()
                .cases[0]
                .answer
                .as_deref(),
            Some(ANSWER)
        );
        let writes = posts.lock().unwrap().clone();
        let cleanups: Vec<_> = writes
            .iter()
            .filter_map(|v| v["cleanupUid"].as_str())
            .collect();
        assert_eq!(cleanups, [old_uid.as_str()]);
        let history = run.local_results().unwrap();
        assert_eq!(history.results.len(), 2);
        assert_eq!(history.run_agents.len(), if removed { 2 } else { 3 });
        assert!(history
            .results
            .iter()
            .all(|r| r.report.as_ref().unwrap().passed == 1));
        assert!(vm
            .with_client(|_, client| client.model_status())
            .await
            .unwrap()
            .is_ok());
        vm.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn blocked_cleanup_candidates_do_not_starve_later_recorded_agents() {
    let (vm, posts, _) = fixture_with_cleanup(false, false, false, Some(false)).await;
    let home = tempfile::tempdir().unwrap();
    let root = Some(home.path().join("data"));
    let installation = vm.with_client(|id, _| id).await.unwrap();
    let history = bridge_engine::local_results::LocalResults::new(root.clone());
    for index in 0..10 {
        history.record(serde_json::from_value(json!({
            "installation": installation, "documentId": bridge_engine::agent_folder::DocumentId::SAMPLE,
            "source": SnapshotId::of(b"a", b"b"), "namespace": "orka-system",
            "agentName": "display-only", "agentUid": format!("queued-{index}"),
            "observedAt": 1, "state": "unknown", "report": null
        })).unwrap()).unwrap();
    }
    let run = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        Arc::new(AgentFolder::new(Some(home.path().into()))),
        root,
    );
    run.start(vm.clone());
    assert_eq!(finished(&run, &vm).await.state, RunState::Complete);
    assert!(!posts
        .lock()
        .unwrap()
        .iter()
        .any(|v| v["cleanupUid"] == "queued-8"));
    run.start(vm.clone());
    assert_eq!(finished(&run, &vm).await.state, RunState::Complete);
    assert!(posts
        .lock()
        .unwrap()
        .iter()
        .any(|v| v["cleanupUid"] == "queued-8"));
    vm.shutdown().await.unwrap();
}

const ANSWER: &str = "cf699c56340dc775230fde4ef84237d27563ea6ef52164c7d078072b586c3918";
struct Channel {
    input: std::io::Cursor<Vec<u8>>,
    objects: HashMap<String, Value>,
    output: Vec<u8>,
    missing_tools: bool,
    posts: Arc<Mutex<Vec<Value>>>,
    lost_task: bool,
    cancel: Option<Arc<cancellation::Script>>,
    cleanup_reply: Option<bool>,
    model: &'static str,
}
impl Write for Channel {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Frame::Request(r) = serde_json::from_slice(bytes)? else {
            panic!("request")
        };
        let result = match r.body {
            RequestBody::ModelStatus {} => {
                if let Some(s) = &self.cancel {
                    if let Some(hook) = s.model_hook.lock().unwrap().take() {
                        hook();
                    }
                }
                ResultBody::ModelState(ModelState {
                    retained_cleanup: true,
                    diagnostic: None,
                    phase: ModelPhase::Ready,
                    credential: CredentialState {
                        profile: if self.model == "gpt-5.6-terra" {
                            CREDENTIAL_PROFILE
                        } else {
                            MODEL_API_KEY_PROFILE
                        },
                        generation: Some(r.boot_nonce),
                        installed: true,
                    },
                    connection: Some(ModelConnection {
                        id: r.boot_nonce,
                        generation: r.boot_nonce,
                        selection: ModelSelection {
                            vendor: match self.model {
                                "gpt-4.1" => Some(ModelVendor::Openai),
                                "claude-sonnet-5-5" => Some(ModelVendor::Anthropic),
                                _ => None,
                            },
                            provider: if self.model == "gpt-5.6-terra" {
                                "copilot"
                            } else {
                                "key"
                            }
                            .into(),
                            model: self.model.into(),
                        },
                    }),
                })
            }
            RequestBody::Execute {
                operation,
                action: EngineActionWire::Orka { request },
                ..
            } => {
                let mut status = 200;
                if let Some(s) = &self.cancel {
                    s.trace
                        .lock()
                        .unwrap()
                        .push((request.path.clone(), request.body.clone()));
                }
                let body = if request.path.ends_with("/cancel") {
                    let s = self.cancel.as_ref().unwrap();
                    if s.fail_cancel.load(Ordering::SeqCst) {
                        return Err(io::ErrorKind::BrokenPipe.into());
                    }
                    status = if s.deny_first.swap(false, Ordering::SeqCst) {
                        403
                    } else {
                        s.status.load(Ordering::SeqCst)
                    };
                    if let Some(phase) = s.cancel_phase.lock().unwrap().take() {
                        *s.phase.lock().unwrap() = phase;
                    }
                    if s.replace_on_cancel.load(Ordering::SeqCst) {
                        s.replaced.store(true, Ordering::SeqCst);
                    }
                    if status == 200 {
                        *s.phase.lock().unwrap() = "Cancelled".into();
                    }
                    if let Some(hook) = s.cancel_hook.lock().unwrap().take() {
                        hook();
                    }
                    json!({})
                } else if let Some(body) = request.body {
                    self.posts.lock().unwrap().push(body.clone());
                    if self.lost_task && request.path == "tasks" {
                        return Err(io::ErrorKind::BrokenPipe.into());
                    }
                    status = 201;
                    let name = body["name"].as_str().unwrap();
                    let mut object = json!({"apiVersion":"core.orka.ai/v1alpha1","kind":if request.path=="agents" {"Agent"}else{"Task"},"metadata":{"name":name,"namespace":"orka-system","uid":format!("uid-{name}"),"generation":1},"spec":body["spec"],"status":{"phase":"Succeeded"}});
                    if request.path == "tasks" {
                        object["spec"]["timeout"] = json!("2m0s");
                        object["spec"]["retryPolicy"]
                            .as_object_mut()
                            .unwrap()
                            .remove("maxRetries");
                    }
                    self.objects
                        .insert(format!("{}/{name}", request.path), object.clone());
                    if request.path == "tasks" {
                        if let Some(s) = &self.cancel {
                            object["status"]["phase"] = json!(*s.phase.lock().unwrap());
                            if let Some(hook) = s.hook.lock().unwrap().take() {
                                hook();
                            }
                            if s.lose_create.load(Ordering::SeqCst) {
                                return Err(io::ErrorKind::BrokenPipe.into());
                            }
                        }
                    }
                    object
                } else if self.cancel.as_ref().is_some_and(|s| {
                    s.evidence_missing.load(Ordering::SeqCst)
                        && (request.path.ends_with("/result") || request.path.ends_with("/events"))
                        || s.events_missing.load(Ordering::SeqCst)
                            && request.path.ends_with("/events")
                }) {
                    status = 404;
                    Value::Null
                } else if request.path.ends_with("/result") {
                    json!({"result":ANSWER})
                } else if request.path.ends_with("/events") {
                    let name = request.path.split('/').nth(1).unwrap();
                    let agent =
                        self.objects[&format!("tasks/{name}")]["spec"]["agentRef"]["name"].clone();
                    let after = request.after.unwrap();
                    let events = if self.missing_tools {
                        vec![]
                    } else {
                        vec![
                            json!({"namespace":"orka-system","streamType":"task","streamID":name,"seq":after+1,"type":"ToolCallCompleted","taskName":name,"agentName":agent,"toolName":"web_fetch","toolCallID":"call-1"}),
                        ]
                    };
                    json!({"namespace":"orka-system","streamType":"task","streamID":name,"afterSeq":after,"latestSeq":if self.missing_tools{after}else{after+1},"events":events})
                } else {
                    let mut object = self.objects[&request.path].clone();
                    if request.path.starts_with("agents/") {
                        if let Some(s) = &self.cancel {
                            if s.trace
                                .lock()
                                .unwrap()
                                .iter()
                                .any(|(path, _)| path.ends_with("/result"))
                            {
                                if let Some(hook) = s.evidence_hook.lock().unwrap().take() {
                                    hook();
                                }
                            }
                        }
                    }
                    if request.path.starts_with("tasks/") {
                        if let Some(s) = &self.cancel {
                            if s.read_failed.load(Ordering::SeqCst)
                                || s.bad_task
                                    .lock()
                                    .unwrap()
                                    .as_ref()
                                    .is_some_and(|name| request.path == format!("tasks/{name}"))
                            {
                                status = 404;
                            }
                            object["status"]["phase"] = if s
                                .completed_task
                                .lock()
                                .unwrap()
                                .as_ref()
                                .is_some_and(|name| request.path == format!("tasks/{name}"))
                            {
                                json!("Succeeded")
                            } else {
                                json!(*s.phase.lock().unwrap())
                            };
                            if s.replaced.load(Ordering::SeqCst) {
                                object["metadata"]["uid"] = json!("replacement");
                            }
                        }
                    }
                    object
                };
                self.output = serde_json::to_vec(&json!({"status":status,"body":body}))?;
                ResultBody::Operation {
                    operation,
                    info: OperationInfo {
                        state: OperationState::Completed,
                        exit: Some(0),
                        bytes: self.output.len(),
                        has_diagnostics: false,
                    },
                }
            }
            RequestBody::Execute {
                operation,
                action: EngineActionWire::RunWorker { name, uid },
                ..
            } => {
                let s = self.cancel.as_ref().unwrap();
                assert_eq!(
                    self.objects[&format!("tasks/{name}")]["metadata"]["uid"],
                    uid
                );
                s.trace
                    .lock()
                    .unwrap()
                    .push((format!("worker/{name}"), None));
                self.output = serde_json::to_vec(&if s.worker_bad.load(Ordering::SeqCst) {
                    json!({"finished":true,"pods":0,"extra":true})
                } else {
                    json!({"finished":matches!(s.phase.lock().unwrap().as_str(), "Succeeded"|"Failed"|"Cancelled"),"pods":s.pods.load(Ordering::SeqCst)})
                })?;
                ResultBody::Operation {
                    operation,
                    info: OperationInfo {
                        state: OperationState::Completed,
                        exit: Some(0),
                        bytes: self.output.len(),
                        has_diagnostics: false,
                    },
                }
            }
            RequestBody::Execute {
                operation,
                action: EngineActionWire::ListAgents { context, namespace },
                snapshot,
            } => {
                assert!(snapshot.is_none());
                assert_eq!(context, format!("kind-bridge-{}", r.boot_nonce.simple()));
                assert_eq!(namespace, "orka-system");
                self.output = serde_json::to_vec(
                    &json!({"apiVersion":"v1","kind":"List","items":self.objects.values().filter(|o| o["kind"] == "Agent").collect::<Vec<_>>()}),
                )?;
                ResultBody::Operation {
                    operation,
                    info: OperationInfo {
                        state: OperationState::Completed,
                        exit: Some(0),
                        bytes: self.output.len(),
                        has_diagnostics: false,
                    },
                }
            }
            RequestBody::Execute {
                operation,
                action: EngineActionWire::CleanupRunAgent { uid },
                snapshot,
            } => {
                assert!(snapshot.is_none());
                let removed = self
                    .cleanup_reply
                    .expect("legacy guest must never be probed");
                self.posts.lock().unwrap().push(json!({"cleanupUid":uid}));
                if removed {
                    self.objects
                        .retain(|_, o| o["kind"] != "Agent" || o["metadata"]["uid"] != uid);
                }
                self.output = serde_json::to_vec(&CleanupRunReply { removed })?;
                ResultBody::Operation {
                    operation,
                    info: OperationInfo {
                        state: OperationState::Completed,
                        exit: Some(0),
                        bytes: self.output.len(),
                        has_diagnostics: false,
                    },
                }
            }
            RequestBody::Status { operation } => ResultBody::Operation {
                operation,
                info: OperationInfo {
                    state: OperationState::Completed,
                    exit: Some(0),
                    bytes: self.output.len(),
                    has_diagnostics: false,
                },
            },
            RequestBody::ReadResult { offset, .. } => ResultBody::Chunk {
                data: encode_chunk(&self.output[offset..]),
            },
            _ => panic!("unexpected request"),
        };
        let mut data = serde_json::to_vec(&Frame::Result(WireResult {
            version: VERSION,
            boot_nonce: r.boot_nonce,
            request_id: r.request_id,
            body: result,
        }))?;
        data.push(b'\n');
        self.input = std::io::Cursor::new(data);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Read for Channel {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.input.read(bytes)
    }
}
struct Session(
    VmClient,
    Arc<AtomicBool>,
    Uuid,
    bool,
    bool,
    Option<Arc<cancellation::Script>>,
);
impl LocalSession for Session {
    fn run_cancel_supported(&self) -> bool {
        self.4
    }
    fn with_client(
        &mut self,
        work: &mut dyn FnMut(Uuid, &mut VmClient) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        work(self.2, &mut self.0)?;
        if let Some(script) = &self.5 {
            if let Some(hook) = script.release_hook.lock().unwrap().take() {
                hook();
            }
        }
        if self.3 {
            Err(RuntimeError::Closed)
        } else {
            Ok(())
        }
    }
    fn execute(
        &mut self,
        _: bridge_engine::engine::types::EngineRequest,
    ) -> bridge_engine::engine::TransportFuture<'_> {
        Box::pin(async {
            Err(bridge_engine::engine::types::EngineError::new(
                bridge_engine::engine::types::ErrorKind::Unavailable,
            ))
        })
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        self.1.store(true, Ordering::SeqCst);
        Ok(())
    }
}
struct Factory(Mutex<Option<Session>>);
impl LocalFactory for Factory {
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        _: Arc<dyn Fn(bridge_engine::local_vm::LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        Box::pin(async {
            Ok(Box::new(self.0.lock().unwrap().take().unwrap()) as Box<dyn LocalSession>)
        })
    }
}
async fn fixture(
    missing_tools: bool,
    lost_task: bool,
) -> (Arc<LocalVm>, Arc<Mutex<Vec<Value>>>, Arc<AtomicBool>) {
    fixture_with_outcome(missing_tools, lost_task, false).await
}
async fn fixture_with_outcome(
    missing_tools: bool,
    lost_task: bool,
    closed_after_work: bool,
) -> (Arc<LocalVm>, Arc<Mutex<Vec<Value>>>, Arc<AtomicBool>) {
    fixture_with_cleanup(missing_tools, lost_task, closed_after_work, None).await
}
async fn fixture_with_cleanup(
    missing_tools: bool,
    lost_task: bool,
    closed_after_work: bool,
    cleanup_reply: Option<bool>,
) -> (Arc<LocalVm>, Arc<Mutex<Vec<Value>>>, Arc<AtomicBool>) {
    fixture_with_model(
        missing_tools,
        lost_task,
        closed_after_work,
        cleanup_reply,
        "gpt-5.6-terra",
    )
    .await
}
async fn fixture_with_model(
    missing_tools: bool,
    lost_task: bool,
    closed_after_work: bool,
    cleanup_reply: Option<bool>,
    model: &'static str,
) -> (Arc<LocalVm>, Arc<Mutex<Vec<Value>>>, Arc<AtomicBool>) {
    let id = Uuid::new_v4();
    let posts = Arc::new(Mutex::new(Vec::new()));
    let stopped = Arc::new(AtomicBool::new(false));
    let client = VmClient::established_with_hello(
        Box::new(Channel {
            input: std::io::Cursor::new(Vec::new()),
            objects: HashMap::new(),
            output: Vec::new(),
            missing_tools,
            posts: posts.clone(),
            lost_task,
            cancel: None,
            cleanup_reply,
            model,
        }),
        id,
        &StageHello {
            version: VERSION,
            stage: Stage::Serve,
            guest_nonce: Uuid::new_v4(),
            accepted_boot_nonce: Some(id),
            run_cleanup: cleanup_reply.is_some(),
        },
    )
    .unwrap();
    let vm = LocalVm::new(Arc::new(Factory(Mutex::new(Some(Session(
        client,
        stopped.clone(),
        id,
        closed_after_work,
        false,
        None,
    ))))));
    vm.start();
    for _ in 0..100 {
        if vm.status() == LocalVmStatus::Ready {
            return (vm, posts, stopped);
        }
        tokio::task::yield_now().await;
    }
    panic!("VM never ready")
}
async fn finished(run: &ExampleRun, vm: &LocalVm) -> bridge_engine::example_run::RunSnapshot {
    for _ in 0..500 {
        let status = run.status(vm);
        if status.state != RunState::Running {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("Run never finished")
}
#[tokio::test]
async fn ipc_waiter_loss_keeps_native_owner() {
    let (vm, posts, stopped) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
    assert_eq!(run.start(vm.clone()).state, RunState::Running);
    run.start(vm.clone()); // duplicate renderer command cannot submit twice
    let result = finished(&run, &vm).await;
    let display = serde_json::to_value(&result).unwrap();
    assert_eq!(display["stage"], "done");
    assert_eq!(display["model"], "gpt-5.6-terra");
    assert!(display["report"]["cases"][0]["question"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    let report = result.report.unwrap();
    assert_eq!(report.verdict, bridge_engine::engine::run::Verdict::Passed);
    assert_eq!(report.cases[0].answer.as_deref(), Some(ANSWER));
    assert_eq!(report.cases[0].completed_tools, ["web_fetch"]);
    assert_eq!(
        report.cases[0]
            .assertions
            .iter()
            .map(|a| a.kind.as_str())
            .collect::<Vec<_>>(),
        ["contains", "toolCalled"]
    );
    assert_eq!(result.acknowledged.len(), 2);
    {
        let writes = posts.lock().unwrap();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[0]["spec"]["model"]["name"], "gpt-5.6-terra");
        assert_eq!(writes[0]["spec"]["providerRef"]["name"], "copilot");
        assert!(!writes[0].to_string().contains(ANSWER));
    }
    vm.shutdown().await.unwrap();
    assert!(stopped.load(Ordering::SeqCst));
}
#[test]
fn blocking_ipc_caller_does_not_need_an_entered_tokio_runtime() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let run = ExampleRun::new(runtime.handle().clone(), true, folder);
    let vm = LocalVm::unavailable();
    assert_eq!(run.start(vm.clone()).state, RunState::Running);
    let result = runtime.block_on(finished(&run, &vm));
    assert_eq!(result.state, RunState::Unknown);
    assert_eq!(result.error, Some("session"));
}
#[tokio::test]
async fn unsupported_image_never_probes_or_retires_the_shared_client() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let run = ExampleRun::new(tokio::runtime::Handle::current(), false, folder);
    assert!(!run.status(&vm).can_run);
    assert_eq!(run.start(vm.clone()).state, RunState::Idle);
    assert!(posts.lock().unwrap().is_empty());
    // The refusal must leave the original channel usable, not discover support by probing.
    assert!(vm
        .with_client(|_, client| client.model_status())
        .await
        .unwrap()
        .is_ok());
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn absent_completed_tools_stays_unknown_with_contains_passed() {
    let (vm, _, _) = fixture(true, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
    run.start(vm.clone());
    let result = finished(&run, &vm).await;
    let report = result.report.unwrap();
    assert_eq!(report.verdict, bridge_engine::engine::run::Verdict::Unknown);
    assert_eq!(
        report.cases[0].assertions[0].verdict,
        bridge_engine::engine::run::Verdict::Passed
    );
    assert_eq!(
        report.cases[0].assertions[1].verdict,
        bridge_engine::engine::run::Verdict::Unknown
    );
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn uncertain_task_write_keeps_agent_and_pending_task_without_retry() {
    let (vm, posts, _) = fixture(false, true).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let history_root = Some(home.path().join("local-data"));
    let run = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        folder,
        history_root.clone(),
    );
    run.start(vm.clone());
    let result = finished(&run, &vm).await;
    assert_eq!(result.state, RunState::Unknown);
    assert_eq!(result.acknowledged.len(), 1);
    let pending = result.pending.unwrap();
    assert_eq!(pending.kind, "Task");
    assert_eq!(posts.lock().unwrap().len(), 2);
    let observation = run.overview_observation().unwrap();
    assert_eq!(observation.agent_uid, result.acknowledged[0].uid);
    assert_eq!(observation.state, RunState::Unknown);
    assert!(observation.report.is_none());
    // A changed-source retry on the retired channel must not rebind the old receipt.
    let folder = AgentFolder::new(Some(home.path().into()));
    let source = folder.open().unwrap();
    let changed = folder
        .save(format!("{}\nRetry new prompt", source.agent))
        .unwrap()
        .source;
    // A second attempt on the retired channel fails preflight without losing remote IDs.
    run.start(vm.clone());
    let refused = finished(&run, &vm).await;
    assert_eq!(refused.error, Some("session"));
    assert_eq!(refused.acknowledged.len(), 1);
    assert_eq!(refused.pending.unwrap().name, pending.name);
    assert_eq!(refused.source.as_ref(), Some(&changed));
    let retained = run.overview_observation().unwrap();
    assert_eq!(retained.agent_uid, observation.agent_uid);
    assert_eq!(retained.source, observation.source);
    assert_ne!(retained.source, changed);
    let saved = bridge_engine::local_results::LocalResults::new(history_root)
        .list()
        .unwrap();
    assert_eq!(saved.results[0].agent_uid, observation.agent_uid);
    assert_eq!(saved.results[0].source, observation.source);
    assert_eq!(saved.results[0].state, RunState::Unknown);
    assert_eq!(saved.run_agents.len(), 1);
    assert!(!run.is_active());
    assert_eq!(posts.lock().unwrap().len(), 2);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_test_uses_owned_pair_vm_transport() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let edited = folder.open().unwrap().agent.replace(
        "Answer from the web page named in the request.",
        "Read the edited prompt on disk.",
    );
    folder.save(edited).unwrap();
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder.clone());
    run.start(vm.clone());
    let result = finished(&run, &vm).await;
    assert_eq!(result.state, RunState::Complete);
    assert!(posts.lock().unwrap()[0]["spec"]
        .to_string()
        .contains("Read the edited prompt on disk."));
    assert_eq!(result.source.as_ref(), Some(&folder.open().unwrap().source));
    // Native reports retain their captured source across renderer reloads. The
    // renderer compares that identity with the newly opened/displayed pair.
    folder.save("another draft after Run".into()).unwrap();
    let reopened = folder.open().unwrap();
    assert_ne!(run.status(&vm).source.as_ref(), Some(&reopened.source));
    vm.shutdown().await.unwrap();
}
#[tokio::test]
async fn malformed_saved_source_refuses_before_any_remote_write() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    folder.save("unfinished raw draft".into()).unwrap();
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
    run.start(vm.clone());
    let result = finished(&run, &vm).await;
    assert_eq!(result.error, Some("source"));
    assert!(posts.lock().unwrap().is_empty());
    assert!(vm
        .with_client(|_, client| client.model_status())
        .await
        .unwrap()
        .is_ok());
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn preparation_refusal_keeps_previous_resource_receipts() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let agent = folder.open().unwrap().agent;
    let history_root = Some(home.path().join("local-data"));
    let run = ExampleRun::with_history_root(
        tokio::runtime::Handle::current(),
        true,
        folder.clone(),
        history_root.clone(),
    );
    run.start(vm.clone());
    let completed = finished(&run, &vm).await;
    assert_eq!(completed.state, RunState::Complete);
    assert_eq!(completed.acknowledged.len(), 2);
    folder
        .save(format!("{agent}{}", "x".repeat(65536)))
        .unwrap();
    run.start(vm.clone());
    let refused = finished(&run, &vm).await;
    assert_eq!(refused.error, Some("submit"));
    let saved = bridge_engine::local_results::LocalResults::new(history_root)
        .list()
        .unwrap();
    assert_eq!(saved.results[0].state, RunState::Complete);
    assert_eq!(saved.results[0].report.as_ref().unwrap().passed, 1);
    assert_eq!(saved.run_agents.len(), 1);
    assert_eq!(
        refused
            .acknowledged
            .iter()
            .map(|r| (&r.kind, &r.name, &r.uid))
            .collect::<Vec<_>>(),
        completed
            .acknowledged
            .iter()
            .map(|r| (&r.kind, &r.name, &r.uid))
            .collect::<Vec<_>>()
    );
    assert!(refused.pending.is_none());
    let observed = run.overview_observation().unwrap();
    assert_eq!(observed.source, completed.source.unwrap());
    assert_eq!(observed.state, RunState::Complete); // Pre-ack failure did not change it.
    assert_eq!(posts.lock().unwrap().len(), 2);
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn vm_transport_refuses_large_mapped_body_before_any_post() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let agent = folder.open().unwrap().agent;
    folder
        .save(format!("{agent}{}", "x".repeat(65536)))
        .unwrap();
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
    run.start(vm.clone());
    let result = finished(&run, &vm).await;
    assert_eq!(result.error, Some("submit"));
    assert!(result.pending.is_none());
    assert!(result.acknowledged.is_empty());
    assert!(posts.lock().unwrap().is_empty());
    assert!(vm
        .with_client(|_, client| client.model_status())
        .await
        .unwrap()
        .is_ok());
    vm.shutdown().await.unwrap();
}

#[tokio::test]
async fn admitted_run_freezes_source_before_busy_vm_refusal() {
    let (vm, posts, _) = fixture(false, false).await;
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let original = folder.open().unwrap().agent;
    let (path, _) = folder.source().unwrap();
    let before = original.replace(
        "Answer from the web page named in the request.",
        "Frozen before client queue.",
    );
    std::fs::write(path.join("web-reader.agent.md"), &before).unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let target = vm.clone();
    let blocked = tokio::spawn(async move {
        target
            .with_client(move |_, _| {
                let _ = entered.send(());
                held.recv().unwrap();
            })
            .await
            .unwrap()
    });
    waiting.await.unwrap();
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder.clone());
    #[cfg(not(target_os = "macos"))]
    {
        let busy = bridge_engine::overview::live_agents(vm.clone(), run.clone()).await;
        assert_eq!(busy.state, "busy");
        assert!(!busy.run_active); // A held session is not necessarily a Run.
    }
    run.start(vm.clone());
    #[cfg(not(target_os = "macos"))]
    {
        let busy = bridge_engine::overview::live_agents(vm.clone(), run.clone()).await;
        assert_eq!(busy.state, "busy");
        assert!(busy.run_active); // Defers before attempting the occupied session.
    }
    // capture reconciles dirty disk to HEAD, proving it ran while the VM client is held.
    let mut frozen = false;
    for _ in 0..500 {
        let repo = git2::Repository::open(&path).unwrap();
        let tree = repo
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .tree()
            .unwrap();
        let id = tree.get_name("web-reader.agent.md").unwrap().id();
        if repo.find_blob(id).unwrap().content() == before.as_bytes() {
            frozen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(frozen, "source capture waited for VM access");
    folder
        .save(original.replace(
            "Answer from the web page named in the request.",
            "Later edit after admission.",
        ))
        .unwrap();
    // Keep the client held until Run has observed busy; releasing first races
    // admission and can legitimately let the captured pair run successfully.
    let result = finished(&run, &vm).await;
    release.send(()).unwrap();
    blocked.await.unwrap();
    assert_eq!(result.state, RunState::Unknown);
    assert_eq!(result.error, Some("session"));
    assert!(posts.lock().unwrap().is_empty());
    vm.shutdown().await.unwrap();
}
