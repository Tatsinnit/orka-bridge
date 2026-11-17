use super::*;

fn receipt() -> Receipt {
    Receipt {
        version: 1,
        environment_id: Uuid::new_v4(),
        workspace_id: Uuid::new_v4(),
        target: Target::from_config("team", &config()).unwrap(),
        namespace: "creator-one".into(),
        image: format!("ghcr.io/example/environment@sha256:{}", "a".repeat(64)),
        deployment_uid: None,
        pvc_uid: None,
        policy_uid: None,
    }
}

pub(super) fn config() -> Config {
    Config::new("https://cluster.example:6443".parse().unwrap())
}

#[test]
fn hardened_templates_keep_a_stopped_deployment_and_private_volume() {
    let r = receipt();
    let [pvc, policy, deployment] = resources::manifests(&r).unwrap();
    assert_eq!(
        pvc["spec"]["accessModes"],
        serde_json::json!(["ReadWriteOnce"])
    );
    assert_eq!(pvc["spec"]["resources"]["requests"]["storage"], "10Gi");
    assert!(pvc["metadata"].get("ownerReferences").is_none());
    assert_eq!(deployment["spec"]["replicas"], 0);
    assert_eq!(deployment["spec"]["strategy"]["type"], "Recreate");
    let pod = &deployment["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["enableServiceLinks"], false);
    assert_eq!(pod["securityContext"]["fsGroup"], 1000);
    assert_eq!(
        pod["containers"][0]["securityContext"]["readOnlyRootFilesystem"],
        true
    );
    assert_eq!(policy["spec"]["ingress"], serde_json::json!([]));
    assert_eq!(policy["spec"]["egress"][1]["ports"][0]["port"], 443);
    assert_ne!(r.resource_name(), receipt().resource_name());
}

#[test]
fn policy_exceptions_are_strict_subnets_in_kubernetes_address_family() {
    use std::net::IpAddr;
    fn cidr(text: &str) -> (IpAddr, u32) {
        let (ip, prefix) = text.split_once('/').unwrap();
        (
            ip.parse::<IpAddr>().unwrap().to_canonical(),
            prefix.parse().unwrap(),
        )
    }
    let policy = resources::manifests(&receipt()).unwrap()[1].clone();
    for peer in policy["spec"]["egress"][1]["to"].as_array().unwrap() {
        let block = &peer["ipBlock"];
        let (base, prefix) = cidr(block["cidr"].as_str().unwrap());
        for except in block["except"].as_array().unwrap() {
            let (ip, mask) = cidr(except.as_str().unwrap());
            // Kubernetes ValidateIPBlock uses Go net.IPNet.Contains, which canonicalizes mapped IPv4.
            assert_eq!(
                base.is_ipv4(),
                ip.is_ipv4(),
                "{except} is not in {}",
                block["cidr"]
            );
            let width = if base.is_ipv4() { 32 } else { 128 };
            assert!(mask > prefix && mask <= width);
            let bits = |ip| match ip {
                IpAddr::V4(v) => u32::from(v) as u128,
                IpAddr::V6(v) => u128::from(v),
            };
            let netmask = if prefix == 0 {
                0
            } else {
                u128::MAX << (width - prefix)
            };
            assert_eq!(bits(base) & netmask, bits(ip) & netmask);
        }
    }
}

#[test]
fn receipt_survives_reload_without_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let r = receipt();
    let s = store::Store::new(dir.path()).unwrap();
    s.save(&r).unwrap();
    drop(s);
    let s = store::Store::new(dir.path()).unwrap();
    let loaded = s.load(r.environment_id).unwrap();
    assert_eq!(loaded.target, r.target);
    assert_eq!(loaded.workspace_id, r.workspace_id);
    assert!(loaded.deployment_uid.is_none());
}

