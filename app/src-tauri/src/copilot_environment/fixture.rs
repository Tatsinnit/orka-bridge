use super::*;
use kube::{client::Body, Client};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(super) struct Cluster {
    pub objects: BTreeMap<String, Value>,
    pub requests: Vec<(String, String, Value)>,
    pub denied: bool,
    pub fail_create: Option<String>,
    pub terminating: Option<String>,
    pub replace_before_patch: bool,
    pub restart_after_stop: bool,
    pub slow_replica_gets: bool,
}
pub(super) fn connection() -> (Connection, Arc<Mutex<Cluster>>) {
    let state = Arc::new(Mutex::new(Cluster::default()));
    let shared = state.clone();
    let service = tower::service_fn(move |request: http::Request<Body>| {
        let shared = shared.clone();
        async move {
            let method = request.method().to_string();
            let path = request.uri().path().to_owned();
            let bytes = request.into_body().collect_bytes().await?;
            let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
            let delayed =
                shared.lock().unwrap().slow_replica_gets && path.contains("/replicasets/");
            if delayed {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            let mut s = shared.lock().unwrap();
            s.requests
                .push((method.clone(), path.clone(), body.clone()));
            let mut status = 200;
            let result = if path.ends_with("selfsubjectaccessreviews") {
                json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectAccessReview","metadata":{},"status":{"allowed":!s.denied}})
            } else if method == "POST" {
                let mut object = body;
                // Kubernetes omits zero-valued non-pointer fields when encoding its response.
                if object["kind"] == "Deployment" {
                    for key in ["hostNetwork", "hostPID", "hostIPC"] {
                        object["spec"]["template"]["spec"]
                            .as_object_mut()
                            .unwrap()
                            .remove(key);
                    }
                }
                if object["kind"] == "NetworkPolicy" {
                    object["spec"].as_object_mut().unwrap().remove("ingress");
                }
                let key = format!("{}/{}", path, object["metadata"]["name"].as_str().unwrap());
                object["metadata"]["uid"] = json!(format!("uid-{}", s.objects.len()));
                object["metadata"]["resourceVersion"] = json!("1");
                if object["kind"] == "Deployment" {
                    object["metadata"]["generation"] = json!(1);
                    object["status"] = json!({"observedGeneration":1,"replicas":0});
                }
                s.objects.insert(key, object.clone());
                if s.fail_create
                    .as_ref()
                    .is_some_and(|kind| path.ends_with(kind))
                {
                    status = 500;
                    json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"fixture-secret-do-not-echo","reason":"InternalError","code":500})
                } else {
                    object
                }
            } else if method == "GET" && s.objects.contains_key(&path) {
                s.objects[&path].clone()
            } else if method == "GET"
                && matches!(
                    path.rsplit('/').next().unwrap(),
                    "deployments"
                        | "persistentvolumeclaims"
                        | "networkpolicies"
                        | "pods"
                        | "replicasets"
                )
            {
                if path.ends_with("/pods") && s.restart_after_stop {
                    s.restart_after_stop = false;
                    for object in s.objects.values_mut().filter(|v| v["kind"] == "Deployment") {
                        object["spec"]["replicas"] = json!(1);
                        object["metadata"]["resourceVersion"] = json!("2");
                    }
                }
                let prefix = format!("{path}/");
                json!({"apiVersion":"v1","kind":"List","metadata":{},"items":s.objects.iter().filter(|(k,_)| k.starts_with(&prefix)).map(|(_,v)|v.clone()).collect::<Vec<_>>()})
            } else if method == "PATCH" && s.objects.contains_key(&path) {
                let replaced = s.replace_before_patch;
                if replaced {
                    s.objects.get_mut(&path).unwrap()["metadata"]["uid"] = json!("replacement");
                }
                let old = s.objects.get_mut(&path).unwrap();
                if old["metadata"]["uid"] != body["metadata"]["uid"]
                    || old["metadata"]["resourceVersion"] != body["metadata"]["resourceVersion"]
                {
                    status = 409;
                    json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"Conflict","code":409})
                } else {
                    let changed = old["spec"]["replicas"] != body["spec"]["replicas"]
                        || (body["metadata"]["annotations"].is_object()
                            && old["metadata"]["annotations"] != body["metadata"]["annotations"]);
                    old["spec"]["replicas"] = body["spec"]["replicas"].clone();
                    if body["metadata"]["annotations"].is_object() {
                        old["metadata"]["annotations"] = body["metadata"]["annotations"].clone();
                    }
                    if changed {
                        let generation = old["metadata"]["generation"].as_i64().unwrap_or(1) + 1;
                        let version = old["metadata"]["resourceVersion"]
                            .as_str()
                            .unwrap()
                            .parse::<u64>()
                            .unwrap()
                            + 1;
                        old["metadata"]["resourceVersion"] = json!(version.to_string());
                        old["metadata"]["generation"] = json!(generation);
                        old["status"] = json!({"observedGeneration":generation,"replicas":body["spec"]["replicas"]});
                    }
                    old.clone()
                }
            } else if method == "DELETE" && s.objects.contains_key(&path) {
                if s.objects[&path]["metadata"]["uid"] != body["preconditions"]["uid"] {
                    status = 409;
                } else if s
                    .terminating
                    .as_ref()
                    .is_some_and(|kind| path.contains(kind))
                {
                    s.objects.get_mut(&path).unwrap()["metadata"]["deletionTimestamp"] =
                        json!("2026-01-01T00:00:00Z");
                } else {
                    s.objects.remove(&path);
                }
                json!({"kind":"Status","apiVersion":"v1","status":"Success","code":status})
            } else {
                status = 404;
                json!({"kind":"Status","apiVersion":"v1","status":"Failure","reason":"NotFound","code":404})
            };
            Ok::<_, kube::Error>(
                http::Response::builder()
                    .status(status)
                    .body(Body::from(serde_json::to_vec(&result).unwrap()))
                    .unwrap(),
            )
        }
    });
    (
        Connection {
            target: Target::from_config("team", &super::tests::config()).unwrap(),
            client: Client::new(service, "default"),
        },
        state,
    )
}
