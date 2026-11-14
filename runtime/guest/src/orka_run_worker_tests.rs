// External processes and HTTP are fixtures; validation, identity and dispatch are real.
fn worker_task(phase: &str, job: &str) -> Value {
    json!({"apiVersion":"core.orka.ai/v1alpha1","kind":"Task",
        "metadata":{"name":TASK,"namespace":"orka-system","uid":"ack-uid"},
        "status":{"phase":phase,"jobName":job}})
}
fn worker_pods(items: Value) -> Value {
    json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":items})
}
fn worker_fixture(task: &Value, pods: &Value) -> (tempfile::TempDir, Orka) {
    let (dir, orka) = kube(1);
    fs::write(dir.path().join("tasks.core.orka.ai.json"), task.to_string()).unwrap();
    fs::write(dir.path().join("pods.json"), pods.to_string()).unwrap();
    (dir, orka)
}
#[test]
fn run_cancel_sends_exact_uid_post_and_preserves_success_conflict_and_denial() {
    let s = server(vec![http("200 OK", r#"{"requested":true}"#),
        http("409 Conflict", r#"{"error":"finished"}"#), http("403 Forbidden", r#"{}"#)]);
    let (_dir, mut orka) = kube(s.port);
    let request = OrkaRequest { path:format!("tasks/{TASK}/cancel"), after:None,
        body:Some(json!({"uid":"ack-uid"})) };
    for status in [200, 409, 403] {
        assert_eq!(reply(orka.execute(&request).unwrap()).status, status);
    }
    let seen = s.seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "one attempt per cancel, no implicit retry");
    for req in seen.iter() {
        assert!(req.starts_with(&format!("POST /api/v1/tasks/{TASK}/cancel?namespace=orka-system HTTP/1.1\r\n")));
        assert!(req.ends_with(r#"{"uid":"ack-uid"}"#));
    }
    let chat_rules = &manifests(CHAT_IDENTITY)[1].1["rules"];
    assert!(!chat_rules.to_string().contains("tasks/cancel"));
    assert!(!chat_rules.to_string().contains("pods"));
}
#[test]
fn worker_read_counts_only_unfinished_pods_and_dispatches_as_run_without_a_bearer() {
    use crate::engine::{GuestRunner, Kmx, Runner};
    let (dir, orka) = worker_fixture(&worker_task("Cancelled", "task-job"), &worker_pods(json!([
        {"status":{"phase":"Succeeded"}}, {"status":{"phase":"Failed"}},
        {"status":{"phase":"Running"},"metadata":{"deletionTimestamp":"now"}},
        {"status":{"phase":"Pending"}}, {"status":{"phase":"Unknown"}},
        {"status":{"phase":"FuturePhase"}}, {}])));
    let action = EngineActionWire::RunWorker {name:TASK.into(), uid:"ack-uid".into()};
    let mut runner = GuestRunner {kmx:Kmx, orka, chat:Orka::new_chat(installation()), catalog_provider:None};
    let result = runner.execute(&action).unwrap();
    assert_eq!(result.exit, 0);
    assert_eq!(serde_json::from_slice::<Value>(&result.stdout).unwrap(), json!({"finished":true,"pods":5}));
    assert!(!result.has_diagnostics);
    assert!(crate::engine::argv(&action).is_err());
    let commands = log(dir.path());
    let reads: Vec<_> = commands.iter().filter(|s| s.contains("--as=")).collect();
    let prefix = format!("{CTX} --namespace=orka-system");
    assert_eq!(reads, [&format!("{prefix} get tasks.core.orka.ai {TASK} -o json --as=system:serviceaccount:orka-system:bridge-native-run"),
        &format!("{prefix} get pods --selector=job-name=task-job -o json --chunk-size=0 --as=system:serviceaccount:orka-system:bridge-native-run")]);
    assert_eq!(commands.len(), 8, "six identity setup commands, then two impersonated reads");
    assert!(commands[..6].iter().all(|s| !s.contains("--as=")));
    assert!(!commands.iter().any(|s| s.contains("create token") || s.contains("port-forward")));
    assert!(runner.orka.token.is_none() && runner.orka.forward.is_none());
}
#[test]
fn worker_finished_is_task_terminal_not_phase_cancelled_or_absence_of_pods() {
    for (phase, finished) in [("Succeeded", true), ("Failed", true), ("Cancelled", true),
        ("Pending", false), ("Running", false), ("Finalizing", false), ("Scheduled", false), ("", false)] {
        for job in ["task-job", ""] {
            let (dir, mut orka) = worker_fixture(&worker_task(phase, job), &worker_pods(json!([
                {"status":{"phase":"Succeeded"}}, {"status":{"phase":"Failed"}}])));
            let result = orka.run_worker(TASK, "ack-uid").unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&result.stdout).unwrap(), json!({"finished":finished,"pods":0}), "{phase} {job}");
            assert_eq!(log(dir.path()).iter().any(|s| s.contains("get pods")), !job.is_empty());
        }
    }
}
#[test]
fn worker_refuses_invalid_input_and_chat_identity_before_running_kubectl() {
    let (dir, mut orka) = kube(1);
    for (name, uid) in [("--all", "ack-uid"), (TASK, ""), (TASK, &"x".repeat(65))] {
        assert!(orka.run_worker(name, uid).is_err());
    }
    orka.identity = CHAT_IDENTITY;
    assert!(orka.run_worker(TASK, "ack-uid").is_err());
    assert!(log(dir.path()).is_empty());
}
#[test]
fn worker_uid_missing_replaced_and_malformed_task_never_reach_pod_list() {
    for task in [json!({}), json!({"metadata":{"uid":"replacement"}}),
        json!({"metadata":{"uid":123}}), json!({"metadata":{"uid":""}}),
        json!({"metadata":{"uid":"ack-uid"},"status":"bad"}),
        worker_task("FuturePhase", "task-job"), worker_task("Cancelled", "a,b=x"),
        worker_task("Cancelled", "-job"), worker_task("Cancelled", &"x".repeat(64)),
        json!({"metadata":{"uid":"ack-uid"},"status":{"jobName":123}}),
        json!({"metadata":{"uid":"ack-uid"},"status":{"phase":false}})] {
        let (dir, mut orka) = worker_fixture(&task, &worker_pods(json!([])));
        assert!(orka.run_worker(TASK, "ack-uid").is_err(), "{task}");
        assert!(!log(dir.path()).iter().any(|s| s.contains("get pods")));
    }
}
#[test]
fn worker_partial_and_malformed_pod_lists_never_prove_zero() {
    for pods in [json!({}), json!({"items":null}), worker_pods(json!([null])),
        json!({"metadata":{"continue":"next"},"items":[]}),
        json!({"metadata":{"remainingItemCount":1},"items":[]}),
        json!({"metadata":{"continue":true},"items":[]}),
        json!({"metadata":{"remainingItemCount":"unknown"},"items":[]})] {
        let (_dir, mut orka) = worker_fixture(&worker_task("Cancelled", "task-job"), &pods);
        assert!(orka.run_worker(TASK, "ack-uid").is_err(), "{pods}");
    }
}
#[test]
fn worker_command_failures_and_bad_json_are_not_zero_counts() {
    for file in ["tasks.core.orka.ai.json", "pods.json"] {
        let (dir, mut orka) = worker_fixture(&worker_task("Cancelled", "task-job"), &worker_pods(json!([])));
        fs::write(dir.path().join(file), "not-json").unwrap();
        assert!(orka.run_worker(TASK, "ack-uid").is_err());
    }
    for resource in ["tasks.core.orka.ai", "pods"] {
        let (dir, mut orka) = worker_fixture(&worker_task("Cancelled", "task-job"), &worker_pods(json!([])));
        let script = format!("#!/bin/sh\ncase \"$4\" in {resource}) exit 1;; esac\nexec '{}' \"$@\"\n", orka.kubectl.display());
        let path = dir.path().join("denied");
        write_kubectl(&path, &script);
        orka.kubectl = path;
        assert!(orka.run_worker(TASK, "ack-uid").is_err());
    }
}