#[tokio::test]
async fn create_records_uids_before_any_pod_can_start_and_is_once_per_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    assert!(r.deployment_uid.is_some() && r.pvc_uid.is_some() && r.policy_uid.is_some());
    let again = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    assert_eq!(r.environment_id, again.environment_id);
    let s = cluster.lock().unwrap();
    let creates: Vec<_> = s
        .requests
        .iter()
        .filter(|(method, path, _)| method == "POST" && !path.ends_with("selfsubjectaccessreviews"))
        .collect();
    assert_eq!(creates.len(), 3);
    assert!(creates[0].1.ends_with("persistentvolumeclaims"));
    assert!(creates[1].1.ends_with("networkpolicies"));
    assert_eq!(creates[2].2["spec"]["replicas"], 0);
}

#[tokio::test]
async fn denied_preflight_and_invalid_input_make_no_resource_writes() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    cluster.lock().unwrap().denied = true;
    let r = receipt();
    assert!(matches!(
        owner
            .create(&c, r.workspace_id, &r.namespace, &r.image)
            .await,
        Err(Error::Denied)
    ));
    assert!(cluster.lock().unwrap().objects.is_empty());
    cluster.lock().unwrap().requests.clear();
    assert!(matches!(
        owner
            .create(&c, r.workspace_id, "../foreign", &r.image)
            .await,
        Err(Error::Invalid)
    ));
    assert!(cluster.lock().unwrap().requests.is_empty());
}

#[tokio::test]
async fn lifecycle_reloads_receipt_without_orka_and_retains_volume_until_remove() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    drop(owner);
    let owner = Owner::new(dir.path().into());
    owner.start(&c, r.environment_id).await.unwrap();
    owner.stop(&c, r.environment_id).await.unwrap();
    assert_eq!(cluster.lock().unwrap().objects.len(), 3);
    assert_eq!(owner.receipts().unwrap().len(), 1);
    owner.remove(&c, r.environment_id).await.unwrap();
    assert!(cluster.lock().unwrap().objects.is_empty());
    assert!(owner.receipts().unwrap().is_empty());
    let s = cluster.lock().unwrap();
    for (method, _, body) in &s.requests {
        if method == "PATCH" {
            assert!(
                body["metadata"]["uid"].is_string()
                    && body["metadata"]["resourceVersion"].is_string()
            );
        }
        if method == "DELETE" {
            assert!(body["preconditions"]["uid"].is_string());
        }
    }
}

#[tokio::test]
async fn lost_create_response_leaves_a_stopped_unmanaged_conflict_not_an_adoption() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    cluster.lock().unwrap().fail_create = Some("deployments".into());
    let input = receipt();
    let error = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap_err();
    assert_eq!(error, Error::Unavailable);
    assert!(!format!("{error:?}").contains("fixture-secret"));
    let r = owner.receipts().unwrap().pop().unwrap();
    assert!(r.deployment_uid.is_none() && r.pvc_uid.is_some());
    cluster.lock().unwrap().fail_create = None;
    assert!(matches!(
        owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await,
        Err(Error::Conflict)
    ));
    let views = owner.discover(&c, &input.namespace).await.unwrap();
    assert_eq!(views.len(), 1);
    assert!(!views[0].managed);
    assert!(cluster
        .lock()
        .unwrap()
        .objects
        .values()
        .filter(|v| v["kind"] == "Deployment")
        .all(|v| v["spec"]["replicas"] == 0));
}

#[tokio::test]
async fn missing_receipt_is_discovery_only_and_changed_target_cannot_mutate() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (mut c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    let second_install = Owner::new(other.path().into());
    assert!(!second_install.discover(&c, &r.namespace).await.unwrap()[0].managed);
    assert!(second_install.stop(&c, r.environment_id).await.is_err());
    c.target = Target::from_config("different-context", &config()).unwrap();
    cluster.lock().unwrap().requests.clear();
    assert_eq!(
        owner.stop(&c, r.environment_id).await,
        Err(Error::TargetChanged)
    );
    assert!(cluster.lock().unwrap().requests.is_empty());
}

#[tokio::test]
async fn replacement_between_read_and_patch_is_not_changed() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    cluster.lock().unwrap().replace_before_patch = true;
    assert_eq!(
        owner.start(&c, r.environment_id).await,
        Err(Error::Conflict)
    );
    assert!(cluster
        .lock()
        .unwrap()
        .objects
        .values()
        .filter(|v| v["kind"] == "Deployment")
        .all(|v| v["spec"]["replicas"] == 0));
}

