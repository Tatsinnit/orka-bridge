// Included in orka's test module to reuse its synthetic kubectl and HTTP fixtures.
const CHAT_AGENT: &str = "bridge-chat-agent-0123456789abcdef0123456789abcdef";
const CHAT_TASK: &str = "bridge-chat-task-0123456789abcdef0123456789abcdef";
const CHAT_SESSION: &str = "bridge-chat-0123456789abcdef0123456789abcdef";
fn chat_request(method: ChatMethod, path: String) -> ChatRequest {
    ChatRequest {
        method,
        path,
        after: None,
        body: None,
    }
}
fn chat_kube(port: u16) -> (tempfile::TempDir, Orka) {
    let (dir, run) = kube(port);
    let mut chat = Orka::new_chat(installation());
    chat.kubectl = run.kubectl.clone();
    (dir, chat)
}
#[test]
fn chat_uses_separate_exact_identity_and_explicit_methods() {
    let s = server(vec![
        http("201 Created", "{}"),
        http("204 No Content", ""),
        http("200 OK", r#"{"activeTask":"","transcript":""}"#),
    ]);
    let (dir, mut chat) = chat_kube(s.port);
    let mut post = chat_request(ChatMethod::Post, "agents".into());
    post.body = Some(json!({"name":CHAT_AGENT,"namespace":"orka-system","spec":{}}));
    assert_eq!(reply(chat.execute_chat(&post).unwrap()).status, 201);
    let delete = chat_request(ChatMethod::Delete, format!("tasks/{CHAT_TASK}"));
    assert_eq!(
        reply(chat.execute_chat(&delete).unwrap()),
        OrkaReply {
            status: 204,
            body: Value::Null
        }
    );
    let session = chat_request(ChatMethod::Get, format!("sessions/{CHAT_SESSION}"));
    assert_eq!(
        reply(chat.execute_chat(&session).unwrap()).body["transcript"],
        ""
    );
    let commands = log(dir.path());
    assert!(commands
        .iter()
        .all(|l| !l.contains("bridge-native-run") && !l.contains("patch") && !l.contains("apply")));
    assert!(commands
        .iter()
        .any(|l| l.contains("create token bridge-native-chat --duration=10m")));
    let created: Vec<Value> = fs::read_to_string(dir.path().join("created"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(created.len(), 3);
    for value in &created {
        assert_eq!(value["metadata"]["name"], "bridge-native-chat");
    }
    assert_eq!(
        created[1]["rules"],
        json!([
            {"apiGroups":["core.orka.ai"],"resources":["agents"],"verbs":["create","get"]},
            {"apiGroups":["core.orka.ai"],"resources":["tasks"],"verbs":["create","get","delete"]},
            {"apiGroups":["core.orka.ai"],"resources":["sessions"],"verbs":["get"]},
            {"apiGroups":["core.orka.ai"],"resources":["tools","providers"],"verbs":["get"]}
        ])
    );
    assert_eq!(created[2]["roleRef"]["name"], "bridge-native-chat");
    assert_eq!(created[2]["subjects"][0]["name"], "bridge-native-chat");
    let seen = s.seen.lock().unwrap();
    for (request, first) in seen.iter().zip([
        "POST /api/v1/agents?namespace=orka-system HTTP/1.1\r\n".to_owned(),
        format!("DELETE /api/v1/tasks/{CHAT_TASK}?namespace=orka-system HTTP/1.1\r\n"),
        format!("GET /api/v1/sessions/{CHAT_SESSION}?namespace=orka-system HTTP/1.1\r\n"),
    ]) {
        assert!(request.starts_with(&first), "{request}");
    }
    assert_eq!(seen.len(), 3);
}
#[test]
fn chat_identity_mismatch_and_cross_identity_calls_issue_no_mutation() {
    let (dir, mut chat) = chat_kube(1);
    fs::write(dir.path().join("serviceaccount.json"), json!({"metadata":{"name":"bridge-native-run","namespace":"orka-system","labels":{"app.kubernetes.io/managed-by":"bridge-guest"}},"automountServiceAccountToken":false}).to_string()).unwrap();
    let request = chat_request(ChatMethod::Get, format!("tasks/{CHAT_TASK}"));
    assert!(chat.execute_chat(&request).is_err());
    assert!(log(dir.path())
        .iter()
        .all(|l| !l.contains("create") && !l.contains("bridge-native-run")));
    let (run_dir, mut run) = kube(1);
    assert!(run.execute_chat(&request).is_err());
    assert!(log(run_dir.path()).is_empty());
    let (chat_dir, mut fresh_chat) = chat_kube(1);
    assert!(fresh_chat.execute(&get(&format!("tasks/{TASK}"))).is_err());
    assert!(log(chat_dir.path()).is_empty());
    let invalid = chat_request(ChatMethod::Delete, format!("tasks/{TASK}"));
    assert!(fresh_chat.execute_chat(&invalid).is_err());
    assert!(log(chat_dir.path()).is_empty());
}
#[test]
fn chat_delete_is_not_replayed_when_reply_is_lost() {
    let s = server(vec![None]);
    let (_dir, mut chat) = chat_kube(s.port);
    assert!(chat
        .execute_chat(&chat_request(
            ChatMethod::Delete,
            format!("tasks/{CHAT_TASK}")
        ))
        .is_err());
    assert_eq!(s.seen.lock().unwrap().len(), 1);
}
#[test]
fn chat_replies_preserve_acknowledgement_without_claiming_termination() {
    let s = server(vec![
        http("202 Accepted", r#"{"pending":true}"#),
        http("404 Not Found", r#"{"error":"missing"}"#),
    ]);
    let (_dir, mut chat) = chat_kube(s.port);
    let request = chat_request(ChatMethod::Delete, format!("tasks/{CHAT_TASK}"));
    assert_eq!(
        reply(chat.execute_chat(&request).unwrap()),
        OrkaReply {
            status: 202,
            body: json!({"pending":true})
        }
    );
    assert_eq!(
        reply(chat.execute_chat(&request).unwrap()),
        OrkaReply {
            status: 404,
            body: json!({"error":"missing"})
        }
    );
}
#[test]
fn chat_operations_replay_results_without_repeating_delete_and_withhold_unsafe_replies() {
    let exact = format!("{{\"transcript\":\"{}\"}}", "x".repeat(MAX_ORKA_REPLY - 17));
    assert_eq!(exact.len(), MAX_ORKA_REPLY);
    let s = server(vec![
        http("204 No Content", ""),
        http("200 OK", r#"{"echo":"{AUTH}"}"#),
        http("200 OK", &exact),
        http("200 OK", &format!("{exact} ")),
    ]);
    let (_dir, chat) = chat_kube(s.port);
    let root = tempfile::tempdir().unwrap();
    let staging = crate::staging::Staging::new(root.path().join("snapshots")).unwrap();
    let runner = crate::engine::GuestRunner {
        kmx: crate::engine::Kmx,
        orka: Orka::new(installation()),
        chat,
        catalog_provider: None,
    };
    let mut ops = crate::engine::Operations::new(root.path().join("ops"), runner).unwrap();
    let id = Uuid::new_v4();
    let delete = EngineActionWire::OrkaChat {
        request: chat_request(ChatMethod::Delete, format!("tasks/{CHAT_TASK}")),
    };
    for _ in 0..2 {
        assert_eq!(
            ops.execute(id, delete.clone(), None, &staging).unwrap(),
            OperationState::Completed
        );
    }
    assert_eq!(s.seen.lock().unwrap().len(), 1);
    assert_eq!(
        serde_json::from_slice::<OrkaReply>(&ops.read(id, 0).unwrap()).unwrap(),
        OrkaReply {
            status: 204,
            body: Value::Null
        }
    );
    for _ in 0..3 {
        let id = Uuid::new_v4();
        assert_eq!(
            ops.execute(
                id,
                EngineActionWire::OrkaChat {
                    request: chat_request(ChatMethod::Get, format!("sessions/{CHAT_SESSION}"))
                },
                None,
                &staging
            )
            .unwrap(),
            OperationState::Unknown
        );
        assert!(ops.read(id, 0).is_err());
    }
    for entry in walk(&root.path().join("ops")) {
        assert!(!String::from_utf8_lossy(&fs::read(entry).unwrap()).contains("synthetic-bearer"));
    }
    assert_eq!(s.seen.lock().unwrap().len(), 4);
}
