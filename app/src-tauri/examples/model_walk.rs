// Native UI fixture only. Embeds the real frontend, but never starts a VM,
// reads a credential, or runs GitHub CLI. Not linked into the Bridge executable.
#[cfg(not(all(target_os = "macos", feature = "custom-protocol")))]
fn main() {}

// Stages are advanced only by explicit UI actions and status reads. The final
// turn waits for Stop so the driver cannot miss that control on a fast runner.
#[cfg(all(target_os = "macos", feature = "custom-protocol"))]
struct ChatFixture {
    view: bridge_engine::chat::ChatSnapshot,
    started: Option<std::time::Instant>,
    fresh: bool,
    wait_for_stop: bool,
    phase_file: Option<std::path::PathBuf>,
}
#[cfg(all(target_os = "macos", feature = "custom-protocol"))]
impl ChatFixture {
    fn new() -> Self {
        Self {
            view: bridge_engine::chat::ChatSnapshot {
                document_id: bridge_engine::agent_folder::DocumentId::SAMPLE,
                state: bridge_engine::chat::ChatState::Ready,
                busy: false,
                can_send: true,
                versions: vec![],
                error: None,
            },
            started: None,
            fresh: true,
            wait_for_stop: false,
            phase_file: std::env::var_os("BRIDGE_CHAT_PHASE").map(std::path::PathBuf::from),
        }
    }
    fn call(
        &mut self,
        command: &str,
        args: &serde_json::Value,
        run_active: bool,
    ) -> Result<bridge_engine::chat::ChatSnapshot, bridge_engine::chat::ChatError> {
        use bridge_engine::chat::{ChatError, ChatState, ChatTurn, ChatVersion, ToolObservation};
        if args["documentId"] != serde_json::json!(self.view.document_id) {
            return Err(ChatError::Source);
        }
        if let Some(started) = self.started {
            let phase = self
                .phase_file
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok());
            if phase.as_deref() == Some("answer")
                || phase.is_none() && started.elapsed().as_millis() >= 1500
            {
                let first = self.view.versions[0].turns.len() == 1;
                let turn = self
                    .view
                    .versions
                    .last_mut()
                    .unwrap()
                    .turns
                    .last_mut()
                    .unwrap();
                if self.view.state == ChatState::RequestingStop {
                    turn.phase = "Stop requested".into();
                    turn.termination_unconfirmed = true;
                } else {
                    turn.phase = "Succeeded".into();
                    turn.answer = Some(if first {
                        "The checksum is ALPHA. <script>fixture</script>".into()
                    } else {
                        "You asked me to remember ALPHA.".into()
                    });
                    turn.context_retained = true;
                }
                self.view.state = ChatState::Ready;
                self.view.busy = false;
                self.view.can_send = true;
                self.started = None;
            } else if matches!(phase.as_deref(), Some("session-null" | "session-answer")) {
                self.view.state = ChatState::WaitingForSession;
                let turn = self
                    .view
                    .versions
                    .last_mut()
                    .unwrap()
                    .turns
                    .last_mut()
                    .unwrap();
                turn.phase = "Succeeded".into();
                turn.context_retained = false;
                turn.answer = (phase.as_deref() == Some("session-answer"))
                    .then(|| "The checksum is ALPHA. <script>fixture</script>".into());
            } else if self.view.state == ChatState::Preparing || phase.is_some() {
                self.view.state = ChatState::Working;
                self.view
                    .versions
                    .last_mut()
                    .unwrap()
                    .turns
                    .last_mut()
                    .unwrap()
                    .phase = if phase.as_deref() == Some("pending") {
                    "Pending"
                } else {
                    "Running"
                }
                .into();
            }
        }
        match command {
            "agent_chat_send" => {
                if self.view.busy || run_active {
                    return Err(ChatError::Busy);
                }
                let message = args["message"]
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or(ChatError::InvalidMessage)?;
                self.wait_for_stop = self.fresh && !self.view.versions.is_empty();
                if self.fresh {
                    self.view.versions.push(ChatVersion::default());
                    self.fresh = false;
                }
                self.view.versions.last_mut().unwrap().turns.push(ChatTurn {
                    message: message.into(),
                    phase: "Running".into(),
                    tools: vec![ToolObservation {
                        name: "web_fetch".into(),
                        call_id: "fixture-tool".into(),
                        phase: "running".into(),
                    }],
                    ..ChatTurn::default()
                });
                let held = self.phase_file.as_ref().is_some_and(|path| path.exists());
                if held {
                    self.view
                        .versions
                        .last_mut()
                        .unwrap()
                        .turns
                        .last_mut()
                        .unwrap()
                        .phase = "Pending".into();
                }
                self.view.state = if self.wait_for_stop || held {
                    ChatState::Working
                } else {
                    ChatState::Preparing
                };
                self.view.busy = true;
                self.view.can_send = false;
                self.started = (!self.wait_for_stop).then(std::time::Instant::now);
            }
            "agent_chat_stop" => {
                if !self.view.busy {
                    return Err(ChatError::Busy);
                }
                self.view.state = ChatState::RequestingStop;
                self.started = Some(std::time::Instant::now());
            }
            "agent_chat_new" => {
                if self.view.busy {
                    return Err(ChatError::Busy);
                }
                self.fresh = true;
            }
            "agent_chat_status" | "agent_chat_reconnect" => {}
            _ => unreachable!("bounded fixture commands"),
        }
        Ok(self.view.clone())
    }
}