#[tokio::test]
async fn terminating_pvc_keeps_receipt_and_remove_can_retry() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    cluster.lock().unwrap().terminating = Some("persistentvolumeclaims".into());
    assert_eq!(
        owner.remove(&c, r.environment_id).await,
        Err(Error::Pending)
    );
    assert_eq!(owner.receipts().unwrap().len(), 1);
    cluster.lock().unwrap().objects.clear();
    owner.remove(&c, r.environment_id).await.unwrap();
    assert!(owner.receipts().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn stop_waits_for_pod_owner_chain_not_just_labels_and_never_force_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    let rs = serde_json::json!({"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{
        "name":"owned-rs","namespace":r.namespace,"uid":"rs-uid","ownerReferences":[{
            "apiVersion":"apps/v1","kind":"Deployment","name":r.resource_name(),"uid":r.deployment_uid,"controller":true}]}});
    let pod = serde_json::json!({"apiVersion":"v1","kind":"Pod","metadata":{
        "name":"owned-pod","namespace":r.namespace,"uid":"pod-uid","ownerReferences":[{
            "apiVersion":"apps/v1","kind":"ReplicaSet","name":"owned-rs","uid":"rs-uid","controller":true}]}});
    {
        let mut s = cluster.lock().unwrap();
        s.objects.insert(
            format!(
                "/apis/apps/v1/namespaces/{}/replicasets/owned-rs",
                r.namespace
            ),
            rs,
        );
        s.objects.insert(
            format!("/api/v1/namespaces/{}/pods/owned-pod", r.namespace),
            pod,
        );
    }
    assert_eq!(owner.stop(&c, r.environment_id).await, Err(Error::Pending));
    let s = cluster.lock().unwrap();
    assert!(!s.requests.iter().any(|(m, _, _)| m == "DELETE"));
    assert!(s
        .requests
        .iter()
        .any(|(_, p, _)| p.ends_with("replicasets")));
}

#[tokio::test(start_paused = true)]
async fn orphaned_pods_keep_stop_pending_and_keep_the_network_policy() {
    for variant in ["orphaned-rs", "ownerless-labelled", "ownerless-pvc"] {
        let dir = tempfile::tempdir().unwrap();
        let owner = Owner::new(dir.path().into());
        let (c, cluster) = fixture::connection();
        let input = receipt();
        let r = owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await
            .unwrap();
        owner.start(&c, r.environment_id).await.unwrap();
        let mut pod = serde_json::json!({"apiVersion":"v1","kind":"Pod","metadata":{
            "name":"orphan","namespace":r.namespace,"uid":"orphan-uid"}});
        if variant == "ownerless-pvc" {
            pod["spec"] = serde_json::json!({"volumes":[{"name":"home","persistentVolumeClaim":{"claimName":r.resource_name()}}]});
        } else {
            pod["metadata"]["labels"] =
                serde_json::json!({resources::ENVIRONMENT:r.environment_id.to_string()});
        }
        {
            let mut s = cluster.lock().unwrap();
            s.objects.retain(|_, v| v["kind"] != "Deployment");
            if variant == "orphaned-rs" {
                pod["metadata"]["ownerReferences"] = serde_json::json!([{"apiVersion":"apps/v1","kind":"ReplicaSet","name":"orphan-rs","uid":"rs-uid","controller":true}]);
                s.objects.insert(format!("/apis/apps/v1/namespaces/{}/replicasets/orphan-rs",r.namespace),serde_json::json!({"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{"name":"orphan-rs","namespace":r.namespace,"uid":"rs-uid"}}));
            }
            s.objects.insert(
                format!("/api/v1/namespaces/{}/pods/orphan", r.namespace),
                pod,
            );
            s.requests.clear();
        }
        assert_eq!(
            owner.stop(&c, r.environment_id).await,
            Err(Error::Pending),
            "{variant}"
        );
        assert_eq!(
            owner.remove(&c, r.environment_id).await,
            Err(Error::Pending),
            "{variant}"
        );
        assert!(!cluster
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, _, _)| m == "DELETE"));
        assert_eq!(owner.receipts().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn changing_target_or_namespace_does_not_duplicate_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (mut c, cluster) = fixture::connection();
    let input = receipt();
    owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    assert!(matches!(
        owner
            .create(&c, input.workspace_id, "different-namespace", &input.image)
            .await,
        Err(Error::Conflict)
    ));
    c.target = Target::from_config("renamed-context", &config()).unwrap();
    assert!(matches!(
        owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await,
        Err(Error::TargetChanged)
    ));
    assert_eq!(owner.receipts().unwrap().len(), 1);
    assert_eq!(cluster.lock().unwrap().objects.len(), 3);
}

#[tokio::test]
async fn create_does_not_rebuild_around_a_terminating_volume() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    cluster.lock().unwrap().terminating = Some("persistentvolumeclaims".into());
    assert_eq!(
        owner.remove(&c, r.environment_id).await,
        Err(Error::Pending)
    );
    cluster.lock().unwrap().requests.clear();
    assert!(matches!(
        owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await,
        Err(Error::Pending)
    ));
    assert_eq!(cluster.lock().unwrap().objects.len(), 1);
    assert!(!cluster
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, p, _)| m == "POST" && !p.ends_with("selfsubjectaccessreviews")));
}

