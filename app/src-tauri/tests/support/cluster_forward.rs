use super::*;
use bridge_engine::engine::run::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tungstenite::{protocol::Role, Message, WebSocket};

#[path = "cluster_send.rs"]
mod send;

#[derive(Default)]
pub(super) struct Wire {
    calls: Mutex<Vec<(String, Value)>>,
    mode: Mutex<&'static str>,
    catalog: Mutex<Option<Vec<(u16, Value)>>>,
    active: AtomicUsize,
    forwards: AtomicUsize,
    objects: Mutex<HashMap<String, Value>>,
    after_post: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl Wire {
    pub(super) fn serve(&self, mut stream: std::net::TcpStream, headers: &str) {
        self.forwards.fetch_add(1, Ordering::SeqCst);
        assert!(headers
            .lines()
            .next()
            .unwrap()
            .starts_with("GET /api/v1/namespaces/control/pods/ready/portforward?"));
        let port: u16 = headers
            .lines()
            .next()
            .unwrap()
            .split("ports=")
            .nth(1)
            .unwrap()
            .split(['&', ' '])
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(port == 8080 || port == 9090);
        let key = headers
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                k.eq_ignore_ascii_case("sec-websocket-key")
                    .then(|| v.trim())
            })
            .unwrap();
        let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
        write!(stream, "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: v4.channel.k8s.io\r\n\r\n").unwrap();
        let mut ws = WebSocket::from_raw_socket(stream, Role::Server, None);
        self.active.fetch_add(1, Ordering::SeqCst);
        struct Active<'a>(&'a AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _active = Active(&self.active);
        for channel in [0, 1] {
            let [lo, hi] = port.to_le_bytes();
            ws.send(Message::Binary(vec![channel, lo, hi].into()))
                .unwrap();
        }
        let mut bytes = Vec::new();
        while let Ok(message) = ws.read() {
            let Message::Binary(data) = message else {
                continue;
            };
            if data.first() != Some(&0) {
                continue;
            }
            bytes.extend_from_slice(&data[1..]);
            while let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let end = i + 4;
                let headers = String::from_utf8_lossy(&bytes[..end]).into_owned();
                let len: usize = headers
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() < end + len {
                    break;
                }
                let mut first = headers.lines().next().unwrap().split_whitespace();
                let method = first.next().unwrap();
                let path = first.next().unwrap();
                assert!(path.contains("namespace=agents"));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer creator-fixture")
                        || headers
                            .to_ascii_lowercase()
                            .contains("authorization: bearer issued-fixture")
                );
                let body: Value =
                    serde_json::from_slice(&bytes[end..end + len]).unwrap_or(Value::Null);
                self.calls
                    .lock()
                    .unwrap()
                    .push((format!("{method} {path}"), body.clone()));
                let mode = *self.mode.lock().unwrap();
                if mode == "broken" && method == "POST" {
                    return;
                }
                if (mode == "hang" && method == "POST")
                    || (mode == "hang-tools" && path.starts_with("/api/v1/tools?"))
                {
                    // Wait for the owner's close to tear down the actual websocket.
                    while ws.read().is_ok() {}
                    return;
                }
                while mode == "hold-history" && *self.mode.lock().unwrap() == "hold-history" {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let route = path.split('?').next().unwrap();
                let mut objects = self.objects.lock().unwrap();
                let (mut status, mut value) = if method == "POST" {
                    let kind = if route.ends_with("agents") {
                        "Agent"
                    } else {
                        "Task"
                    };
                    let mut object = json!({"apiVersion":"core.orka.ai/v1alpha1","kind":kind,"metadata":{"name":body["name"],"namespace":"agents","uid":format!("uid-{}",body["name"].as_str().unwrap()),"generation":1},"spec":body["spec"],"status":{"phase":match mode { "failed" => "Failed", "running" => "Running", _ => "Succeeded" }}});
                    if kind == "Task" {
                        object["spec"]["timeout"] = json!("2m0s");
                        object["spec"]["retryPolicy"]
                            .as_object_mut()
                            .unwrap()
                            .remove("maxRetries");
                    }
                    objects.insert(
                        format!("{route}/{}", body["name"].as_str().unwrap()),
                        object.clone(),
                    );
                    (201, object)
                } else if route.ends_with("/auth/whoami") {
                    if mode == "fallback" && headers.contains("creator-fixture") {
                        (
                            200,
                            json!({"authenticated":true,"authType":"tokenReview","namespace":""}),
                        )
                    } else {
                        (
                            200,
                            json!({"authenticated":true,"authType":"oidc","namespace":"agents"}),
                        )
                    }
                } else if route.ends_with("/providers") && mode == "paged" {
                    let url = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
                    let next = url
                        .query_pairs()
                        .any(|(k, v)| k == "continue" && v == "next+/ =?");
                    (
                        200,
                        json!({"items":[{"metadata":{"name":if next {"second"} else {"first"},"namespace":"agents"},"status":{"ready":true}}],"metadata":if next {json!({})} else {json!({"continue":"next+/ =?"})}}),
                    )
                } else if route.ends_with("/providers") {
                    (
                        200,
                        json!({"items":[{"metadata":{"name":"model","namespace":"agents"},"status":{"ready":mode != "unready"}}],"metadata":{}}),
                    )
                } else if route.ends_with("/tools") {
                    let pages = self.catalog.lock().unwrap();
                    if let Some(pages) = pages.as_ref() {
                        let url = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
                        let index = url
                            .query_pairs()
                            .find(|(k, _)| k == "continue")
                            .map(|(_, v)| {
                                assert_eq!(v, "next+/ =?");
                                1
                            })
                            .unwrap_or(0);
                        pages[index].clone()
                    } else {
                        (
                            200,
                            json!({"items":[{"name":"web_fetch","builtin":true}],"metadata":{}}),
                        )
                    }
                } else if route.ends_with("/agents") && mode == "paged-agents" {
                    let url = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
                    let next = url
                        .query_pairs()
                        .any(|(k, v)| k == "continue" && v == "next+/ =?");
                    (
                        200,
                        json!({"items":[{"metadata":{"name":"reader","namespace":"agents","uid":if next {"second"} else {"first"}}}],"metadata":if next {json!({})} else {json!({"continue":"next+/ =?"})}}),
                    )
                } else if route.ends_with("/agents") {
                    (200, json!({"items":[],"metadata":{}}))
                } else if route.ends_with("/result") {
                    if mode == "result-404" {
                        (404, json!({"error":"missing"}))
                    } else if mode == "result-403" {
                        (403, json!({"error":"creator-fixture"}))
                    } else if mode == "result-echo" {
                        (200, json!({"result":"creator-fixture"}))
                    } else {
                        (200, json!({"result":"expected answer"}))
                    }
                } else if route.ends_with("/events") {
                    let task = route
                        .trim_end_matches("/events")
                        .rsplit('/')
                        .next()
                        .unwrap();
                    let agent = objects.get(route.trim_end_matches("/events")).unwrap()["spec"]
                        ["agentRef"]["name"]
                        .clone();
                    (
                        200,
                        json!({"namespace":"agents","streamType":"task","streamID":task,"afterSeq":0,"latestSeq":1,"events":[{"namespace":"agents","streamType":"task","streamID":task,"seq":1,"type":"ToolCallCompleted","taskName":task,"agentName":agent,"toolName":"web_fetch","toolCallID":"call"}]}),
                    )
                } else {
                    objects
                        .get(route)
                        .map(|v| (200, v.clone()))
                        .unwrap_or((404, json!({"error":"missing"})))
                };
                if method == "POST" {
                    if let Some(hook) = self.after_post.lock().unwrap().take() {
                        hook();
                    }
                }
                if method == "GET"
                    && route.ends_with("/result")
                    && ["wrong-result", "wrong-result-events-missing"].contains(&mode)
                {
                    value = json!({"result":"wrong answer"});
                }
                if method == "GET" && route.ends_with("/result") && mode == "empty-result" {
                    value = json!({"result":null});
                }
                if method == "GET"
                    && mode == "empty-result"
                    && route.contains("/tasks/")
                    && !route.ends_with("/result")
                    && !route.ends_with("/events")
                {
                    value["status"]["phase"] = json!("Succeeded");
                }
                if method == "GET" && mode == "failed-empty" && route.contains("/tasks/") {
                    if route.ends_with("/result") {
                        status = 404;
                        value = json!({"error":"missing"});
                    } else if !route.ends_with("/events") {
                        value["status"]["phase"] = json!("Failed");
                    }
                }
                if method == "GET" && route.ends_with("/result") && mode == "large-result" {
                    value = json!({"result": format!("expected answer{}", "x".repeat(900_000))});
                }
                if method == "GET" && route.contains("/agents/") && mode == "agent-replaced" {
                    value["metadata"]["uid"] = json!("replacement");
                }
                if method == "GET" && route.contains("/agents/") && mode == "agent-generation" {
                    value["metadata"]["generation"] = json!(2);
                }
                if method == "GET"
                    && route.ends_with("/events")
                    && ["events-404", "wrong-result-events-missing"].contains(&mode)
                {
                    status = 404;
                    value = json!({"error":"missing"});
                }
                if method == "GET"
                    && route.contains("/tasks/")
                    && !route.ends_with("/result")
                    && !route.ends_with("/events")
                    && mode == "task-404"
                {
                    status = 404;
                    value = json!({"error":"missing"});
                }
                if method == "GET"
                    && mode
                        .strip_prefix("get-")
                        .is_some_and(|m| route.ends_with(m))
                {
                    status = 401;
                } else if method == "GET"
                    && mode
                        .strip_prefix("deny-")
                        .is_some_and(|m| route.ends_with(m))
                {
                    status = 403;
                }
                if method == "POST" {
                    match mode {
                        "401" => {
                            status = 401;
                            value = json!({"error":"creator-fixture"});
                        }
                        "403" => {
                            status = 403;
                            value = json!({"error":"creator-fixture"});
                        }
                        "echo" => value["status"]["echo"] = json!("prefix creator-fixture suffix"),
                        "echo-issued" => value["status"]["echo"] = json!("issued-fixture"),
                        "chunked-large" => value["padding"] = json!("x".repeat(1024 * 1024)),
                        "redirect" => status = 307,
                        "large" => value["padding"] = json!("x".repeat(1024 * 1024)),
                        _ => {}
                    }
                }
                let body = value.to_string();
                let response = if mode == "chunked-large" && method == "POST" {
                    format!("HTTP/1.1 {status} OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",body.len())
                } else {
                    format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len())
                };
                let mut frame = vec![0];
                frame.extend_from_slice(response.as_bytes());
                if ws.send(Message::Binary(frame.into())).is_err() {
                    return;
                }
                bytes.drain(..end + len);
            }
        }
    }
    pub(super) fn posts(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.0.starts_with("POST"))
            .count()
    }
}
async fn access(api: &Arc<ApiFixture>) -> (ClusterClient, RemoteAccess) {
    let mut client = ClusterClient::new(
        config(&api.start(), json!({"token":"creator-fixture"})),
        Arc::new(PodOrkaRead),
    );
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    (client, access)
}
fn pair() -> ParsedPair {
    pair_with_prompt("")
}
fn pair_with_prompt(extra: &str) -> ParsedPair {
    let dir = tempfile::tempdir().unwrap();
    let agent = dir.path().join("agent.md");
    let tests = dir.path().join("agent.test.md");
    std::fs::write(&agent,"---\nname: reader\ndescription: inert\nmodel: 'model:exact'\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    - kind: builtin\n      name: web_fetch\n---\nFetch only.\n").unwrap();
    std::fs::write(&tests,"---\nschemaVersion: 1\n---\n```yaml agent-test\nid: fetch\ninput: go\nassertions:\n  - id: answer\n    type: contains\n    value: expected answer\n  - id: fetched\n    type: toolCalled\n    tool: web_fetch\n```\n").unwrap();
    if !extra.is_empty() {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&agent)
            .unwrap()
            .write_all(extra.as_bytes())
            .unwrap();
    }
    let catalog = bridge_engine::pair_source::PairCatalog::default();
    let id = catalog
        .bind_owned_agent(
            dir.path().to_path_buf(),
            bridge_engine::runtime::snapshot::AgentDocument {
                agent: "agent.md".into(),
                tests: "agent.test.md".into(),
            },
        )
        .unwrap();
    let captured = catalog.capture(id).unwrap();
    assert_eq!(captured.pair().snapshot(), &captured.snapshot().id());
    let parsed = captured.into_pair();
    std::fs::write(agent, "changed after capture").unwrap();
    parsed
}
#[tokio::test]
async fn pod_reads_keep_kubernetes_and_orka_status_origins() {
    let cases = [
        (
            "get-whoami",
            Check::OrkaIdentity,
            CheckResult::AuthenticationRejected,
        ),
        (
            "deny-whoami",
            Check::OrkaIdentity,
            CheckResult::PermissionDenied,
        ),
        ("deny-agents", Check::Agents, CheckResult::PermissionDenied),
        (
            "get-providers",
            Check::Providers,
            CheckResult::AuthenticationRejected,
        ),
    ];
    for (mode, check, result) in cases {
        let api = Arc::new(ApiFixture::default());
        *api.wire.mode.lock().unwrap() = mode;
        let mut client = ClusterClient::new(
            config(&api.start(), json!({"token":"creator-fixture"})),
            Arc::new(PodOrkaRead),
        );
        let id = target(&mut client).await;
        assert_eq!(
            client.connect(id, None).await.err(),
            Some(failed(check, result)),
            "{mode}"
        );
    }
    // Opening the Pod stream is the Orka connection check, with Kubernetes' own status.
    for (code, result) in [
        (401, CheckResult::AuthenticationRejected),
        (403, CheckResult::PermissionDenied),
    ] {
        let api = Arc::new(ApiFixture::default());
        let mut client = ClusterClient::new(
            config(&api.start(), json!({"token":"creator-fixture"})),
            Arc::new(PodOrkaRead),
        );
        let id = target(&mut client).await;
        *api.override_response.lock().unwrap() = Some((
            "/pods".into(),
            code,
            json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"creator-fixture","reason":"Fixture","code":code}),
        ));
        assert_eq!(
            client.connect(id, None).await.err(),
            Some(failed(Check::OrkaConnection, result))
        );
        assert_eq!(api.wire.posts(), 0);
    }
}
async fn transport(api: &Arc<ApiFixture>) -> OrkaStreamTransport {
    let (client, access) = access(api).await;
    access
        .transport(&client, access.providers()[0].id)
        .await
        .unwrap()
}
#[tokio::test]
async fn disk_pair_to_cluster_checks_pass() {
    let api = Arc::new(ApiFixture::default());
    let transport = transport(&api).await;
    assert_eq!(transport.peer().pod_uid, "pod-uid");
    let runner = NativeRunner::with_transport(transport);
    let pair = pair();
    let snapshot = pair.snapshot().clone();
    let handle = runner.submit(pair).await.unwrap();
    assert_eq!(handle.snapshot(), &snapshot);
    let report = runner.observe(&handle).await.unwrap();
    assert_eq!(report.verdict, Verdict::Passed);
    runner.close().await;
    assert_eq!(api.wire.posts(), 2);
    let calls = api.wire.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .find(|c| c.0.starts_with("POST /api/v1/agents"))
            .unwrap()
            .1["spec"]["model"]["name"],
        "model:exact"
    );
}
#[tokio::test]
async fn overview_inventory_wire_has_one_scoped_page_limit_not_the_access_probe() {
    let api = Arc::new(ApiFixture::default());
    let mut client = ClusterClient::new(
        config(&api.start(), json!({"token":"creator-fixture"})),
        Arc::new(PodOrkaRead),
    );
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    api.wire.calls.lock().unwrap().clear();
    *api.wire.mode.lock().unwrap() = "paged-agents";
    assert_eq!(access.inventory().await.unwrap().len(), 2);
    let calls = api.wire.calls.lock().unwrap();
    let requests: Vec<_> = calls
        .iter()
        .filter(|c| c.0.starts_with("GET /api/v1/agents?"))
        .collect();
    assert_eq!(requests.len(), 2);
    for (index, call) in requests.iter().enumerate() {
        let request = &call.0;
        let url = reqwest::Url::parse(&format!(
            "http://fixture{}",
            request.trim_start_matches("GET ")
        ))
        .unwrap();
        assert_eq!(
            url.query_pairs()
                .filter(|(k, _)| k == "limit")
                .map(|(_, v)| v.into_owned())
                .collect::<Vec<_>>(),
            ["10"]
        );
        assert!(url
            .query_pairs()
            .any(|(k, v)| k == "namespace" && v == "agents"));
        assert_eq!(
            url.query_pairs()
                .find(|(k, _)| k == "continue")
                .map(|(_, v)| v.into_owned()),
            if index == 0 {
                None
            } else {
                Some("next+/ =?".into())
            }
        );
    }
    drop(calls);
    assert_eq!(api.wire.posts(), 0);
}