// Returning-workspace transitions are action-owned; the existing selector only
// settles an in-flight transition (or injects an unreadable status).
#[cfg(all(target_os = "macos", feature = "custom-protocol"))]
struct WorkspaceFixture {
    selected: bool,
    phase: &'static str,
    returning: bool,
    stop_failed: bool,
}
#[cfg(all(target_os = "macos", feature = "custom-protocol"))]
impl WorkspaceFixture {
    fn status(&self) -> serde_json::Value {
        let mut status = serde_json::json!({"selected":self.selected.then_some("localVm"), "localVm":self.phase,
            "model":if self.phase == "ready" {"available"} else {"locked"}, "modelSupported":true,
            "workspaceProgress":(self.phase == "starting").then(|| serde_json::json!({
                "phase":"starting", "bytes":null, "totalBytes":null, "elapsedMs":18000, "remainingMs":null}))});
        if self.phase == "failed" {
            status["workspaceFailure"] = if self.stop_failed {
                serde_json::json!({"operation":"stop", "cause":"stopUnconfirmed", "codes":[]})
            } else {
                serde_json::json!({"operation":"start", "cause":"permission", "codes":[13]})
            };
        }
        status
    }
}

// Synthetic UUIDs only, in the native error's safe recovery shape.
#[cfg(all(target_os = "macos", feature = "custom-protocol"))]
fn fixture_recovery(stage: Option<&str>) -> serde_json::Value {
    let mut recovery = serde_json::json!({
        "installation":"10000000-0000-4000-8000-000000000001",
        "savedInstallation":"20000000-0000-4000-8000-000000000002",
        "credentialSlot":"30000000-0000-4000-8000-000000000003"
    });
    if let Some(stage) = stage {
        recovery["resetStage"] = stage.into();
    }
    recovery
}