#[tokio::test]
async fn remove_rechecks_stopped_replicas_before_deleting_the_deployment() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    cluster.lock().unwrap().restart_after_stop = true;
    assert_eq!(
        owner.remove(&c, r.environment_id).await,
        Err(Error::Conflict)
    );
    assert_eq!(cluster.lock().unwrap().objects.len(), 3);
}

#[tokio::test]
async fn start_rejects_policy_selectors_that_exclude_the_environment() {
    for variant in ["label", "expression", "port-range"] {
        let dir = tempfile::tempdir().unwrap();
        let owner = Owner::new(dir.path().into());
        let (c, cluster) = fixture::connection();
        let input = receipt();
        let r = owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await
            .unwrap();
        {
            let mut s = cluster.lock().unwrap();
            let policy = s
                .objects
                .values_mut()
                .find(|v| v["kind"] == "NetworkPolicy")
                .unwrap();
            if variant == "label" {
                policy["spec"]["podSelector"]["matchLabels"]["unmatched"] =
                    serde_json::json!("extra");
            } else if variant == "expression" {
                policy["spec"]["podSelector"]["matchExpressions"] =
                    serde_json::json!([{"key":"unmatched","operator":"Exists"}]);
            } else {
                policy["spec"]["egress"][1]["ports"][0]["endPort"] = serde_json::json!(65535);
            }
            s.requests.clear();
        }
        assert_eq!(
            owner.start(&c, r.environment_id).await,
            Err(Error::Conflict),
            "{variant}"
        );
        assert!(!cluster
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, _, _)| m == "PATCH"));
    }
}