#[tokio::test]
async fn agent_access_probe_requests_only_one_item() {
    let api = Arc::new(ApiFixture::default());
    let _ = access(&api).await;
    let calls = api.wire.calls.lock().unwrap();
    let path = calls
        .iter()
        .find(|c| c.0.starts_with("GET /api/v1/agents?"))
        .unwrap()
        .0
        .trim_start_matches("GET ");
    let url = reqwest::Url::parse(&format!("http://fixture{path}")).unwrap();
    assert!(url.query_pairs().any(|(k, v)| k == "limit" && v == "1"));
    drop(calls);
    assert_eq!(api.wire.posts(), 0);
}
#[tokio::test]
async fn metadata_continue_is_distinct_from_event_after_cursor() {
    let api = Arc::new(ApiFixture::default());
    *api.wire.mode.lock().unwrap() = "paged";
    let (_, access) = access(&api).await;
    assert_eq!(
        access
            .providers()
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
    assert_eq!(api.wire.posts(), 0);
}
#[tokio::test]
async fn remote_create_over_64k_has_no_guest_cap() {
    let api = Arc::new(ApiFixture::default());
    let runner = NativeRunner::with_transport(transport(&api).await);
    let handle = runner
        .submit(pair_with_prompt(&"x".repeat(70_000)))
        .await
        .unwrap();
    assert!(handle.agent().is_some());
    assert!(api
        .wire
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(path, body)| path.starts_with("POST") && body.to_string().len() > 65_536));
    runner.close().await;
}
#[tokio::test]
async fn selected_ready_pod_resolves_named_target_port_during_rollout() {
    let api = Arc::new(ApiFixture::default());
    *api.override_response.lock().unwrap() = Some((
        "/pods".into(),
        200,
        json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[{"metadata":{"name":"ready","namespace":"control","uid":"rollout-uid","labels":{"app":"orka"}},"spec":{"containers":[{"name":"controller","ports":[{"name":"api","containerPort":9090}]}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}]}}]}),
    ));
    let transport = transport(&api).await;
    assert_eq!(transport.peer().port, 9090);
    assert_eq!(transport.peer().pod_uid, "rollout-uid");
    transport.close().await;
}
#[tokio::test]
async fn unavailable_optional_evidence_does_not_poison_a_healthy_forward() {
    for mode in ["result-404", "result-echo"] {
        let api = Arc::new(ApiFixture::default());
        let runner = NativeRunner::with_transport(transport(&api).await);
        let handle = runner.submit(pair()).await.unwrap();
        let forwards = api.wire.forwards.load(Ordering::SeqCst);
        *api.wire.mode.lock().unwrap() = mode;
        let report = runner.observe(&handle).await.unwrap();
        assert_eq!(report.verdict, Verdict::Unknown);
        assert!(report.cases[0].answer.is_none());
        assert_eq!(report.cases[0].completed_tools, vec!["web_fetch"]);
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("creator-fixture"));
        assert_eq!(api.wire.forwards.load(Ordering::SeqCst), forwards);
        assert_eq!(api.wire.posts(), 2);
        runner.close().await;
    }
}
#[tokio::test]
async fn authentication_during_optional_evidence_keeps_receipts() {
    let api = Arc::new(ApiFixture::default());
    let runner = NativeRunner::with_transport(transport(&api).await);
    let handle = runner.submit(pair()).await.unwrap();
    *api.wire.mode.lock().unwrap() = "result-403";
    let error = runner.observe(&handle).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Permission);
    assert!(error.handle.agent().is_some());
    assert_eq!(error.handle.tasks().len(), 1);
    assert!(!format!("{error:?}").contains("creator-fixture"));
    runner.close().await;
}
#[tokio::test]
async fn credential_echo_not_exported_after_account_fallback() {
    for mode in ["echo", "echo-issued"] {
        let api = Arc::new(ApiFixture::default());
        *api.wire.mode.lock().unwrap() = "fallback";
        let mut client = ClusterClient::new(
            config(&api.start(), json!({"token":"creator-fixture"})),
            Arc::new(PodOrkaRead),
        );
        let id = target(&mut client).await;
        let access = client.connect(id, Some("existing".into())).await.unwrap();
        let runner = NativeRunner::with_transport(
            access
                .transport(&client, access.providers()[0].id)
                .await
                .unwrap(),
        );
        *api.wire.mode.lock().unwrap() = mode;
        let error = runner.submit(pair()).await.unwrap_err();
        assert_eq!(error.kind, ErrorKind::Response);
        assert!(error.pending.is_some());
        assert!(!format!("{error:?}").contains("fixture"));
        runner.close().await;
    }
}
#[tokio::test]
async fn stop_before_submit_and_observe_closes_without_new_request() {
    for observe in [false, true] {
        let api = Arc::new(ApiFixture::default());
        let runner = NativeRunner::with_transport(transport(&api).await);
        let handle = if observe {
            Some(runner.submit(pair()).await.unwrap())
        } else {
            None
        };
        let count = api.wire.calls.lock().unwrap().len();
        let (_tx, rx) = tokio::sync::watch::channel(true);
        let error = if let Some(handle) = handle {
            runner.observe_stoppable(&handle, rx).await.unwrap_err()
        } else {
            runner
                .submit_prepared(runner.prepare(pair()).unwrap(), rx)
                .await
                .unwrap_err()
        };
        assert_eq!(error.kind, ErrorKind::Stopped);
        assert!(error.pending.is_none());
        assert_eq!(api.wire.calls.lock().unwrap().len(), count);
        tokio::time::timeout(Duration::from_secs(2), async {
            while api.wire.active.load(Ordering::SeqCst) != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn failed_task_never_passes() {
    let api = Arc::new(ApiFixture::default());
    let runner = NativeRunner::with_transport(transport(&api).await);
    *api.wire.mode.lock().unwrap() = "failed";
    let handle = runner.submit(pair()).await.unwrap();
    assert_eq!(
        runner.observe(&handle).await.unwrap().verdict,
        Verdict::Failed
    );
    runner.close().await;
}
#[tokio::test]
async fn binding_changed_zero_product_post() {
    let api = Arc::new(ApiFixture::default());
    let (client, access) = access(&api).await;
    let transport = access
        .transport(&client, access.providers()[0].id)
        .await
        .unwrap();
    *api.replacement.lock().unwrap() = true;
    assert!(matches!(
        access.transport(&client, access.providers()[0].id).await,
        Err(ClusterError::Unavailable)
    ));
    let runner = NativeRunner::with_transport(transport);
    assert_eq!(
        runner.submit(pair()).await.unwrap_err().kind,
        ErrorKind::Identity
    );
    assert_eq!(api.wire.posts(), 0);
    runner.close().await;
}
#[tokio::test]
async fn broken_forward_unknown_no_replay() {
    let api = Arc::new(ApiFixture::default());
    let runner = NativeRunner::with_transport(transport(&api).await);
    let forwards = api.wire.forwards.load(Ordering::SeqCst);
    *api.wire.mode.lock().unwrap() = "broken";
    let error = runner.submit(pair()).await.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Transport);
    assert!(error.pending.is_some());
    assert!(error.handle.agent().is_none());
    assert_eq!(api.wire.posts(), 1);
    assert_eq!(api.wire.forwards.load(Ordering::SeqCst), forwards);
    runner.close().await;
}
#[tokio::test]
async fn create_auth_echo_redirect_and_cap_are_finite() {
    for (mode, kind) in [
        ("401", ErrorKind::Authentication),
        ("403", ErrorKind::Permission),
        ("echo", ErrorKind::Response),
        ("redirect", ErrorKind::Response),
        ("large", ErrorKind::Response),
        ("chunked-large", ErrorKind::Response),
    ] {
        let api = Arc::new(ApiFixture::default());
        let runner = NativeRunner::with_transport(transport(&api).await);
        *api.wire.mode.lock().unwrap() = mode;
        let error = runner.submit(pair()).await.unwrap_err();
        assert_eq!(error.kind, kind, "{mode}");
        assert!(error.pending.is_some());
        assert!(!format!("{error:?}").contains("creator-fixture"));
        assert_eq!(api.wire.posts(), 1);
        runner.close().await;
    }
}
#[tokio::test]
async fn stop_inflight_closes_forward_and_retains_pending() {
    let api = Arc::new(ApiFixture::default());
    let runner = NativeRunner::with_transport(transport(&api).await);
    *api.wire.mode.lock().unwrap() = "hang";
    let (tx, rx) = tokio::sync::watch::channel(false);
    let prepared = runner.prepare(pair()).unwrap();
    let stop = async {
        while api.wire.posts() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tx.send_replace(true);
    };
    let (result, ()) = tokio::join!(runner.submit_prepared(prepared, rx), stop);
    let error = result.unwrap_err();
    assert_eq!(error.kind, ErrorKind::Stopped);
    assert!(error.pending.is_some());
    // Stop itself must close the stream, not just a later explicit owner close.
    tokio::time::timeout(Duration::from_secs(2), async {
        while api.wire.active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    // close is idempotent and completes even after Stop already joined the tasks.
    runner.close().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while api.wire.active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
