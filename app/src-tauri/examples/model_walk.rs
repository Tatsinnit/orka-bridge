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
            if started.elapsed().as_millis() >= 1500 {
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
            } else if self.view.state == ChatState::Preparing {
                self.view.state = ChatState::Working;
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
                self.view.state = if self.wait_for_stop {
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
    let selected = std::sync::atomic::AtomicBool::new(true);
    let connected = std::sync::atomic::AtomicBool::new(false);
    let key_connected = std::sync::atomic::AtomicBool::new(false);
    let key_attempt = std::sync::atomic::AtomicUsize::new(0);
    let key_check_release =
        std::env::var_os("BRIDGE_MODEL_CHECK_RELEASE").map(std::path::PathBuf::from);
    // A bounded state selector for setup renderer captures, never part of the app.
    let setup_progress = std::env::var_os("BRIDGE_SETUP_PROGRESS").map(std::path::PathBuf::from);
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
                        "source":source,"summary":{"name":"pr-summary","description":"Explains a pull request before review.","model":"gpt-5.6-terra","tools":[]},
                        "reason":null,"localResult":null
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
                "overview_places" => json!({"historyState":"complete","unobserved":[],"places":[{
                    "key":"team-observed","context":"team-dev","service":"control/orka","namespace":"agents",
                    "stale":false,"observedAt":1234567890000_u64,"agents":[{
                        "name":"review-helper","namespace":"agents","uid":"external-agent",
                        "agentType":"runtime","model":"team-model","tools":["web_fetch"],"record":null
                    }]
                },{
                    "key":"team-remembered","context":"team-west","service":"control/orka","namespace":"agents",
                    "stale":true,"observedAt":null,"agents":[]
                }]}),
                "agents_list" => json!([{"id":bridge_engine::agent_folder::DocumentId::SAMPLE,
                    "name":"web-reader", "path":null, "available":true}]),
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
                "workspace_catalog" => json!({
                    "defaultModel":"gpt-5.6-terra",
                    "models":{"state":"ready", "items":[{"id":"gpt-5.6-terra"}]},
                    "tools":{"state":"ready", "items":[
                        {"kind":"builtin", "name":"web_fetch", "description":"Read a web page"},
                        {"kind":"builtin", "name":"file_read", "description":"Read a file"}
                    ]}
                }),
                // Only a synthetic destination for the failed-local-test popover walk.
                // No Send command is implemented: Fix it first and Escape must not submit.
                "cluster_places" => json!({"places":[{
                    "key":"team-observed", "context":"team-dev", "service":"control/orka", "namespace":"agents",
                    "stale":false, "observedAt":1234567890000_u64, "agents":[]
                }], "selectedKey":cluster_selected.load(std::sync::atomic::Ordering::SeqCst).then_some("team-observed"), "persistenceState":"complete"}),
                "cluster_reconnect" => target.clone(),
                "cluster_use" => {
                    cluster_selected.store(true, std::sync::atomic::Ordering::SeqCst);
                    json!({"status":"connected", "providers":providers, "failure":null})
                }
                "send_preflight" => {
                    let content = agent.lock().unwrap();
                    json!({"documentId":bridge_engine::agent_folder::DocumentId::SAMPLE,
                        "targetId":target["id"], "providerId":providers[0]["id"],
                        "source":bridge_runtime_protocol::SnapshotId::of(content.as_bytes(), tests.as_bytes()),
                        "model":"gpt-5.6-terra", "canSend":true, "action":"create", "canSeparate":false,
                        "rows":[{"label":"Destination", "status":"allowed", "message":"Synthetic destination ready"}]})
                }
                "cluster_contexts" => json!([]),
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
                "setup_status" | "setup_change_local" | "setup_select_local" => {
                    let phase = setup_progress.as_ref().and_then(|path| std::fs::read_to_string(path).ok());
                    if invoke.message.command() == "setup_status" {
                        let progress = match phase.as_deref() {
                            Some("installing") => Some(json!({"selected":"localVm", "localVm":"installing", "model":"locked", "modelSupported":true,
                                "workspaceProgress":{"phase":"materializing", "bytes":1073741824_u64, "totalBytes":2147483648_u64, "elapsedMs":20000, "remainingMs":30000}})),
                            Some("starting") => Some(json!({"selected":"localVm", "localVm":"starting", "model":"locked", "modelSupported":true,
                                "workspaceProgress":{"phase":"starting", "bytes":null, "totalBytes":null, "elapsedMs":45000, "remainingMs":null}})),
                            _ => None,
                        };
                        if let Some(progress) = progress {
                            invoke.resolver.resolve(progress);
                            return true;
                        }
                    }
                    if invoke.message.command() == "setup_change_local" {
                        selected.store(false, std::sync::atomic::Ordering::SeqCst);
                        connected.store(false, std::sync::atomic::Ordering::SeqCst);
                        // With an outcome selected, Stop stands in for a fresh profile: restore the
                        // pristine sample and reload the renderer without its one-shot first-test claim.
                        if outcome().is_some() {
                            *agent.lock().unwrap() = original.to_owned();
                            invoke
                                .message
                                .webview()
                                .eval("localStorage.removeItem('bridge.firstTest.v1'); location.reload();")
                                .expect("fixture renderer reload");
                        }
                    } else if invoke.message.command() == "setup_select_local" {
                        cancelling.store(false, std::sync::atomic::Ordering::SeqCst);
                        selected.store(true, std::sync::atomic::Ordering::SeqCst);
                        first_test_pending.store(true, std::sync::atomic::Ordering::SeqCst);
                        running.store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                    if selected.load(std::sync::atomic::Ordering::SeqCst) {
                        json!({"selected":"localVm", "localVm":"ready", "model":"available"})
                    } else {
                        json!({"selected":null, "localVm":"cancelled", "model":"locked"})
                    }
                }
                "setup_example" => json!({"agent":original, "tests":tests}),
                // Sizing is synthetic, distinct from transfer progress above. The renderer
                // must show decimal GB without treating materialized image bytes as a download.
                "setup_requirements" => json!({"memoryBytes":4294967296_u64, "imageBytes":3006477107_u64, "stateDiskBytes":21474836480_u64}),
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
                    if key_connected.load(std::sync::atomic::Ordering::SeqCst)
                        && !key_removed_once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        invoke.resolver.reject(json!({"kind":"modelKeyCleanup"}));
                        return true;
                    }
                    key_connected.store(false, std::sync::atomic::Ordering::SeqCst);
                    if !removed_once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        invoke.resolver.reject(json!({"kind":"modelBusy"}));
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