#[tokio::test(start_paused = true)]
async fn stop_waits_for_replica_set_scale_down_and_remove_waits_for_child_gc() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    let key = format!(
        "/apis/apps/v1/namespaces/{}/replicasets/owned-rs",
        r.namespace
    );
    cluster.lock().unwrap().objects.insert(key.clone(),serde_json::json!({"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{
        "name":"owned-rs","namespace":r.namespace,"uid":"rs-uid","generation":1,"ownerReferences":[{
            "apiVersion":"apps/v1","kind":"Deployment","name":r.resource_name(),"uid":r.deployment_uid,"controller":true}]},
        "spec":{"replicas":1},"status":{"replicas":0,"observedGeneration":1}}));
    assert_eq!(owner.stop(&c, r.environment_id).await, Err(Error::Pending));
    assert!(!cluster
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, _, _)| m == "DELETE"));
    cluster.lock().unwrap().objects.get_mut(&key).unwrap()["spec"]["replicas"] =
        serde_json::json!(0);
    owner.stop(&c, r.environment_id).await.unwrap();
    assert_eq!(
        owner.remove(&c, r.environment_id).await,
        Err(Error::Pending)
    );
    assert!(cluster
        .lock()
        .unwrap()
        .objects
        .values()
        .any(|v| v["kind"] == "NetworkPolicy"));
    cluster.lock().unwrap().objects.remove(&key);
    owner.remove(&c, r.environment_id).await.unwrap();
    assert!(owner.receipts().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn only_conclusively_terminated_pods_can_be_ignored_by_stop() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    let key = format!("/api/v1/namespaces/{}/pods/terminal", r.namespace);
    cluster.lock().unwrap().objects.insert(key.clone(),serde_json::json!({"apiVersion":"v1","kind":"Pod","metadata":{
        "name":"terminal","namespace":r.namespace,"uid":"pod-uid","labels":{resources::ENVIRONMENT:r.environment_id.to_string()}},
        "spec":{"containers":[{"name":"environment"}]},"status":{"phase":"Failed"}}));
    assert_eq!(owner.stop(&c, r.environment_id).await, Err(Error::Pending));
    cluster.lock().unwrap().objects.get_mut(&key).unwrap()["status"]["containerStatuses"] = serde_json::json!([
        {"name":"environment","state":{"terminated":{"exitCode":1}}}]);
    owner.stop(&c, r.environment_id).await.unwrap();
    assert!(!cluster
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, _, _)| m == "DELETE"));
}

#[tokio::test]
async fn an_unrelated_pod_with_a_reused_replica_set_name_does_not_block_stop() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    {
        let mut s = cluster.lock().unwrap();
        s.objects.insert(format!("/api/v1/namespaces/{}/pods/unrelated",r.namespace),serde_json::json!({"apiVersion":"v1","kind":"Pod","metadata":{
            "name":"unrelated","namespace":r.namespace,"uid":"unrelated","ownerReferences":[{
                "apiVersion":"apps/v1","kind":"ReplicaSet","name":"reused-rs","uid":"old-uid","controller":true}]}}));
        s.objects.insert(
            format!(
                "/apis/apps/v1/namespaces/{}/replicasets/reused-rs",
                r.namespace
            ),
            serde_json::json!({"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{
            "name":"reused-rs","namespace":r.namespace,"uid":"new-uid"}}),
        );
    }
    owner.stop(&c, r.environment_id).await.unwrap();
}

#[tokio::test]
async fn start_rejects_container_security_overrides() {
    for variant in ["root", "seccomp", "capability", "privileged", "proc-mount"] {
        let dir = tempfile::tempdir().unwrap();
        let owner = Owner::new(dir.path().into());
        let (c, cluster) = fixture::connection();
        let input = receipt();
        let r = owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await
            .unwrap();
        {
            let mut s = cluster.lock().unwrap();
            let deployment = s
                .objects
                .values_mut()
                .find(|v| v["kind"] == "Deployment")
                .unwrap();
            let security =
                &mut deployment["spec"]["template"]["spec"]["containers"][0]["securityContext"];
            match variant {
                "root" => {
                    security["runAsUser"] = serde_json::json!(0);
                    security["runAsNonRoot"] = serde_json::json!(false);
                }
                "seccomp" => security["seccompProfile"] = serde_json::json!({"type":"Unconfined"}),
                "capability" => security["capabilities"]["add"] = serde_json::json!(["SYS_ADMIN"]),
                "privileged" => security["privileged"] = serde_json::json!(true),
                _ => security["procMount"] = serde_json::json!("Unmasked"),
            }
            s.requests.clear();
        }
        assert_eq!(
            owner.start(&c, r.environment_id).await,
            Err(Error::Conflict),
            "{variant}"
        );
        assert!(!cluster
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, _, _)| m == "PATCH"));
    }
}

