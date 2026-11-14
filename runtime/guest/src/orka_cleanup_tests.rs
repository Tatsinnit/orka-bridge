// Real guest dispatch and kubectl argv/stdin; no Kubernetes or Orka server.
const CLEANUP_AS: &str = "--as=system:serviceaccount:orka-system:bridge-native-run-cleanup";
fn cleanup_list(kind: &str, items: Value) -> Value {
    json!({"apiVersion":if kind == "Pod" {"v1"} else {"core.orka.ai/v1alpha1"},
        "kind":format!("{kind}List"),"metadata":{},"items":items})
}
fn cleanup_agent(uid: &str, name: &str) -> Value {
    json!({"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent",
        "metadata":{"name":name,"namespace":"orka-system","uid":uid},"spec":{}})
}
fn cleanup_task(phase: &str) -> Value {
    json!({"apiVersion":"core.orka.ai/v1alpha1","kind":"Task",
        "metadata":{"name":"any-task","namespace":"orka-system","uid":"task-uid"},
        "spec":{"type":"ai","agentRef":{"name":RUN}},
        "status":{"phase":phase,"jobName":"task-job"}})
}
fn cleanup_pod(phase: &str) -> Value {
    json!({"apiVersion":"v1","kind":"Pod",
        "metadata":{"name":"worker","namespace":"orka-system","uid":"pod-uid"},
        "status":{"phase":phase}})
}
fn cleanup_fixture(tasks: Value) -> (tempfile::TempDir, crate::engine::GuestRunner) {
    let (dir, orka) = kube(1);
    let d = dir.path().display();
    let script = format!(r#"#!/bin/sh
D={d}
printf '%s\n' "$*" >> $D/log
case "$3" in
get)
  case "$4" in
    agents.core.orka.ai)
      if [ -f $D/deleted ]; then cat $D/after.json; else cat $D/agents.json; fi ;;
    tasks.core.orka.ai) cat $D/tasks.json ;;
    pods) cat $D/pods.json ;;
    *) cat $D/$4.json 2>/dev/null || true ;;
  esac ;;