#[cfg(all(target_os = "macos", feature = "custom-protocol"))]
fn main() {
    use serde_json::json;
    use tauri::Manager;
    let removed_once = std::sync::atomic::AtomicBool::new(false);
    let running = std::sync::atomic::AtomicBool::new(false);
    let cancelling = std::sync::atomic::AtomicBool::new(false);
    let chat = std::sync::Mutex::new(ChatFixture::new());
    let workspace = std::sync::Arc::new(std::sync::Mutex::new(WorkspaceFixture {
        selected: true,
        phase: "ready",
        returning: false,
        stop_failed: false,
    }));
    let connected = std::sync::atomic::AtomicBool::new(false);
    let key_connected = std::sync::atomic::AtomicBool::new(false);
    let key_attempt = std::sync::atomic::AtomicUsize::new(0);
    let key_check_release =
        std::env::var_os("BRIDGE_MODEL_CHECK_RELEASE").map(std::path::PathBuf::from);
    // A bounded state selector for setup renderer captures, never part of the app.
    let setup_progress = std::env::var_os("BRIDGE_SETUP_PROGRESS").map(std::path::PathBuf::from);
    let catalog_release = std::env::var_os("BRIDGE_CATALOG_RELEASE").map(std::path::PathBuf::from);
    // The driver's selector for the automatic first-test outcome: absent is the fixed failed report.
    let first_outcome = std::env::var_os("BRIDGE_FIRST_TEST_OUTCOME").map(std::path::PathBuf::from);
    let outcome = move || {
        first_outcome
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .filter(|value| ["passed", "unknown"].contains(&value.as_str()))
    };
    let key_removed_once = std::sync::atomic::AtomicBool::new(false);
    // A synthetic saved record from an earlier installation. Only the driver's
    // selector creates it; reset clears this flag, never a real store or file.
    let mismatched = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let first_test_pending = std::sync::atomic::AtomicBool::new(false);
    let cluster_selected = std::sync::atomic::AtomicBool::new(false);
    let target = json!({"id":"42345678-1234-4234-8234-123456789abc",
        "context_id":"52345678-1234-4234-8234-123456789abc", "context_name":"team-dev",
        "service_name":"orka", "service_namespace":"control", "watched_namespace":"agents", "image":"orka:fixture"});
    let providers =
        json!([{"id":"62345678-1234-4234-8234-123456789abc", "name":"team-model", "ready":true}]);
    let original = include_str!("../example-agent/web-reader.agent.md");
    let agent = std::sync::Mutex::new(original.to_owned());
    let tests = "---\nschemaVersion: 1\n---\n```yaml agent-test\nid: reads-page\ntitle: Copies the checksum from the page\ninput: What did the page say?\nassertions:\n - id: text\n   type: contains\n   value: checksum\n```\n```yaml agent-test\nid: has-a-date\ntitle: Says which date is on the page\ninput: Which date is shown on the page?\nassertions:\n - id: date\n   type: contains\n   value: '2099-12-31'\n```\n";
    let source = bridge_runtime_protocol::SnapshotId::of(original.as_bytes(), tests.as_bytes());
    // Stable Overview revisions, independent of web-reader's editable walk state.
    let edited_source =
        bridge_runtime_protocol::SnapshotId::of(b"edited fixture instructions", tests.as_bytes());
    let run_source = std::sync::Mutex::new(source.clone());
    let run_attempt = std::sync::Mutex::new(None::<String>);
    tauri::Builder::default()
        .setup(|app| {
            app.get_webview_window("main")
                .unwrap()
                .set_title("Bridge model fixture (synthetic IPC, no VM)")?;
            Ok(())
        })
        .invoke_handler(move |invoke| {
            let capture_phase = setup_progress.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
            let account_scene = matches!(capture_phase.as_deref(), Some("cluster-account-picker" | "cluster-account-forbidden"));
            let reply = match invoke.message.command() {
                "overview_files" => {
                    let content = agent.lock().unwrap();
                    let summary = bridge_engine::engine::run::ParsedPair::parse(content.as_bytes(), tests.as_bytes())
                        .ok()
                        .map(|pair| json!({"name":pair.name(), "description":pair.description(), "model":pair.model(), "tools":pair.tools()}));
                    let reason = summary.is_none().then_some("The saved declarations are unsupported or invalid.");
                    let observation = |document: &str, name: &str, verdict: &str, passed: usize| json!({
                        "installation":"12345678-1234-4234-8234-123456789abc", "documentId":document,
                        "source":source, "namespace":"orka-system", "agentName":name, "agentUid":format!("run-{name}"),
                        "observedAt":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64, "state":"complete", "report":{"verdict":verdict,"passed":passed,"total":2}
                    });
                    json!({"state":"complete", "runActive":running.load(std::sync::atomic::Ordering::SeqCst),
                        "historyState":"complete", "runAgents":[], "files":[{
                        "documentId":"9b2b93c4-375c-4815-9717-39852a96e411", "name":"web-reader", "state":"readable",
                        "source":bridge_runtime_protocol::SnapshotId::of(content.as_bytes(), tests.as_bytes()),
                        "summary":summary, "reason":reason,
                        "localResult":observation("9b2b93c4-375c-4815-9717-39852a96e411", "web-reader", "failed", 1)
                    }, {
                        "documentId":"22345678-1234-4234-8234-123456789abc", "name":"release-notes", "state":"readable",
                        "source":source, "summary":{"name":"release-notes","description":"Summarises changes for a release.","model":"gpt-5.6-terra","tools":["file_read"]},
                        "reason":null,"localResult":observation("22345678-1234-4234-8234-123456789abc", "release-notes", "passed", 2)
                    }, {
                        "documentId":"32345678-1234-4234-8234-123456789abc", "name":"pr-summary", "state":"readable",
                        "source":edited_source,"summary":{"name":"pr-summary","description":"Explains a pull request before review.","model":"gpt-5.6-terra","tools":[]},
                        "reason":null,"localResult":null
                    }, {
                        "documentId":"72345678-1234-4234-8234-123456789abc", "name":"draft-helper", "state":"readable",
                        "source":source, "draft":true, "summary":null,
                        "reason":"Add instructions and a test to get started.", "localResult":null
                    }]})
                }
                // Match the native Mac image's unavailable-list contract, not a live VM.
                "overview_agents" => json!({"state":"unavailable", "installation":null, "agents":[],
                    "observedAt":null, "observation":null, "runActive":running.load(std::sync::atomic::Ordering::SeqCst), "reason":"image"}),
                "agent_open" | "agent_save" => {
                    let mut content = agent.lock().unwrap();
                    if invoke.message.command() == "agent_save" {
                        if let tauri::ipc::InvokeBody::Json(args) = invoke.message.payload() {
                            if let Some(value) = args["content"].as_str() {
                                *content = value.into();
                            }
                        }
                    }
                    json!({"documentId":"9b2b93c4-375c-4815-9717-39852a96e411", "name":"web-reader", "agent":*content, "tests":tests,
                        "source":bridge_runtime_protocol::SnapshotId::of(content.as_bytes(), tests.as_bytes())})
                }
                "overview_places" => {
                    let recorded = |id: &str, document: &str, name: &str, uid: &str, revision: &bridge_runtime_protocol::SnapshotId, verdict: &str, passed: usize| json!({
                        "name":name, "namespace":"agents", "uid":uid, "agentType":"Declarative", "model":"team-model", "tools":["file_read"],
                        "record":{"id":id, "documentId":document, "detailState":"saved", "source":revision,
                            "targetDisplay":"team-dev", "targetUids":{"serviceNamespace":"control", "serviceUid":"fixture-service",
                                "deploymentNamespace":"control", "deploymentUid":"fixture-deployment", "watchedNamespace":"agents"},
                            "provider":"team-model", "plannedAgentName":name, "agentUid":uid,
                            "verdict":verdict, "time":1730000000_u64, "report":{"verdict":verdict,"passed":passed,"total":2}}
                    });
                    json!({"historyState":"complete","unobserved":[],"places":[{
                    "key":"team-observed","context":"team-dev","service":"control/orka","namespace":"agents",
                    "stale":false,"observedAt":1234567890000_u64,"agents":[
                        // Each recorded revision equals the OTHER file: only document identity can link correctly.
                        recorded("82345678-1234-4234-8234-123456789abc", "22345678-1234-4234-8234-123456789abc", "release-notes", "release-uid", &edited_source, "passed", 2),
                        recorded("92345678-1234-4234-8234-123456789abc", "32345678-1234-4234-8234-123456789abc", "pr-summary", "pr-uid", &source, "failed", 1), {
                        "name":"review-helper","namespace":"agents","uid":"external-agent",
                        "agentType":"runtime","model":"team-model","tools":["web_fetch"],"record":null
                    }]
                },{
                    "key":"team-remembered","context":"team-west","service":"control/orka","namespace":"agents",
                    "stale":true,"observedAt":null,"agents":[]
                }]})
                },
                "agents_list" => json!([{"id":bridge_engine::agent_folder::DocumentId::SAMPLE,
                    "name":"web-reader", "path":null, "available":true}]),
                "agent_creation_root" => json!("/Users/you/Orka/agents"),
                "agent_selected" => json!(bridge_engine::agent_folder::DocumentId::SAMPLE),
                "agent_select" => {
                    if !matches!(invoke.message.payload(), tauri::ipc::InvokeBody::Json(args)
                        if args["documentId"] == json!(bridge_engine::agent_folder::DocumentId::SAMPLE))
                    {
                        invoke.resolver.reject("Unknown fixture agent");
                        return true;
                    }
                    json!(null)
                }
                // Catalog choices are synthetic and never execute a tool or model.
                "workspace_catalog" => {
                    assert_ne!(capture_phase.as_deref(), Some("designer-no-destination"), "No destination must not request a catalog");
                    if catalog_release.as_ref().is_some_and(|path| !path.is_file()) {
                        invoke.resolver.reject("catalogBusy");
                        return true;
                    }
                    json!({
                    "defaultModel":"gpt-5.6-terra",
                    "models":{"state":"ready", "items":[{"id":"gpt-5.6-terra"}]},
                    "tools":{"state":"ready", "items":[
                        {"kind":"builtin", "name":"web_fetch", "description":"Read a web page"},
                        {"kind":"builtin", "name":"file_read", "description":"Read a file"},
                        {"kind":"builtin", "name":"request_approval", "description":"Ask a person to approve an action."},
                        {"kind":"builtin", "name":"reply_in_conversation", "description":"Send a reply in the current conversation."}
                    ]}
                })
                },
                // Only a synthetic destination for the failed-local-test popover walk.
                // No Send command is implemented: Fix it first and Escape must not submit.
                "cluster_places" => json!({"places":[{
                    "key":"team-observed", "context":"team-dev", "service":"control/orka", "namespace":"agents",
                    "stale":false, "observedAt":1234567890000_u64, "agents":[]
                }], "selectedKey":cluster_selected.load(std::sync::atomic::Ordering::SeqCst).then_some("team-observed"), "persistenceState":"complete"}),
                "cluster_reconnect" => target.clone(),
                "cluster_use" => {
                    if account_scene {
                        assert!(matches!(invoke.message.payload(), tauri::ipc::InvokeBody::Json(args)
                            if args["targetId"] == target["id"] && args["saName"].is_null()), "Picker actions must not Use a selected account");
                        json!({"status":"needServiceAccount", "providers":[], "failure":null})
                    } else {
                        cluster_selected.store(true, std::sync::atomic::Ordering::SeqCst);
                        json!({"status":"connected", "providers":providers, "failure":null})
                    }
                }
                "cluster_service_accounts" | "cluster_service_account_preview" => {
                    assert!(account_scene, "Account commands require the named synthetic scene");
                    assert!(matches!(invoke.message.payload(), tauri::ipc::InvokeBody::Json(args)
                        if args["targetId"] == target["id"]), "Wrong account target");
                    if invoke.message.command() == "cluster_service_accounts" {
                        if capture_phase.as_deref() == Some("cluster-account-forbidden") {
                            invoke.resolver.reject(json!({"check":"serviceAccount", "result":"permissionDenied"}));
                            return true;
                        }
                        json!({"names":["team-runner", "task-reader"], "truncated":false})
                    } else {
                        assert!(matches!(invoke.message.payload(), tauri::ipc::InvokeBody::Json(args)
                            if args["saName"] == "team-runner"), "Only the selected account may be previewed");
                        let checks: Vec<_> = [("agents", vec!["create", "get", "list"]), ("tasks", vec!["create", "get", "list"]),
                            ("providers", vec!["get", "list"]), ("tools", vec!["get", "list"])]
                            .into_iter().flat_map(|(resource, verbs)| verbs.into_iter().map(move |verb|
                                json!({"resource":resource, "verb":verb, "status":"allowed"}))).collect();
                        json!({"status":"allowed", "checks":checks})
                    }
                }
                "send_preflight" => {
                    let content = agent.lock().unwrap();
                    json!({"documentId":bridge_engine::agent_folder::DocumentId::SAMPLE,
                        "targetId":target["id"], "providerId":providers[0]["id"],
                        "source":bridge_runtime_protocol::SnapshotId::of(content.as_bytes(), tests.as_bytes()),
                        "model":"gpt-5.6-terra", "canSend":true, "action":"create", "canSeparate":false,
                        "rows":[{"label":"Destination", "status":"allowed", "message":"Synthetic destination ready"}]})
                }
                "cluster_contexts" => json!([
                    {"id":"52345678-1234-4234-8234-123456789abc", "name":"team-dev", "current":true},
                    {"id":"72345678-1234-4234-8234-123456789abc", "name":"team-west", "current":false}
                ]),
                "cluster_discover" => {
                    if account_scene {
                        let mut discovered = target.clone();
                        if let tauri::ipc::InvokeBody::Json(args) = invoke.message.payload() {
                            discovered["context_id"] = args["contextId"].clone();
                            discovered["context_name"] = json!(if args["contextId"] == "72345678-1234-4234-8234-123456789abc" {"team-west"} else {"team-dev"});
                        }
                        invoke.resolver.resolve(json!([discovered]));
                        return true;
                    }
                    // A UI-action-only synthetic discovery held until the driver captures Checking.
                    let selector = setup_progress.clone();
                    let resolver = invoke.resolver;
                    tauri::async_runtime::spawn(async move {
                        while selector.as_ref().and_then(|path| std::fs::read_to_string(path).ok()).as_deref() == Some("cluster-checking") {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        resolver.reject(json!({"check":"clusterApi", "result":"connectionFailed"}));
                    });
                    return true;
                }
                "send_history" => json!([]),
                "send_status" => json!({"documentId":"9b2b93c4-375c-4815-9717-39852a96e411", "view":null, "busy":null}),
                command @ ("agent_chat_status" | "agent_chat_send" | "agent_chat_reconnect" | "agent_chat_stop" | "agent_chat_new") => {
                    let tauri::ipc::InvokeBody::Json(args) = invoke.message.payload() else {
                        invoke.resolver.reject(bridge_engine::chat::ChatError::Source);
                        return true;
                    };
                    match chat.lock().unwrap().call(command, args, running.load(std::sync::atomic::Ordering::SeqCst)) {
                        Ok(view) => json!(view),
                        Err(error) => { invoke.resolver.reject(error); return true; }
                    }
                }
                "agent_run_status" | "agent_run" | "agent_run_cancel" => {
                    if invoke.message.command() == "agent_run_cancel" { cancelling.store(true, std::sync::atomic::Ordering::SeqCst); }
                    if invoke.message.command() == "agent_run" {
                        *run_attempt.lock().unwrap() = match invoke.message.payload() {
                            tauri::ipc::InvokeBody::Json(args) => args["attemptId"].as_str().map(str::to_owned),
                            _ => None,
                        };
                        *run_source.lock().unwrap() = bridge_runtime_protocol::SnapshotId::of(agent.lock().unwrap().as_bytes(), tests.as_bytes());
                        cancelling.store(false, std::sync::atomic::Ordering::SeqCst);
                        // The automatic first test finishes with the fixed failed report.
                        // A later manual Run stays active for the existing result captures.
                        let automatic = matches!(invoke.message.payload(), tauri::ipc::InvokeBody::Json(args) if args["bundledExample"].as_bool() == Some(true));
                        first_test_pending.store(false, std::sync::atomic::Ordering::SeqCst);
                        running.store(!automatic, std::sync::atomic::Ordering::SeqCst);
                    }
                    let active = running.load(std::sync::atomic::Ordering::SeqCst);
                    let waiting = first_test_pending.load(std::sync::atomic::Ordering::SeqCst);
                    let credit_limited = key_connected.load(std::sync::atomic::Ordering::SeqCst);
                    let selected_outcome = outcome();
                    // Both outcomes share the exact source that agent_open returned to the renderer.
                    let report = match selected_outcome.as_deref() {
                        Some(verdict) => json!({"verdict":verdict, "cases":[
                            {"id":"reads-page", "title":"Copies the checksum from the page", "question":"What did the page say?", "phase":"Succeeded", "answer":"Fixture checksum.", "completedTools":["web_fetch"], "verdict":"passed", "assertions":[{"id":"text", "kind":"contains", "expected":"checksum", "verdict":"passed"}]},
                            if verdict == "passed" {
                                json!({"id":"has-a-date", "title":"Says which date is on the page", "question":"Which date is shown on the page?", "phase":"Succeeded", "answer":"The page shows 2099-12-31.", "completedTools":["web_fetch"], "verdict":"passed", "assertions":[{"id":"date", "kind":"contains", "expected":"2099-12-31", "verdict":"passed"}]})
                            } else {
                                json!({"id":"has-a-date", "title":"Says which date is on the page", "question":"Which date is shown on the page?", "phase":"Unknown", "answer":null, "completedTools":[], "verdict":"unknown", "assertions":[{"id":"date", "kind":"contains", "expected":"2099-12-31", "verdict":"unknown"}]})
                            }
                        ]}),
                        None => json!({"verdict":"failed", "cases":[
                            {"id":"reads-page", "title":"Copies the checksum from the page", "question":"What did the page say?", "phase":if credit_limited {"CreditLimited"} else {"Succeeded"}, "answer":if credit_limited {json!(null)} else {json!("Fixture checksum.")}, "completedTools":if credit_limited {json!([])} else {json!(["web_fetch"])}, "verdict":if credit_limited {"failed"} else {"passed"}, "assertions":[{"id":"text", "kind":"contains", "expected":"checksum", "verdict":if credit_limited {"unknown"} else {"passed"}}]},
                            {"id":"has-a-date", "title":"Says which date is on the page", "question":"Which date is shown on the page?", "phase":"Succeeded", "answer":"No date was given.", "completedTools":[], "verdict":"failed", "assertions":[{"id":"date", "kind":"contains", "expected":"2099-12-31", "verdict":"failed"}]}
                        ]}),
                    };
                    json!({"documentId":"9b2b93c4-375c-4815-9717-39852a96e411", "busy":null, "attemptId":run_attempt.lock().unwrap().clone(),
                        "canCancel":active && !cancelling.load(std::sync::atomic::Ordering::SeqCst), "cancellation":if cancelling.load(std::sync::atomic::Ordering::SeqCst) {"cancelling"} else {"none"},
                        "state":if active {"running"} else if waiting {"idle"} else {"complete"}, "stage":if active {json!(if cancelling.load(std::sync::atomic::Ordering::SeqCst) {"cancel"} else {"observe"})} else if waiting {json!(null)} else {json!("done")},
                        "model":if credit_limited {"claude-sonnet-5-5"} else {"gpt-5.6-terra"}, "vendor":if credit_limited {json!("anthropic")} else {json!(null)}, "modelConnected":connected.load(std::sync::atomic::Ordering::SeqCst), "canRun":!active, "source":run_source.lock().unwrap().clone(),
                        "acknowledged":[], "pending":null, "error":if selected_outcome.as_deref() == Some("unknown") {json!("model")} else {json!(null)},
                        "report":if active || waiting {json!(null)} else {report}})
                }
                "setup_status" | "setup_change_local" | "setup_select_local" | "setup_cancel_local" => {
                    let command = invoke.message.command();
                    let phase = setup_progress.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                    if command == "setup_status" && matches!(phase.as_deref(), Some("workspace-unavailable" | "workspace-stop-lost-reply")) {
                        invoke.resolver.reject("Synthetic workspace status unreadable");
                        return true;
                    }
                    if command == "setup_status" {
                        let progress = match phase.as_deref() {
                            Some("designer-no-destination") => Some(json!({"selected":null, "localVm":"cancelled", "model":"locked", "modelSupported":true})),
                            Some("installing") => Some(json!({"selected":"localVm", "localVm":"installing", "model":"locked", "modelSupported":true,
                                "workspaceProgress":{"phase":"materializing", "bytes":1073741824_u64, "totalBytes":2147483648_u64, "elapsedMs":20000, "remainingMs":30000}})),
                            Some("starting") => Some(json!({"selected":"localVm", "localVm":"starting", "model":"locked", "modelSupported":true,
                                "workspaceProgress":{"phase":"starting", "bytes":null, "totalBytes":null, "elapsedMs":45000, "remainingMs":null}})),
                            Some("image-update" | "image-unsupported") => Some(json!({"selected":"localVm", "localVm":"failed", "model":"locked", "modelSupported":true,
                                "workspaceFailure":{"operation":"verify", "cause":if phase.as_deref() == Some("image-update") {"workspaceUpdateRequired"} else {"workspaceVersionUnsupported"}, "codes":[]}})),
                            _ => None,
                        };
                        if let Some(progress) = progress {
                            invoke.resolver.resolve(progress);
                            return true;
                        }
                    }
                    let mut state = workspace.lock().unwrap();
                    if command == "setup_status" && phase.as_deref() == Some("workspace-reset-ready") {
                        // Explicit fixture-only reset between independent terminal captures.
                        // Never rearm the first test or exercise a real restart here.
                        state.selected = true;
                        state.phase = "ready";
                        state.returning = true;
                        state.stop_failed = false;
                    }
                    if command == "setup_status" && phase.as_deref().is_some_and(|s| s.starts_with("preflight-")) {
                        // Preserve the earlier independent preflight captures without using Stop
                        // as a shortcut back to first-run setup.
                        let reset_first_test = state.selected && outcome().is_some();
                        state.selected = false;
                        state.phase = "cancelled";
                        connected.store(false, std::sync::atomic::Ordering::SeqCst);
                        // Reset a fresh-profile outcome once, through the fixture selector, not Stop.
                        if reset_first_test {
                            *agent.lock().unwrap() = original.to_owned();
                            invoke
                                .message
                                .webview()
                                .eval("localStorage.removeItem('bridge.firstTest.v1'); location.reload();")
                                .expect("fixture renderer reload");
                        }
                    }
                    match command {
                        "setup_change_local" => {
                            if state.returning || phase.as_deref().is_some_and(|s| s.starts_with("workspace-")) {
                                invoke.resolver.reject("Stop must preserve the fixture's local selection");
                                return true;
                            }
                            state.selected = false;
                            state.phase = "cancelled";
                            connected.store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                        "setup_select_local" => {
                            cancelling.store(false, std::sync::atomic::Ordering::SeqCst);
                            state.selected = true;
                            state.stop_failed = false;
                            state.phase = if state.returning { "starting" } else { "ready" };
                            if !state.returning { first_test_pending.store(true, std::sync::atomic::Ordering::SeqCst); }
                            running.store(false, std::sync::atomic::Ordering::SeqCst);
                        }
                        "setup_cancel_local" => {
                            assert!(state.selected, "Cancel must retain the selected workspace");
                            state.returning = true;
                            state.stop_failed = false;
                            state.phase = "closing";
                            connected.store(false, std::sync::atomic::Ordering::SeqCst);
                            running.store(false, std::sync::atomic::Ordering::SeqCst);
                            let workspace = workspace.clone();
                            let selector = setup_progress.clone();
                            let resolver = invoke.resolver;
                            tauri::async_runtime::spawn(async move {
                                loop {
                                    let phase = selector.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                                    if matches!(phase.as_deref(), Some("workspace-stopped" | "workspace-closed" | "workspace-stop-failed" | "workspace-stop-lost-reply")) {
                                        let mut state = workspace.lock().unwrap();
                                        state.stop_failed = phase.as_deref() == Some("workspace-stop-failed");
                                        state.phase = if state.stop_failed { "failed" } else if phase.as_deref() == Some("workspace-closed") { "closed" } else { "cancelled" };
                                        if state.stop_failed || phase.as_deref() == Some("workspace-stop-lost-reply") {
                                            // Real cancel errors reject the command; setup_status owns
                                            // the subsequent failure/cancellation observation.
                                            resolver.reject("Synthetic Stop failed or reply lost");
                                        } else {
                                            resolver.resolve(state.status());
                                        }
                                        break;
                                    }
                                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                                }
                            });
                            return true;
                        }
                        _ => {}
                    }
                    if command == "setup_status" && state.phase == "starting" {
                        match phase.as_deref() {
                            Some("workspace-failed") => state.phase = "failed",
                            Some("workspace-running") => state.phase = "ready",
                            _ => {}
                        }
                    }
                    state.status()
                }
                "setup_example" => json!({"agent":original, "tests":tests}),
                // Sizing is synthetic, distinct from transfer progress above. The renderer
                // must show decimal GB without treating materialized image bytes as a download.
                "setup_requirements" => json!({"memoryBytes":4294967296_u64, "imageBytes":3006477107_u64, "stateDiskBytes":21474836480_u64, "downloadBytes":2800000000_u64}),
                "setup_preflight" => {
                    let phase = setup_progress.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                    let unknown = phase.as_deref() == Some("preflight-unknown");
                    let blocked = matches!(phase.as_deref(), Some("preflight-blocked" | "preflight-cluster"));
                    let memory = matches!(phase.as_deref(), Some("preflight-memory" | "preflight-mixed"));
                    let mixed = phase.as_deref() == Some("preflight-mixed");
                    json!({"platform":"macos", "virtualization":if unknown || mixed {"unknown"} else if blocked {"unsupported"} else {"ready"},
                        "memory":if unknown {"unknown"} else if memory {"insufficient"} else {"ready"}, "installedMemoryBytes":if unknown {json!(null)} else if memory {json!(2147483648_u64)} else {json!(17179869184_u64)},
                        "requiredMemoryBytes":4294967296_u64})
                }
                "setup_connect_model" => {
                    let phase = setup_progress.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                    if phase.as_deref() == Some("model-mismatch") {
                        mismatched.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    if mismatched.load(std::sync::atomic::Ordering::SeqCst) {
                        invoke.resolver.reject(json!({"kind":"modelInstallationMismatch", "recovery":fixture_recovery(None)}));
                        return true;
                    }
                    connected.store(true, std::sync::atomic::Ordering::SeqCst);
                    key_connected.store(false, std::sync::atomic::Ordering::SeqCst);
                    // A synthetic login only; no GitHub CLI or credential is read.
                    let vendor = matches!(invoke.message.payload(), tauri::ipc::InvokeBody::Json(args) if !args["vendor"].is_null());
                    if vendor { json!({"ready":true}) } else {
                        json!({"ready":true, "connection":{"kind":"copilot", "model":"gpt-5.6-terra", "account":"synthetic-user"}})
                    }
                }
                "setup_connect_key" => {
                    let tauri::ipc::InvokeBody::Json(args) = invoke.message.payload() else { return false; };
                    let vendor = args["vendor"].as_str().unwrap_or("");
                    let expected = match vendor {
                        "anthropic" => "sk-ant-synthetic-only-not-a-real-key",
                        "openai" => "sk-synthetic-only-not-a-real-key",
                        _ => "",
                    };
                    if expected.is_empty() || args["key"] != expected {
                        invoke.resolver.reject(json!({"kind":"modelKeyInvalid"}));
                        return true;
                    }
                    let attempt = key_attempt.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if let Some(kind) = ["modelCredentialRejected", "modelProviderUnreachable", "modelModelUnavailable"].get(attempt) {
                        let error = json!({"kind":kind});
                        let resolver = invoke.resolver;
                        let release = key_check_release.clone();
                        // The native driver releases the first rejection after capturing Checking.
                        tauri::async_runtime::spawn(async move {
                            if attempt == 0 {
                                if let Some(release) = release {
                                    while !release.exists() { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
                                }
                            }
                            resolver.reject(error);
                        });
                        return true;
                    }
                    key_connected.store(true, std::sync::atomic::Ordering::SeqCst);
                    connected.store(true, std::sync::atomic::Ordering::SeqCst);
                    json!({"ready":true, "connection":{"kind":"key", "vendor":vendor,
                        "model":if vendor == "anthropic" {"claude-sonnet-5-5"} else {"gpt-4.1"}}})
                }
                // Mismatch-only reset with no renderer arguments. The driver's selector
                // chooses a finite outcome; a pending reset waits for the selector to change.
                "setup_reset_model" => {
                    if !mismatched.load(std::sync::atomic::Ordering::SeqCst) {
                        invoke.resolver.reject(json!({"kind":"modelUnavailable"}));
                        return true;
                    }
                    let path = setup_progress.clone();
                    let flag = mismatched.clone();
                    let resolver = invoke.resolver;
                    tauri::async_runtime::spawn(async move {
                        let read = || path.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                        while read().as_deref() == Some("reset-pending") {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        match read().as_deref() {
                            Some("reset-fail-credential") => resolver.reject(json!({"kind":"modelStore",
                                "recovery":fixture_recovery(Some("credentialRemovalUnconfirmed"))})),
                            Some("reset-fail-record") => resolver.reject(json!({"kind":"modelUnavailable",
                                "recovery":fixture_recovery(Some("recordRemovalUnconfirmed"))})),
                            _ => {
                                flag.store(false, std::sync::atomic::Ordering::SeqCst);
                                resolver.resolve(json!({"ready":false}));
                            }
                        }
                    });
                    return true;
                }
                "setup_remove_model" => {
                    connected.store(false, std::sync::atomic::Ordering::SeqCst);
                    let phase = setup_progress.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                    let cleanup = match phase.as_deref() {
                        Some("cleanup-status") => Some(("modelStatus", "unsupported")),
                        Some("cleanup-preconditions") => Some(("modelStatusPreconditions", "refused")),
                        _ => None,
                    };
                    if let Some(cleanup) = cleanup {
                        invoke.resolver.reject(json!({"kind":"modelReconnect", "cleanup":cleanup, "message":"synthetic-private-token"}));
                        return true;
                    }
                    if key_connected.load(std::sync::atomic::Ordering::SeqCst)
                        && !key_removed_once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        invoke.resolver.reject(json!({"kind":"modelKeyCleanup"}));
                        return true;
                    }
                    key_connected.store(false, std::sync::atomic::Ordering::SeqCst);
                    if !removed_once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        invoke.resolver.reject(json!({"kind":"modelReconnect", "cleanup":["disconnectRetainedModel","refused"], "message":"synthetic-private-token"}));
                        return true;
                    }
                    json!({"ready":false})
                }
                _ => {
                    invoke
                        .resolver
                        .reject("Unavailable in the native model fixture");
                    return true;
                }
            };
            invoke.resolver.resolve(reply);
            true
        })
        .run(tauri::generate_context!())
        .expect("native model fixture failed");
}