#[tokio::test(start_paused = true)]
async fn unrelated_pods_sharing_an_owner_do_not_exhaust_the_stop_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    {
        let mut s = cluster.lock().unwrap();
        s.objects.insert(
            format!(
                "/apis/apps/v1/namespaces/{}/replicasets/unrelated-rs",
                r.namespace
            ),
            serde_json::json!({"apiVersion":"apps/v1","kind":"ReplicaSet","metadata":{
            "name":"unrelated-rs","namespace":r.namespace,"uid":"unrelated-uid"}}),
        );
        for i in 0..25 {
            s.objects.insert(format!("/api/v1/namespaces/{}/pods/unrelated-{i}",r.namespace),serde_json::json!({"apiVersion":"v1","kind":"Pod","metadata":{
                "name":format!("unrelated-{i}"),"namespace":r.namespace,"uid":format!("pod-{i}"),"ownerReferences":[{
                    "apiVersion":"apps/v1","kind":"ReplicaSet","name":"unrelated-rs","uid":"unrelated-uid","controller":true}]}}));
        }
        s.slow_replica_gets = true;
    }
    owner.stop(&c, r.environment_id).await.unwrap();
    assert!(!cluster
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, _, _)| m == "DELETE"));
}

#[tokio::test]
async fn start_rejects_unexpected_profiles_and_environment_injection() {
    for variant in [
        "pod-apparmor",
        "legacy-apparmor",
        "container-apparmor",
        "selinux",
        "env",
        "env-from",
        "lifecycle",
        "startup-probe",
        "liveness-probe",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let owner = Owner::new(dir.path().into());
        let (c, cluster) = fixture::connection();
        let input = receipt();
        let r = owner
            .create(&c, input.workspace_id, &input.namespace, &input.image)
            .await
            .unwrap();
        {
            let mut s = cluster.lock().unwrap();
            let deployment = s
                .objects
                .values_mut()
                .find(|v| v["kind"] == "Deployment")
                .unwrap();
            let pod = &mut deployment["spec"]["template"]["spec"];
            match variant {
                "legacy-apparmor" => {}
                "pod-apparmor" => {
                    pod["securityContext"]["appArmorProfile"] =
                        serde_json::json!({"type":"Unconfined"})
                }
                "container-apparmor" => {
                    pod["containers"][0]["securityContext"]["appArmorProfile"] =
                        serde_json::json!({"type":"Unconfined"})
                }
                "selinux" => {
                    pod["securityContext"]["seLinuxOptions"] = serde_json::json!({"type":"spc_t"})
                }
                "env" => {
                    pod["containers"][0]["env"] =
                        serde_json::json!([{"name":"LD_PRELOAD","value":"/tmp/unexpected.so"}])
                }
                "env-from" => {
                    pod["containers"][0]["envFrom"] =
                        serde_json::json!([{"secretRef":{"name":"unexpected-secret"}}])
                }
                "lifecycle" => {
                    pod["containers"][0]["lifecycle"] = serde_json::json!({"postStart":{"exec":{"command":["touch","/home/creator/unexpected"]}}})
                }
                "startup-probe" => {
                    pod["containers"][0]["startupProbe"] =
                        serde_json::json!({"exec":{"command":["touch","/home/creator/unexpected"]}})
                }
                _ => {
                    pod["containers"][0]["livenessProbe"] =
                        serde_json::json!({"exec":{"command":["touch","/home/creator/unexpected"]}})
                }
            }
            if variant == "legacy-apparmor" {
                deployment["spec"]["template"]["metadata"]["annotations"] = serde_json::json!({"container.apparmor.security.beta.kubernetes.io/environment":"unconfined"});
            }
            s.requests.clear();
        }
        assert_eq!(
            owner.start(&c, r.environment_id).await,
            Err(Error::Conflict),
            "{variant}"
        );
        assert!(!cluster
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|(m, _, _)| m == "PATCH"));
    }
}

