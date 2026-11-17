use super::*;

async fn connect(api: &Arc<ApiFixture>) -> Result<RemoteAccess, AccessOutcome> {
    let mut client = ClusterClient::new(
        config(&api.start(), json!({"token":"creator-fixture"})),
        Arc::new(PodOrkaRead),
    );
    let id = target(&mut client).await;
    client.connect(id, None).await
}
fn ready(list: &mut Value) -> &mut Value {
    &mut list["items"][1]
}
fn pod_change(pod: &mut Value, change: &str) {
    match change {
        "unowned" => pod["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("ownerReferences")
            .map(drop)
            .unwrap(),
        "owner-api" => pod["metadata"]["ownerReferences"][0]["apiVersion"] = json!("apps/v2"),
        "owner-kind" => pod["metadata"]["ownerReferences"][0]["kind"] = json!("Deployment"),
        "owner-controller" => pod["metadata"]["ownerReferences"][0]["controller"] = json!(false),
        "owner-name" => pod["metadata"]["ownerReferences"][0]["name"] = json!("other"),
        "owner-uid" => pod["metadata"]["ownerReferences"][0]["uid"] = json!("other"),
        "two-controllers" => {
            let owner = pod["metadata"]["ownerReferences"][0].clone();
            pod["metadata"]["ownerReferences"]
                .as_array_mut()
                .unwrap()
                .push(owner);
        }
        "pod-namespace" => pod["metadata"]["namespace"] = json!("other"),
        "pod-uid" => pod["metadata"]["uid"] = json!("replaced"),
        "pod-name" => pod["metadata"]["name"] = json!("replacement"),
        "pod-ready" => pod["status"]["conditions"][0]["status"] = json!("False"),
        "pod-deleting" => pod["metadata"]["deletionTimestamp"] = json!("2026-01-01T00:00:00Z"),
        "pod-phase" => pod["status"]["phase"] = json!("Pending"),
        "pod-port" => pod["spec"]["containers"][0]["ports"][0]["containerPort"] = json!(9090),
        _ => panic!("unknown pod change {change}"),
    }
}
fn change_response(path: &str, list: &mut Value, change: &str) {
    if path.ends_with("/pods") {
        if !change.starts_with("rs-")
            && !change.starts_with("service-")
            && !change.starts_with("deployment-")
        {
            pod_change(ready(list), change);
        }
    } else if path.ends_with("/replicasets") && change.starts_with("rs-") {
        let rs = &mut list["items"][0];
        match change {
            "rs-uid" => rs["metadata"]["uid"] = json!("replacement"),
            "rs-name" => rs["metadata"]["name"] = json!("other"),
            "rs-namespace" => rs["metadata"]["namespace"] = json!("other"),
            "rs-owner-api" => rs["metadata"]["ownerReferences"][0]["apiVersion"] = json!("apps/v2"),
            "rs-owner-kind" => rs["metadata"]["ownerReferences"][0]["kind"] = json!("ReplicaSet"),
            "rs-owner-uid" => rs["metadata"]["ownerReferences"][0]["uid"] = json!("other"),
            "rs-owner-name" => rs["metadata"]["ownerReferences"][0]["name"] = json!("other"),
            "rs-controller" => rs["metadata"]["ownerReferences"][0]["controller"] = json!(false),
            _ => panic!("unknown RS change {change}"),
        }
    } else if path.ends_with("/services") && change.starts_with("service-") {
        match change {
            "service-uid" => list["items"][0]["metadata"]["uid"] = json!("other"),
            "service-name" => list["items"][0]["metadata"]["name"] = json!("other"),
            "service-namespace" => list["items"][0]["metadata"]["namespace"] = json!("other"),
            "service-selector" => {
                list["items"][0]["spec"]["selector"] = json!({"app":"orka", "extra":"label"})
            }
            "service-port" => list["items"][0]["spec"]["ports"][0]["targetPort"] = json!(9090),
            "service-protocol" => list["items"][0]["spec"]["ports"][0]["protocol"] = json!("UDP"),
            _ => panic!("unknown Service change {change}"),
        }
    } else if path.ends_with("/deployments") && change.starts_with("deployment-") {
        match change {
            "deployment-uid" => list["items"][0]["metadata"]["uid"] = json!("other"),
            "deployment-name" => list["items"][0]["metadata"]["name"] = json!("other"),
            "deployment-namespace" => list["items"][0]["metadata"]["namespace"] = json!("other"),
            "deployment-watch" => {
                list["items"][0]["spec"]["template"]["spec"]["containers"][0]["args"] =
                    json!(["--watch-namespace=other"])
            }
            _ => panic!("unknown Deployment change {change}"),
        }
    }
}

// Removing controller-chain validation must fail every case before any HTTP bearer is sent.
#[tokio::test]
async fn bounded_forward_rejects_foreign_or_malformed_owner_chains() {
    for change in [
        "unowned",
        "owner-api",
        "owner-kind",
        "owner-controller",
        "owner-name",
        "owner-uid",
        "two-controllers",
        "pod-namespace",
        "rs-uid",
        "rs-name",
        "rs-namespace",
        "rs-owner-api",
        "rs-owner-kind",
        "rs-owner-uid",
        "rs-owner-name",
        "rs-controller",
    ] {
        let api = Arc::new(ApiFixture::default());
        *api.response_hook.lock().unwrap() = Some(Box::new(move |path, list| {
            change_response(path, list, change)
        }));
        assert_eq!(
            connect(&api).await.err(),
            Some(failed(Check::OrkaConnection, CheckResult::IdentityRejected)),
            "{change}"
        );
        assert!(api.wire.calls.lock().unwrap().is_empty(), "{change}");
        assert_eq!(api.wire.forwards.load(Ordering::SeqCst), 0, "{change}");
    }
}

#[tokio::test]
async fn bounded_forward_no_ready_pod_is_unavailable_not_an_identity_mismatch() {
    for mode in ["empty", "unready", "terminating"] {
        let api = Arc::new(ApiFixture::default());
        *api.response_hook.lock().unwrap() = Some(Box::new(move |path, list| {
            if path.ends_with("/pods") {
                match mode {
                    "empty" => list["items"] = json!([]),
                    "terminating" => pod_change(ready(list), "pod-deleting"),
                    _ => ready(list)["status"]["conditions"][0]["status"] = json!("False"),
                }
            }
        }));
        assert_eq!(
            connect(&api).await.err(),
            Some(failed(Check::OrkaConnection, CheckResult::Unknown))
        );
        assert_eq!(api.wire.forwards.load(Ordering::SeqCst), 0);
        assert!(api.wire.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn bounded_forward_skips_foreign_pod_before_valid_rollout() {
    let api = Arc::new(ApiFixture::default());
    *api.response_hook.lock().unwrap() = Some(Box::new(|path, list| {
        if path.ends_with("/pods") {
            let mut foreign = ready(list).clone();
            foreign["metadata"]["name"] = json!("foreign");
            foreign["metadata"]["uid"] = json!("foreign-uid");
            foreign["metadata"]["ownerReferences"][0]["uid"] = json!("foreign-rs");
            // Deliberately keep both candidates: the production selector must skip the foreign one.
            list["items"].as_array_mut().unwrap().insert(0, foreign);
        }
    }));
    let (client, access) = access(&api).await;
    let transport = access
        .transport(&client, access.providers()[0].id)
        .await
        .unwrap();
    assert_eq!(transport.peer().pod_uid, "pod-uid");
    transport.close().await;
}

// A named port can change in a valid rollout; do not pin to discovery's old 8080.
#[tokio::test]
async fn bounded_forward_accepts_rollout_replicaset_and_named_9090() {
    let api = Arc::new(ApiFixture::default());
    *api.response_hook.lock().unwrap() = Some(Box::new(|path, list| {
        if path.ends_with("/pods") {
            let pod = ready(list);
            pod["metadata"]["ownerReferences"][0]["name"] = json!("controller-new");
            pod["metadata"]["ownerReferences"][0]["uid"] = json!("new-rs");
            pod_change(pod, "pod-port");
        } else if path.ends_with("/replicasets") {
            list["items"][0]["metadata"]["name"] = json!("controller-new");
            list["items"][0]["metadata"]["uid"] = json!("new-rs");
        }
    }));
    let transport = transport(&api).await;
    assert_eq!(transport.peer().port, 9090);
    transport.close().await;
    let queries = api.queries.lock().unwrap();
    let rs: Vec<_> = queries
        .iter()
        .filter(|q| q.contains("/replicasets?"))
        .collect();
    assert!(!rs.is_empty(), "must read the live namespaced ReplicaSet");
    assert!(rs.iter().all(
        |q| q.starts_with("/apis/apps/v1/namespaces/control/replicasets?")
            && q.contains("fieldSelector=metadata.name%3Dcontroller-new")
    ));
    assert!(
        !queries.iter().any(|q| q.contains("/replicasets/")),
        "no new get permission"
    );
}

#[tokio::test]
async fn bounded_forward_preopen_rechecks_frozen_candidate() {
    for change in [
        "pod-uid",
        "pod-name",
        "pod-ready",
        "pod-deleting",
        "pod-phase",
        "pod-port",
        "service-selector",
        "service-port",
        "service-protocol",
        "service-namespace",
        "deployment-uid",
        "deployment-watch",
    ] {
        let api = Arc::new(ApiFixture::default());
        let mut pod_lists = 0;
        *api.response_hook.lock().unwrap() = Some(Box::new(move |path, list| {
            if path.ends_with("/pods") {
                pod_lists += 1;
            }
            // Service/Deployment are read before Pods on each pass.
            if pod_lists >= 1 && (!path.ends_with("/pods") || pod_lists >= 2) {
                change_response(path, list, change);
            }
        }));
        assert_eq!(
            connect(&api).await.err(),
            Some(failed(Check::OrkaConnection, CheckResult::IdentityRejected)),
            "{change}"
        );
        assert!(api.wire.calls.lock().unwrap().is_empty(), "{change}");
        assert_eq!(api.wire.forwards.load(Ordering::SeqCst), 0, "{change}");
    }
}

#[tokio::test]
async fn bounded_forward_postupgrade_rejects_changes_without_http_and_aborts_forward() {
    for change in [
        "pod-uid",
        "pod-name",
        "pod-namespace",
        "pod-ready",
        "pod-port",
        "owner-uid",
        "rs-uid",
        "rs-owner-uid",
        "service-uid",
        "service-name",
        "service-namespace",
        "service-selector",
        "service-port",
        "service-protocol",
        "deployment-uid",
        "deployment-name",
        "deployment-namespace",
        "deployment-watch",
    ] {
        let api = Arc::new(ApiFixture::default());
        let weak = Arc::downgrade(&api);
        *api.wire.before_upgrade.lock().unwrap() = Some(Box::new(move || {
            let api = weak.upgrade().unwrap();
            *api.response_hook.lock().unwrap() = Some(Box::new(move |path, list| {
                change_response(path, list, change)
            }));
        }));
        assert_eq!(
            connect(&api).await.err(),
            Some(failed(Check::OrkaConnection, CheckResult::IdentityRejected)),
            "{change}"
        );
        until(|| api.wire.closed.load(Ordering::SeqCst) == 1).await;
        assert_eq!(
            api.wire.forwards.load(Ordering::SeqCst),
            1,
            "no replay: {change}"
        );
        assert_eq!(api.wire.active.load(Ordering::SeqCst), 0, "{change}");
        assert!(
            api.wire.calls.lock().unwrap().is_empty(),
            "no HTTP bearer after rejection: {change}"
        );
    }
}

#[tokio::test]
async fn bounded_forward_cancelled_postupgrade_check_aborts_driver_without_http() {
    let api = Arc::new(ApiFixture::default());
    let weak = Arc::downgrade(&api);
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    *api.wire.before_upgrade.lock().unwrap() = Some(Box::new(move || {
        let mut entered = Some(entered);
        *weak.upgrade().unwrap().response_hook.lock().unwrap() = Some(Box::new(move |path, _| {
            if path.ends_with("/services") {
                if let Some(entered) = entered.take() {
                    let _ = entered.send(());
                    let _ = gate.recv();
                }
            }
        }));
    }));
    let opening = tokio::spawn({
        let api = api.clone();
        async move { connect(&api).await }
    });
    tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .unwrap()
        .unwrap();
    opening.abort();
    assert!(matches!(opening.await, Err(e) if e.is_cancelled()));
    until(|| api.wire.closed.load(Ordering::SeqCst) == 1).await;
    assert!(api.wire.calls.lock().unwrap().is_empty());
    assert_eq!(api.wire.active.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
}

#[tokio::test]
async fn bounded_forward_send_replicaset_denial_does_not_open_another_stream() {
    let api = Arc::new(ApiFixture::default());
    let (client, access) = access(&api).await;
    let before = api.wire.forwards.load(Ordering::SeqCst);
    api.wire.calls.lock().unwrap().clear();
    *api.override_response.lock().unwrap() = Some((
        "/replicasets".into(),
        403,
        json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Fixture","message":"creator-fixture","code":403}),
    ));
    assert_eq!(
        access
            .transport(&client, access.providers()[0].id)
            .await
            .err(),
        Some(ClusterError::Denied)
    );
    assert_eq!(api.wire.forwards.load(Ordering::SeqCst), before);
    assert!(api.wire.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn bounded_forward_replicaset_denial_is_kubernetes_preparation_failure() {
    for (code, result) in [
        (401, CheckResult::AuthenticationRejected),
        (403, CheckResult::PermissionDenied),
    ] {
        let api = Arc::new(ApiFixture::default());
        *api.override_response.lock().unwrap() = Some((
            "/replicasets".into(),
            code,
            json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Fixture","message":"creator-fixture","code":code}),
        ));
        assert_eq!(
            connect(&api).await.err(),
            Some(failed(Check::OrkaConnection, result))
        );
        assert!(api.wire.calls.lock().unwrap().is_empty());
    }
}