create) cat | tee -a $D/created; echo >> $D/created ;;
delete) cat > $D/delete-options; touch $D/deleted; echo '{{"kind":"Status","status":"Success"}}' ;;
*) exit 42 ;;
esac
"#);
    write_kubectl(&orka.kubectl, &script);
    for (name, value) in [
        ("agents", cleanup_list("Agent", json!([cleanup_agent("recorded-uid", RUN)]))),
        ("tasks", cleanup_list("Task", tasks)),
        ("pods", cleanup_list("Pod", json!([cleanup_pod("Succeeded"), {
            "apiVersion":"v1","kind":"Pod","metadata":{"name":"worker-2","namespace":"orka-system","uid":"pod-2"},
            "status":{"phase":"Failed"}}]))),
        ("after", cleanup_list("Agent", json!([]))),
    ] {
        fs::write(dir.path().join(format!("{name}.json")), value.to_string()).unwrap();
    }
    let runner = crate::engine::GuestRunner { kmx:crate::engine::Kmx, orka,
        chat:Orka::new_chat(installation()), catalog_provider:None };
    (dir, runner)
}
fn cleanup_call(runner: &mut crate::engine::GuestRunner) -> io::Result<bool> {
    use crate::engine::Runner;
    let action: EngineActionWire = serde_json::from_value(json!({"type":"cleanupRunAgent","uid":"recorded-uid"})).unwrap();
    assert!(crate::engine::argv(&action).is_err());
    let result = runner.execute(&action)?;
    assert_eq!(result.exit, 0);
    assert!(!result.has_diagnostics);
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 1);
    Ok(value["removed"].as_bool().unwrap())
}
fn no_cleanup_delete(dir: &Path) {
    assert!(!dir.join("deleted").exists(), "cleanup must not mutate workload resources");
    assert!(!log(dir).iter().any(|s| s.contains(" delete ")));
}
#[test]
fn cleanup_deletes_only_resolved_uid_with_orphan_precondition_and_confirms_absence() {
    let (dir, mut runner) = cleanup_fixture(json!([cleanup_task("Succeeded")]));
    let tasks = fs::read(dir.path().join("tasks.json")).unwrap();
    assert!(cleanup_call(&mut runner).unwrap());
    assert_eq!(fs::read(dir.path().join("tasks.json")).unwrap(), tasks);
    let options: Value = serde_json::from_slice(&fs::read(dir.path().join("delete-options")).unwrap()).unwrap();
    assert_eq!(options, json!({"apiVersion":"v1","kind":"DeleteOptions",
        "preconditions":{"uid":"recorded-uid"},"propagationPolicy":"Orphan"}));
    let commands = log(dir.path());
    let authorized: Vec<_> = commands.iter().filter(|s| s.contains("--as=")).collect();
    let p = format!("{CTX} --namespace=orka-system");
    assert_eq!(authorized, [
        &format!("{p} get agents.core.orka.ai -o json --chunk-size=0 {CLEANUP_AS}"),
        &format!("{p} get tasks.core.orka.ai -o json --chunk-size=0 {CLEANUP_AS}"),
        &format!("{p} get pods --selector=job-name=task-job -o json --chunk-size=0 {CLEANUP_AS}"),
        &format!("{p} delete --raw /apis/core.orka.ai/v1alpha1/namespaces/orka-system/agents/{RUN} -f - {CLEANUP_AS}"),
        &format!("{p} get agents.core.orka.ai -o json --chunk-size=0 {CLEANUP_AS}")]);
    assert_eq!(commands.len(), 11, "six lazy identity commands, five cleanup commands");
    assert!(!commands.iter().any(|s| s.contains("create token") || s.contains("port-forward")));
    assert!(runner.orka.token.is_none() && runner.orka.forward.is_none());
    let created: Vec<Value> = fs::read_to_string(dir.path().join("created")).unwrap()
        .lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(created[1]["rules"], json!([
        {"apiGroups":["core.orka.ai"],"resources":["agents"],"verbs":["get","list","delete"]},
        {"apiGroups":["core.orka.ai"],"resources":["tasks"],"verbs":["get","list"]},
        {"apiGroups":[""],"resources":["pods"],"verbs":["list"]}]));
    for object in created {
        assert_eq!(object["metadata"]["name"], "bridge-native-run-cleanup");
    }
}
#[test]
fn cleanup_accepts_valid_resource_names_longer_than_a_job_label() {
    for length in [64, 253] {
        let name = "a".repeat(length);
        let mut task = cleanup_task("Succeeded");
        task["metadata"]["name"] = json!(name);
        let (dir, mut runner) = cleanup_fixture(json!([task]));
        fs::write(dir.path().join("agents.json"), cleanup_list("Agent", json!([
            cleanup_agent("recorded-uid", RUN), cleanup_agent("unrelated", &name)
        ])).to_string()).unwrap();
        let mut pod = cleanup_pod("Succeeded");
        pod["metadata"]["name"] = json!(name);
        fs::write(dir.path().join("pods.json"), cleanup_list("Pod", json!([pod])).to_string()).unwrap();
        assert!(cleanup_call(&mut runner).unwrap(), "valid resource name of {length} bytes");
    }
}
#[test]
fn cleanup_absent_uid_is_success_even_if_name_recreated_and_never_deletes_replacement() {
    for agents in [json!([]), json!([cleanup_agent("replacement", RUN)])] {
        let (dir, mut runner) = cleanup_fixture(json!([]));
        fs::write(dir.path().join("agents.json"), cleanup_list("Agent", agents).to_string()).unwrap();
        assert!(cleanup_call(&mut runner).unwrap());
        no_cleanup_delete(dir.path());
        assert!(!log(dir.path()).iter().any(|s| s.contains("get tasks")));
    }
}
#[test]
fn cleanup_all_matching_tasks_must_be_terminal_with_an_observable_job() {
    for phase in ["Pending", "Running", "Finalizing", "Scheduled", "", "Unknown", "Future"] {
        let mut task = cleanup_task(phase);
        task["metadata"]["name"] = json!("second-task");
        task["metadata"]["uid"] = json!("second-uid");
        let (dir, mut runner) = cleanup_fixture(json!([cleanup_task("Succeeded"), task]));
        assert!(!cleanup_call(&mut runner).unwrap(), "{phase}");
        no_cleanup_delete(dir.path());
    }
    for phase in ["Succeeded", "Failed", "Cancelled"] {
        let (dir, mut runner) = cleanup_fixture(json!([cleanup_task(phase)]));
        assert!(cleanup_call(&mut runner).unwrap(), "{phase}");
        assert!(dir.path().join("deleted").exists());
    }
    for job in [Value::Null, json!("")] {
        let mut task = cleanup_task("Cancelled");
        task["status"]["jobName"] = job;
        let (dir, mut runner) = cleanup_fixture(json!([task]));
        assert!(!cleanup_call(&mut runner).unwrap());
        no_cleanup_delete(dir.path());
    }
}
#[test]
fn cleanup_observes_effective_reference_namespace_not_task_name_or_prefix() {
    for namespace in [None, Some(""), Some("orka-system")] {
        let mut task = cleanup_task("Running");
        if let Some(namespace) = namespace { task["spec"]["agentRef"]["namespace"] = json!(namespace); }
        let (dir, mut runner) = cleanup_fixture(json!([task]));
        assert!(!cleanup_call(&mut runner).unwrap());
        no_cleanup_delete(dir.path());
    }
    for reference in [json!({"name":RUN,"namespace":"other"}), json!({"name":"other"})] {
        let mut task = cleanup_task("Running");
        task["spec"]["agentRef"] = reference;
        let (_dir, mut runner) = cleanup_fixture(json!([task]));
        assert!(cleanup_call(&mut runner).unwrap());
    }
    let (_dir, mut runner) = cleanup_fixture(json!([]));
    assert!(cleanup_call(&mut runner).unwrap(), "creation before any Task may be reclaimed");
}
#[test]
fn cleanup_live_terminating_unknown_and_missing_phase_pods_block_deletion() {
    for phase in ["Running", "Pending", "Unknown", "", "Future"] {
        let (dir, mut runner) = cleanup_fixture(json!([cleanup_task("Cancelled")]));
        let mut pod = cleanup_pod(phase);
        pod["metadata"]["deletionTimestamp"] = json!("2026-01-01T00:00:00Z");
        fs::write(dir.path().join("pods.json"), cleanup_list("Pod", json!([pod])).to_string()).unwrap();
        assert!(!cleanup_call(&mut runner).unwrap(), "{phase}");
        no_cleanup_delete(dir.path());
    }
}
#[test]
fn cleanup_bad_partial_or_duplicate_lists_never_delete_or_claim_absence() {
    for file in ["agents", "tasks", "pods"] {
        for bad in ["{".to_owned(), "{}".to_owned(),
            json!({"apiVersion":"v1","kind":"List","metadata":{"continue":"next"},"items":[]}).to_string(),
            json!({"apiVersion":"v1","kind":"List","metadata":{"remainingItemCount":1},"items":[]}).to_string(),
            json!({"apiVersion":"v1","kind":"List","metadata":{"continue":true},"items":[]}).to_string(),
            json!({"apiVersion":"v1","kind":"List","metadata":{"remainingItemCount":"0"},"items":[]}).to_string(),
            json!({"apiVersion":"v1","kind":"List","metadata":{},"items":[null]}).to_string()] {
            let (dir, mut runner) = cleanup_fixture(json!([cleanup_task("Succeeded")]));
            fs::write(dir.path().join(format!("{file}.json")), &bad).unwrap();
            assert!(cleanup_call(&mut runner).is_err(), "{file} {bad}");
            no_cleanup_delete(dir.path());
        }
        let item = match file {"agents"=>cleanup_agent("recorded-uid", RUN), "tasks"=>cleanup_task("Succeeded"), _=>cleanup_pod("Succeeded")};
        let (dir, mut runner) = cleanup_fixture(json!([cleanup_task("Succeeded")]));
        fs::write(dir.path().join(format!("{file}.json")), cleanup_list(
            match file {"agents"=>"Agent", "tasks"=>"Task", _=>"Pod"}, json!([item,item])).to_string()).unwrap();
        assert!(cleanup_call(&mut runner).is_err(), "duplicate {file}");
        no_cleanup_delete(dir.path());
    }
}
#[test]
fn cleanup_malformed_agents_tasks_references_and_jobs_fail_closed() {
    for (pointer, value) in [("/metadata/name",json!("not-a-generated-agent")),
        ("/metadata/namespace",json!("other")), ("/metadata/uid",json!(null))] {
        let mut agent = cleanup_agent("recorded-uid", RUN);
        *agent.pointer_mut(pointer).unwrap() = value;
        let (dir, mut runner) = cleanup_fixture(json!([]));
        fs::write(dir.path().join("agents.json"), cleanup_list("Agent",json!([agent])).to_string()).unwrap();
        assert!(cleanup_call(&mut runner).is_err()); no_cleanup_delete(dir.path());
    }
    for (pointer, value) in [("/spec/agentRef",json!("bad")), ("/spec/agentRef/name",json!(null)),
        ("/spec/agentRef",json!({"name":RUN,"namespace":3})), ("/status",json!(false)),
        ("/status/phase",json!(123)), ("/status/jobName",json!("a,b=x")),
        ("/status/jobName",json!("-job")), ("/status/jobName",json!("x".repeat(64)))] {
        let mut task = cleanup_task("Succeeded");
        *task.pointer_mut(pointer).unwrap() = value;
        let (dir, mut runner) = cleanup_fixture(json!([task]));
        assert!(cleanup_call(&mut runner).is_err(), "{pointer}"); no_cleanup_delete(dir.path());
    }
}
#[test]
fn cleanup_post_delete_requires_complete_uid_absence_and_never_replays_delete() {
    for after in ["{".to_owned(), cleanup_list("Agent",json!([cleanup_agent("recorded-uid",RUN)])).to_string(),
        json!({"apiVersion":"v1","kind":"List","metadata":{"continue":"next"},"items":[]}).to_string()] {
        let (dir, mut runner) = cleanup_fixture(json!([]));
        fs::write(dir.path().join("after.json"), after).unwrap();
        assert!(!cleanup_call(&mut runner).unwrap_or(false));
        assert_eq!(log(dir.path()).iter().filter(|s| s.contains(" delete ")).count(),1);
    }
    let (dir, mut runner) = cleanup_fixture(json!([]));
    fs::write(dir.path().join("after.json"),cleanup_list("Agent",json!([cleanup_agent("replacement",RUN)])).to_string()).unwrap();
    assert!(cleanup_call(&mut runner).unwrap());
}
#[test]
fn cleanup_denied_reads_setup_or_uid_conflict_skip_without_retry() {
    for command in ["get serviceaccount", "get agents.core.orka.ai", "get tasks.core.orka.ai", "get pods", "delete --raw"] {
        let (dir, mut runner) = cleanup_fixture(json!([cleanup_task("Failed")]));
        let path = dir.path().join("denied");
        write_kubectl(&path, &format!("#!/bin/sh\ncase \"$3 $4\" in '{command}') exit 1;; esac\nexec '{}' \"$@\"\n", runner.orka.kubectl.display()));
        runner.orka.kubectl = path;
        assert!(cleanup_call(&mut runner).is_err(), "{command}");
        no_cleanup_delete(dir.path());
    }
    let (dir, mut runner) = cleanup_fixture(json!([]));
    fs::write(dir.path().join("role.rbac.authorization.k8s.io.json"),json!({"kind":"Role","rules":[]}).to_string()).unwrap();
    assert!(cleanup_call(&mut runner).is_err()); no_cleanup_delete(dir.path());
}
#[test]
fn cleanup_five_second_budget_includes_lazy_setup_and_kills_its_process_group() {
    let (dir, mut runner) = cleanup_fixture(json!([]));
    let path = dir.path().join("stalled");
    write_kubectl(&path, &format!("#!/bin/sh\nsleep 60 &\necho $! > '{}'\nwait\n",dir.path().join("pid").display()));
    runner.orka.kubectl = path;
    let start = Instant::now();
    assert!(cleanup_call(&mut runner).is_err());
    assert!(start.elapsed() < Duration::from_secs(6));
    gone(&fs::read_to_string(dir.path().join("pid")).unwrap());
    no_cleanup_delete(dir.path());
}