#[tokio::test]
async fn stopping_an_already_zero_deployment_invalidates_a_delayed_start() {
    use k8s_openapi::api::apps::v1::Deployment;
    use kube::api::{Patch, PatchParams};
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    let input = receipt();
    let r = owner
        .create(&c, input.workspace_id, &input.namespace, &input.image)
        .await
        .unwrap();
    let late_start = serde_json::json!({"metadata":{"uid":r.deployment_uid,"resourceVersion":"1"},"spec":{"replicas":1}});
    owner.stop(&c, r.environment_id).await.unwrap();
    let api: kube::Api<Deployment> = kube::Api::namespaced(c.client.clone(), &r.namespace);
    let result = api
        .patch(
            &r.resource_name(),
            &PatchParams::default(),
            &Patch::Merge(late_start),
        )
        .await;
    assert!(matches!(result,Err(kube::Error::Api(error)) if error.code == 409));
    assert!(cluster
        .lock()
        .unwrap()
        .objects
        .values()
        .filter(|v| v["kind"] == "Deployment")
        .all(|v| v["spec"]["replicas"] == 0));
}

#[test]
fn corrupt_or_oversized_receipt_is_not_an_environment() {
    let dir = tempfile::tempdir().unwrap();
    let s = store::Store::new(dir.path()).unwrap();
    let r = receipt();
    s.save(&r).unwrap();
    let path = dir
        .path()
        .join("copilot-environments-v1")
        .join(format!("{}.json", r.environment_id.simple()));
    let mut value = serde_json::to_value(&r).unwrap();
    value["target"]["trust"] = serde_json::json!("corrupt");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(s.load(r.environment_id), Err(Error::Storage)));
    std::fs::write(&path, vec![b' '; 20 * 1024]).unwrap();
    assert!(matches!(s.load(r.environment_id), Err(Error::Storage)));
}

#[test]
fn file_lock_serializes_owners_and_releases_after_drop() {
    let dir = tempfile::tempdir().unwrap();
    let first = store::Store::new(dir.path()).unwrap();
    assert!(matches!(store::Store::new(dir.path()), Err(Error::Busy)));
    drop(first);
    assert!(store::Store::new(dir.path()).is_ok());
}

#[test]
fn root_certificate_file_content_is_part_of_target_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ca.pem");
    let mut c = config();
    std::fs::write(&path, b"first public certificate fixture").unwrap();
    c.root_cert_file = Some(path.clone());
    let before = Target::from_config("team", &c).unwrap();
    std::fs::write(&path, b"changed public certificate fixture").unwrap();
    assert_ne!(before, Target::from_config("team", &c).unwrap());
    let serialized = serde_json::to_string(&before).unwrap();
    assert!(!serialized.contains("certificate fixture") && !serialized.contains("ca.pem"));
}

#[tokio::test]
async fn policy_creation_failure_retains_the_acknowledged_pvc() {
    let dir = tempfile::tempdir().unwrap();
    let owner = Owner::new(dir.path().into());
    let (c, cluster) = fixture::connection();
    cluster.lock().unwrap().fail_create = Some("networkpolicies".into());
    let r = receipt();
    assert_eq!(
        owner
            .create(&c, r.workspace_id, &r.namespace, &r.image)
            .await
            .unwrap_err(),
        Error::Unavailable
    );
    let saved = owner.receipts().unwrap().pop().unwrap();
    assert!(
        saved.pvc_uid.is_some() && saved.policy_uid.is_none() && saved.deployment_uid.is_none()
    );
    assert!(!cluster
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, _, _)| m == "DELETE"));
    assert!(!cluster
        .lock()
        .unwrap()
        .objects
        .values()
        .any(|v| v["kind"] == "Deployment"));
}

#[test]
fn target_identity_ignores_auth_but_not_server_or_trust() {
    let mut c = config();
    let t = Target::from_config("team", &c).unwrap();
    c.auth_info.token = Some("fixture-secret-not-a-real-token".into());
    assert_eq!(t, Target::from_config("team", &c).unwrap());
    assert!(!serde_json::to_string(&t)
        .unwrap()
        .contains("fixture-secret"));
    c.accept_invalid_certs = true;
    assert_ne!(t, Target::from_config("team", &c).unwrap());
    c.accept_invalid_certs = false;
    c.root_cert = Some(vec![vec![1, 2, 3]]);
    assert_ne!(t, Target::from_config("team", &c).unwrap());
    c.root_cert = None;
    c.cluster_url = "https://replacement.example".parse().unwrap();
    assert_ne!(t, Target::from_config("team", &c).unwrap());
}
