#[path = "support/exec_credential_fixture.rs"]
mod credential_fixture;
#[path = "support/cluster_tls_fixture.rs"]
mod tls_fixture;
use bridge_engine::clusters::*;
use kube::config::Kubeconfig;
use serde_json::{json, Value};
use std::{
    future::Future,
    io::{Read, Write},
    net::TcpListener,
    pin::Pin,
    sync::{Arc, Mutex},
};

#[path = "support/cluster_forward.rs"]
mod forward;

#[tokio::test]
async fn places_snapshot_is_passive_and_selection_is_exact_and_revocable() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    assert_eq!(reply(session.places())["places"], json!([]));
    assert_eq!(reply(session.places())["selectedKey"], Value::Null);
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    api.calls.lock().unwrap().clear();
    orka.calls.lock().unwrap().clear();
    let view = reply(session.places());
    assert_eq!(view["places"].as_array().unwrap().len(), 1);
    assert_eq!(view["selectedKey"], view["places"][0]["key"]);
    assert_eq!(view["places"][0]["observedAt"], Value::Null);
    assert!(view["places"][0]["checkedAt"].as_u64().is_some());
    assert_eq!(view["places"][0]["stale"], true);
    assert_eq!(view["places"][0]["agents"], json!([]));
    assert_eq!(reply(session.places()), view);
    assert!(api.calls.lock().unwrap().is_empty());
    assert!(orka.calls.lock().unwrap().is_empty());
    assert!(!view.to_string().contains("static-fixture"));
    *api.replacement.lock().unwrap() = true;
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let both = reply(session.places());
    let places = both["places"].as_array().unwrap();
    assert_eq!(places.len(), 2);
    assert_eq!(places[0]["context"], places[1]["context"]);
    assert_ne!(places[0]["key"], places[1]["key"]);
    assert_ne!(both["selectedKey"], view["selectedKey"]);
    assert!(places
        .iter()
        .any(|place| place["key"] == both["selectedKey"]));
    session.discover(&context).await.unwrap();
    assert_eq!(reply(session.places())["selectedKey"], Value::Null);
    assert_eq!(reply(session.places())["places"], both["places"]);
    session.reload().await.unwrap();
    assert_eq!(reply(session.places())["selectedKey"], Value::Null);
    assert_eq!(reply(session.places())["places"], both["places"]);
}

#[tokio::test]
async fn overview_bound_watched_namespace_must_still_be_explicit() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let first = reply(session.overview().await);
    for missing in [true, false] {
        *api.no_watch.lock().unwrap() = missing;
        *api.changed_watch.lock().unwrap() = !missing;
        orka.calls.lock().unwrap().clear();
        let changed = reply(session.overview().await);
        assert_eq!(changed[0]["stale"], true);
        assert_eq!(changed[0]["observedAt"], first[0]["observedAt"]);
        assert!(orka.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn overview_recreated_deployment_behind_retained_service_stays_stale() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let first = reply(session.overview().await);
    assert_eq!(first[0]["stale"], false);
    *api.deployment_replacement.lock().unwrap() = true;
    orka.calls.lock().unwrap().clear();
    let replaced = reply(session.overview().await);
    assert_eq!(replaced[0]["stale"], true);
    assert_eq!(replaced[0]["observedAt"], first[0]["observedAt"]);
    assert!(
        orka.calls.lock().unwrap().is_empty(),
        "replacement must not be listed under old installation"
    );
}

#[tokio::test]
async fn overview_expired_access_never_sends_or_refreshes_a_credential() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    let mut access = client.connect(id, None).await.unwrap();
    access.expiry = Some(k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
        "2020-01-01T00:00:00Z".parse().unwrap(),
    ));
    api.calls.lock().unwrap().clear();
    orka.calls.lock().unwrap().clear();
    assert!(matches!(
        access.inventory().await,
        Err(ClusterError::Denied)
    ));
    assert!(api.calls.lock().unwrap().is_empty());
    assert!(orka.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn overview_pagination_failure_keeps_the_last_complete_inventory() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let one = json!({"metadata":{"name":"same","namespace":"agents","uid":"one"},"spec":{"systemPrompt":{"inline":"x".repeat(900_000)}}});
    let two = json!({"metadata":{"name":"same","namespace":"agents","uid":"two"}});
    *orka.inventory_pages.lock().unwrap() = vec![
        json!({"items":[one.clone()],"metadata":{"continue":"next & cursor"}}),
        json!({"items":[two.clone()],"metadata":{}}),
    ];
    let complete = reply(session.overview().await);
    assert_eq!(complete[0]["agents"].as_array().unwrap().len(), 2);
    assert!(!complete.to_string().contains("systemPrompt"));
    assert!(orka
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.0 == OrkaResource::AgentInventory && c.3.as_deref() == Some("next & cursor")));
    for bad in [
        json!({"items":[two.clone()],"metadata":{"continue":"next & cursor"}}),
        json!({"items":[one.clone()],"metadata":{}}),
        json!({"items":[{"metadata":{"name":"same","namespace":"other","uid":"new"}}],"metadata":{}}),
        json!({"items":null,"metadata":{}}),
    ] {
        orka.inventory_pages.lock().unwrap()[1] = bad;
        let failed = reply(session.overview().await);
        assert_eq!(failed[0]["stale"], true);
        assert_eq!(failed[0]["agents"], complete[0]["agents"]);
        assert_eq!(failed[0]["observedAt"], complete[0]["observedAt"]);
    }
}

#[tokio::test]
async fn overview_coalesces_refresh_and_fences_results_after_reload() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let initial = reply(session.overview().await);
    let gate = Arc::new(tokio::sync::Notify::new());
    *orka.inventory_gate.lock().unwrap() = Some(gate.clone());
    orka.calls.lock().unwrap().clear();
    let first = tokio::spawn({
        let session = session.clone();
        async move { session.overview().await }
    });
    until(|| !orka.calls.lock().unwrap().is_empty()).await;
    let second = tokio::spawn({
        let session = session.clone();
        async move { session.overview().await }
    });
    tokio::task::yield_now().await;
    session.reload().await.unwrap();
    gate.notify_waiters();
    let first = reply(first.await.unwrap());
    let second = reply(second.await.unwrap());
    assert_eq!(first[0]["stale"], true);
    assert_eq!(first[0]["observedAt"], initial[0]["observedAt"]);
    assert_eq!(first, second);
    assert_eq!(
        orka.calls.lock().unwrap().len(),
        1,
        "concurrent refreshes must share one read"
    );
}

#[tokio::test]
async fn overview_scoped_refresh_does_not_wait_for_unrelated_global_read() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut cfg = config(&api.start(), json!({"token":"static-fixture"}));
    let mut healthy = cfg.contexts[0].clone();
    cfg.contexts[0].name = "a-offline".into();
    healthy.name = "z-healthy".into();
    cfg.contexts = vec![cfg.contexts[0].clone(), healthy];
    let (session, _) = reading_session(cfg, &orka);
    for context in session.contexts().await.unwrap() {
        let target = id_of(&session.discover(&id_of(context)).await.unwrap()[0]);
        assert_eq!(
            reply(session.use_target(&target, None).await)["status"],
            "connected"
        );
    }
    let key = session.places().places[1].key.clone();
    orka.calls.lock().unwrap().clear();
    let gate = Arc::new(tokio::sync::Notify::new());
    *orka.inventory_gate.lock().unwrap() = Some(gate.clone());
    *orka.inventory_gate_context.lock().unwrap() = Some("a-offline".into());
    *orka.agents.lock().unwrap() = Some(
        json!({"items":[{"metadata":{"name":"known","namespace":"agents","uid":"healthy-agent"}}],"metadata":{}}),
    );
    let mut blocked = tokio::spawn({
        let session = session.clone();
        async move { session.overview().await }
    });
    until(|| !orka.calls.lock().unwrap().is_empty()).await;
    let healthy = tokio::select! {
        result = bridge_engine::overview_places::list_one(&session, None, &key) => reply(result)["places"].clone(),
        _ = &mut blocked => panic!("scoped read waited for the unrelated inventory"),
    };
    assert_eq!(healthy.as_array().unwrap().len(), 1);
    assert_eq!(healthy[0]["key"], key);
    assert_eq!(healthy[0]["stale"], false);
    assert_eq!(healthy[0]["agents"][0]["uid"], "healthy-agent");
    let offline = reply(session.places());
    assert_eq!(offline["places"][0]["stale"], true);
    assert_eq!(offline["places"][0]["observedAt"], Value::Null);
    assert_eq!(orka.calls.lock().unwrap().len(), 2);
    assert!(!blocked.is_finished());
    gate.notify_one();
    blocked.await.unwrap();
    orka.calls.lock().unwrap().clear();
    assert!(session.overview_one("not-confirmed").await.is_empty());
    assert!(orka.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn newer_refresh_wins_when_overview_and_manage_finish_out_of_order() {
    for scoped_first in [false, true] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let (session, _) = reading_session(
            config(&api.start(), json!({"token":"static-fixture"})),
            &orka,
        );
        let context = id_of(&session.contexts().await.unwrap()[0]);
        let target = id_of(&session.discover(&context).await.unwrap()[0]);
        assert_eq!(
            reply(session.use_target(&target, None).await)["status"],
            "connected"
        );
        let key = session.places().places[0].key.clone();
        *orka.agents.lock().unwrap() = Some(
            json!({"items":[{"metadata":{"name":"old","namespace":"agents","uid":"old"}}],"metadata":{}}),
        );
        let gate = Arc::new(tokio::sync::Notify::new());
        *orka.inventory_gate.lock().unwrap() = Some(gate.clone());
        orka.calls.lock().unwrap().clear();
        let older = tokio::spawn({
            let session = session.clone();
            let key = key.clone();
            async move {
                if scoped_first {
                    session.overview_one(&key).await
                } else {
                    session.overview().await
                }
            }
        });
        until(|| !orka.calls.lock().unwrap().is_empty()).await;
        *orka.inventory_gate.lock().unwrap() = None;
        *orka.agents.lock().unwrap() = Some(
            json!({"items":[{"metadata":{"name":"new","namespace":"agents","uid":"new"}}],"metadata":{}}),
        );
        let fresh = reply(if scoped_first {
            session.overview().await
        } else {
            session.overview_one(&key).await
        });
        assert_eq!(fresh[0]["agents"][0]["uid"], "new");
        gate.notify_one();
        older.await.unwrap();
        assert_eq!(
            reply(session.places())["places"][0]["agents"],
            fresh[0]["agents"]
        );
        assert_eq!(
            reply(session.places())["places"][0]["observedAt"],
            fresh[0]["observedAt"]
        );
    }
}

#[tokio::test]
async fn overview_scoped_refresh_fences_reload_and_remove() {
    for remove in [false, true] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let (session, _) = reading_session(
            config(&api.start(), json!({"token":"static-fixture"})),
            &orka,
        );
        let context = id_of(&session.contexts().await.unwrap()[0]);
        let target = id_of(&session.discover(&context).await.unwrap()[0]);
        session.use_target(&target, None).await;
        let key = session.places().places[0].key.clone();
        let gate = Arc::new(tokio::sync::Notify::new());
        *orka.inventory_gate.lock().unwrap() = Some(gate.clone());
        orka.calls.lock().unwrap().clear();
        let reading = tokio::spawn({
            let session = session.clone();
            let key = key.clone();
            async move { session.overview_one(&key).await }
        });
        until(|| !orka.calls.lock().unwrap().is_empty()).await;
        if remove {
            session.forget(&key).await.unwrap();
        } else {
            session.reload().await.unwrap();
        }
        gate.notify_one();
        let late = reply(reading.await.unwrap());
        if remove {
            assert_eq!(late, json!([]));
        } else {
            assert_eq!(late[0]["stale"], true);
            assert_eq!(late[0]["observedAt"], Value::Null);
        }
    }
}

#[tokio::test]
async fn overview_inventory_is_get_only_and_keeps_stale_after_reload_without_reconnecting() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let cfg = config(&api.start(), json!({"token":"static-fixture"}));
    let (session, _) = reading_session(cfg, &orka);
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    *orka.agents.lock().unwrap() = Some(json!({"items":[
        {"metadata":{"name":"same","namespace":"agents","uid":"one"},"spec":{"type":"runtime","model":{"name":"other"},"tools":[{"name":"web_fetch"}]}},
        {"metadata":{"name":"same","namespace":"agents","uid":"two"}}
    ],"metadata":{}}));
    api.calls.lock().unwrap().clear();
    orka.calls.lock().unwrap().clear();
    let first = reply(session.overview().await);
    assert_eq!(first[0]["agents"].as_array().unwrap().len(), 2);
    assert_eq!(first[0]["agents"][0]["agentType"], "runtime");
    assert_eq!(first[0]["agents"][0]["model"], "other");
    assert_eq!(first[0]["agents"][1]["model"], Value::Null);
    assert_eq!(first[0]["stale"], false);
    assert!(first[0]["observedAt"].as_u64().is_some());
    assert_eq!(first[0]["checkedAt"], first[0]["observedAt"]);
    {
        let calls = api.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "inventory may only check its recorded Deployment"
        );
        assert_eq!(calls[0].0, "GET");
        assert_eq!(calls[0].1, "/apis/apps/v1/namespaces/control/deployments");
    }
    let query = api.queries.lock().unwrap().last().unwrap().clone();
    let url = reqwest::Url::parse(&format!("http://fixture{query}")).unwrap();
    assert!(url
        .query_pairs()
        .any(|(k, v)| k == "fieldSelector" && v == "metadata.name=controller"));
    assert!(orka
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.0 == OrkaResource::AgentInventory));
    *orka.fail.lock().unwrap() = Some(AccessOutcome::Unavailable);
    let offline = reply(session.overview().await);
    assert_eq!(offline[0]["stale"], true);
    assert_eq!(offline[0]["agents"], first[0]["agents"]);
    *orka.fail.lock().unwrap() = None;
    let latest = reply(session.overview().await);
    assert_eq!(latest[0]["stale"], false);
    session.reload().await.unwrap();
    let calls = orka.calls.lock().unwrap().len();
    assert_eq!(reply(session.overview().await)[0]["stale"], true);
    assert_eq!(
        orka.calls.lock().unwrap().len(),
        calls,
        "reload must not restore read authority"
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let remembered = reply(session.places());
    assert_eq!(
        remembered["places"][0]["agents"], first[0]["agents"],
        "successful re-Use retains the last complete, stale inventory"
    );
    assert_eq!(
        remembered["places"][0]["observedAt"],
        latest[0]["observedAt"]
    );
    assert!(
        remembered["places"][0]["checkedAt"].as_u64().unwrap()
            >= latest[0]["checkedAt"].as_u64().unwrap()
    );
    let again = reply(session.overview().await);
    assert_eq!(again.as_array().unwrap().len(), 1);
    assert_eq!(again[0]["key"], first[0]["key"]);
    assert_eq!(again[0]["stale"], false);
    *orka.fail.lock().unwrap() = Some(AccessOutcome::Denied);
    session.use_target(&target, None).await;
    let calls = orka.calls.lock().unwrap().len();
    assert_eq!(reply(session.overview().await)[0]["stale"], true);
    assert_eq!(
        orka.calls.lock().unwrap().len(),
        calls,
        "failed re-Use revokes inventory access"
    );
}

#[tokio::test]
async fn command_use_cluster_no_agent_task_post() {
    use bridge_engine::clusters::commands::ClusterSession;
    let api = Arc::new(ApiFixture::default());
    let client = ClusterClient::new(
        config(&api.start(), json!({"token":"creator-fixture"})),
        Arc::new(PodOrkaRead),
    );
    let session = ClusterSession::new(client);
    let contexts = session.contexts().await.unwrap();
    assert!(api.calls.lock().unwrap().is_empty());
    let id = serde_json::to_value(&contexts[0]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let found = session.discover(&id).await.unwrap();
    assert!(api.calls.lock().unwrap().iter().all(|c| c.0 == "GET"));
    let target = serde_json::to_value(&found[0]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let reply = session.use_target(&target, None).await;
    let value = serde_json::to_value(reply).unwrap();
    assert_eq!(value["status"], "connected");
    assert_eq!(value["providers"][0]["name"], "model");
    assert!(!value.to_string().contains("creator-fixture"));
    assert!(!value.to_string().contains("http://"));
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    assert_eq!(api.wire.posts(), 0);
}

#[tokio::test]
async fn command_no_tokenrequest_before_use_and_conditional_account() {
    use bridge_engine::clusters::commands::ClusterSession;
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":""});
    let session = ClusterSession::new(client(&api, &orka, json!({"token":"static-fixture"})));
    let contexts = session.contexts().await.unwrap();
    let id = serde_json::to_value(&contexts[0]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let found = session.discover(&id).await.unwrap();
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    let target = serde_json::to_value(&found[0]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        serde_json::to_value(session.use_target(&target, None).await).unwrap()["status"],
        "needServiceAccount"
    );
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    assert_eq!(
        serde_json::to_value(session.use_target(&target, Some("existing".into())).await).unwrap()
            ["status"],
        "connected"
    );
    assert_eq!(
        api.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.1.ends_with("/token"))
            .count(),
        1
    );
}

#[tokio::test]
async fn command_unknown_ids_never_reach_cluster() {
    use bridge_engine::clusters::commands::ClusterSession;
    let api = Arc::new(ApiFixture::default());
    let session = ClusterSession::new(client(
        &api,
        &Arc::new(OrkaFixture::default()),
        json!({"token":"static-fixture"}),
    ));
    for id in [
        "https://example.com",
        "../kubeconfig",
        "00000000-0000-4000-8000-000000000001",
    ] {
        assert!(session.discover(id).await.is_err());
        assert_eq!(
            serde_json::to_value(session.use_target(id, Some("existing".into())).await).unwrap()
                ["status"],
            "unavailable"
        );
    }
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn command_unsupported_discovery_remains_distinct() {
    use bridge_engine::clusters::commands::ClusterSession;
    let api = Arc::new(ApiFixture::default());
    let session = ClusterSession::new(client(
        &api,
        &Arc::new(OrkaFixture::default()),
        json!({"token":"static-fixture"}),
    ));
    let contexts = session.contexts().await.unwrap();
    let id = serde_json::to_value(&contexts[1]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let error = session.discover(&id).await.unwrap_err();
    assert_eq!(
        serde_json::to_value(error).unwrap(),
        json!({"check":"credentials","result":"unsupportedCredential"})
    );
    assert!(api.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn command_use_future_is_send_for_desktop_dispatch() {
    use bridge_engine::clusters::commands::ClusterSession;
    let api = Arc::new(ApiFixture::default());
    let session = ClusterSession::new(client(
        &api,
        &Arc::new(OrkaFixture::default()),
        json!({"token":"static-fixture"}),
    ));
    tokio::spawn(async move { session.use_target("unknown", None).await })
        .await
        .unwrap();
}

// Only the mandatory fields Bridge sends/reads; no equal-image/version requirement.
fn send_schema(resource: &str) -> Value {
    match resource {
        "agents" => json!({"type":"object","properties":{
            "spec":{"type":"object","properties":{
                "model":{"type":"object","properties":{"name":{"type":"string"}}},
                "providerRef":{"type":"object","properties":{"name":{"type":"string"}}},
                "systemPrompt":{"type":"object","properties":{"inline":{"type":"string"}}},
                "tools":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"}}}}
            }}
        }}),
        "tasks" => json!({"type":"object","properties":{
            "spec":{"type":"object","properties":{
                "type":{"type":"string","enum":["container","ai","agent"]},
                "agentRef":{"type":"object","properties":{"name":{"type":"string"}}},
                "prompt":{"type":"string"},"timeout":{"type":"string"},
                "retryPolicy":{"type":"object","properties":{"maxRetries":{"type":"integer"}}}
            }}
        }}),
        "providers" => {
            json!({"type":"object","properties":{"status":{"type":"object","properties":{"ready":{"type":"boolean"}}}}})
        }
        "tools" => json!({"type":"object","properties":{"metadata":{"type":"object"}}}),
        _ => panic!("unexpected resource"),
    }
}

#[tokio::test]
async fn remembered_reconnect_is_exact_uid_discovery_not_admission() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let cfg = config(&api.start(), json!({"token":"static-fixture"}));
    let (session, fresh) = reading_session(cfg.clone(), &orka);
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    session.use_target(&target, None).await;
    let key = session.places().places[0].key.clone();
    orka.calls.lock().unwrap().clear();
    api.calls.lock().unwrap().clear();
    let reconnected = session.reconnect(&key).await.unwrap();
    assert_ne!(id_of(&reconnected), target);
    assert_eq!(reconnected.service_name, "orka");
    assert_eq!(reply(session.places())["selectedKey"], Value::Null);
    assert!(orka.calls.lock().unwrap().is_empty());
    assert!(api.calls.lock().unwrap().iter().all(|c| c.0 == "GET"));

    for changed in [
        &api.replacement,
        &api.deployment_replacement,
        &api.changed_watch,
    ] {
        *changed.lock().unwrap() = true;
        assert_eq!(
            reply(session.reconnect(&key).await.unwrap_err()),
            json!({"check":"compatibility","result":"identityRejected"})
        );
        *changed.lock().unwrap() = false;
    }
    let (_, mut duplicate) = api.response("/apis/apps/v1/deployments");
    let container = duplicate["items"][0]["spec"]["template"]["spec"]["containers"][0].clone();
    duplicate["items"][0]["spec"]["template"]["spec"]["containers"]
        .as_array_mut()
        .unwrap()
        .push(container);
    *api.override_response.lock().unwrap() = Some(("/deployments".into(), 200, duplicate));
    assert_eq!(
        reply(session.reconnect(&key).await.unwrap_err()),
        json!({"check":"compatibility","result":"identityRejected"})
    );
    *api.override_response.lock().unwrap() = None;
    let mut missing = cfg;
    missing.contexts.clear();
    *fresh.lock().unwrap() = Some(missing);
    assert_eq!(reply(session.reconnect(&key).await.unwrap_err()), stale());
}

#[tokio::test]
async fn forget_revokes_old_handles_and_fences_remove_readd_inventory_aba() {
    check_remove_readd_inventory_aba(false).await;
}
#[tokio::test]
async fn overview_scoped_refresh_fences_remove_readd_inventory_aba() {
    check_remove_readd_inventory_aba(true).await;
}
async fn check_remove_readd_inventory_aba(scoped: bool) {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    session.use_target(&target, None).await;
    let key = session.places().places[0].key.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    *orka.inventory_gate.lock().unwrap() = Some(gate.clone());
    orka.calls.lock().unwrap().clear();
    let reading = tokio::spawn({
        let session = session.clone();
        let key = key.clone();
        async move {
            if scoped {
                session.overview_one(&key).await
            } else {
                session.overview().await
            }
        }
    });
    until(|| !orka.calls.lock().unwrap().is_empty()).await;
    session.forget(&key).await.unwrap();
    assert!(session.places().places.is_empty());
    assert_eq!(reply(session.places())["selectedKey"], Value::Null);
    assert_ne!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    gate.notify_waiters();
    let late = reply(reading.await.unwrap());
    assert_eq!(late[0]["key"], key);
    assert_eq!(
        late[0]["stale"], true,
        "old generation cannot publish into re-added identity"
    );
    assert_eq!(late[0]["agents"], json!([]));
}

#[tokio::test]
async fn forget_preserves_other_identity_and_queues_after_started_use() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    session.use_target(&target, None).await;
    let key = session.places().places[0].key.clone();
    *api.replacement.lock().unwrap() = true;
    let next = id_of(&session.discover(&context).await.unwrap()[0]);
    session.use_target(&next, None).await;
    let selected = reply(session.places())["selectedKey"].clone();
    session.forget(&key).await.unwrap();
    assert_eq!(reply(session.places())["selectedKey"], selected);
    assert_eq!(
        reply(session.use_target(&next, None).await)["status"],
        "connected"
    );
    let gate = Arc::new(tokio::sync::Notify::new());
    // Gate the existing Use at an Orka check, then queue Remove behind it.
    let (release, wait) = std::sync::mpsc::channel();
    *api.gate.lock().unwrap() = Some(wait);
    api.calls.lock().unwrap().clear();
    let using = tokio::spawn({
        let session = session.clone();
        async move { session.use_target(&next, None).await }
    });
    until(|| !api.calls.lock().unwrap().is_empty()).await;
    let forgetting = tokio::spawn({
        let session = session.clone();
        let key = selected.as_str().unwrap().to_owned();
        let gate = gate.clone();
        async move {
            gate.notify_one();
            session.forget(&key).await
        }
    });
    gate.notified().await;
    assert!(!forgetting.is_finished());
    release.send(()).unwrap();
    assert_eq!(reply(using.await.unwrap())["status"], "connected");
    forgetting.await.unwrap().unwrap();
    assert!(session.places().places.is_empty());
}

type ImpersonatedRequest = (String, Vec<(String, String)>);
type GateStep = (String, std::sync::mpsc::Sender<()>);
#[derive(Default)]
struct ApiFixture {
    wire: forward::Wire,
    calls: Mutex<Vec<(String, String, String, Value)>>,
    impersonation: Mutex<Vec<ImpersonatedRequest>>,
    deny: Mutex<bool>,
    replacement: Mutex<bool>,
    deployment_replacement: Mutex<bool>,
    queries: Mutex<Vec<String>>,
    no_watch: Mutex<bool>,
    changed_watch: Mutex<bool>,
    unrelated_api: Mutex<bool>,
    override_response: Mutex<Option<(String, u16, Value)>>,
    // Hold an individual check open to exercise lock queuing and deadline labels.
    gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    step_gate: Mutex<Option<std::sync::mpsc::Sender<GateStep>>>,
}
impl ApiFixture {
    fn start(self: &Arc<Self>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let fixture = self.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let fixture = fixture.clone();
                std::thread::spawn(move || {
                    let mut stream = stream.unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut buf = [0; 4096];
                    let header_end = loop {
                        let n = stream.read(&mut buf).unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(i) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                    let mut first = headers.lines().next().unwrap().split_whitespace();
                    let method = first.next().unwrap().to_string();
                    let path = first.next().unwrap().to_string();
                    let header = |name: &str| {
                        headers.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case(name)
                                .then(|| value.trim().to_string())
                        })
                    };
                    fixture.impersonation.lock().unwrap().push((
                        path.split('?').next().unwrap().to_owned(),
                        headers
                            .lines()
                            .filter_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.to_ascii_lowercase()
                                    .starts_with("impersonate-")
                                    .then(|| (key.to_ascii_lowercase(), value.trim().to_owned()))
                            })
                            .collect(),
                    ));
                    if path.contains("/portforward") {
                        fixture.wire.serve(stream, &headers);
                        return;
                    }
                    let len = header("content-length")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    while bytes.len() < header_end + len {
                        let n = stream.read(&mut buf).unwrap();
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    let body = serde_json::from_slice(&bytes[header_end..]).unwrap_or(Value::Null);
                    fixture.calls.lock().unwrap().push((
                        method,
                        path.split('?').next().unwrap().into(),
                        header("authorization").unwrap_or_default(),
                        body,
                    ));
                    fixture.queries.lock().unwrap().push(path.clone());
                    let (status, response) = fixture.response(path.split('?').next().unwrap());
                    let body = response.to_string();
                    let reply = write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    // A deadline test deliberately drops the gated request before its reply.
                    assert!(
                        reply.is_ok()
                            || reply.err().is_some_and(|e| matches!(
                                e.kind(),
                                std::io::ErrorKind::BrokenPipe
                                    | std::io::ErrorKind::ConnectionReset
                            ))
                    );
                });
            }
        });
        address
    }
    fn response(&self, path: &str) -> (u16, Value) {
        if let Some(gate) = self.step_gate.lock().unwrap().as_ref() {
            let (release, wait) = std::sync::mpsc::channel();
            if gate.send((path.into(), release)).is_ok() {
                let _ = wait.recv();
            }
        }
        if let Some((suffix, status, body)) = self.override_response.lock().unwrap().as_ref() {
            if path.ends_with(suffix) {
                return (*status, body.clone());
            }
        }
        if path.ends_with("/token") {
            return (
                200,
                json!({"apiVersion":"authentication.k8s.io/v1","kind":"TokenRequest", "spec":{"audiences":[]}, "status":{"token":"issued-fixture","expirationTimestamp":"2099-01-01T00:00:00Z"}}),
            );
        }
        if path.ends_with("/selfsubjectaccessreviews") {
            return (
                200,
                json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectAccessReview","spec":{},"status":{"allowed": !*self.deny.lock().unwrap()}}),
            );
        }
        if path.contains("/customresourcedefinitions/") {
            let name = path.rsplit('/').next().unwrap();
            let plural = name.split('.').next().unwrap();
            return (
                200,
                json!({"apiVersion":"apiextensions.k8s.io/v1","kind":"CustomResourceDefinition","metadata":{"name":name},"spec":{"group":"core.orka.ai","scope":"Namespaced","names":{"kind":"Fixture","plural":plural},"versions":[{"name":"v1alpha1","served":true,"storage":true,"schema":{"openAPIV3Schema":send_schema(plural)}}]}}),
            );
        }
        if path.ends_with("/pods") {
            return (
                200,
                json!({"apiVersion":"v1","kind":"PodList","metadata":{},"items":[
                    {"metadata":{"name":"not-ready","namespace":"control","uid":"bad","labels":{"app":"orka"}},"spec":{"containers":[]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"False"}]}},
                    {"metadata":{"name":"ready","namespace":"control","uid":"pod-uid","labels":{"app":"orka"}},"spec":{"containers":[{"name":"controller","ports":[{"name":"api","containerPort":8080}]}]},"status":{"phase":"Running","conditions":[{"type":"Ready","status":"True"}]}}
                ]}),
            );
        }
        if path.ends_with("/services") {
            if let Some(gate) = self.gate.lock().unwrap().take() {
                let _ = gate.recv();
            }
            let uid = if *self.replacement.lock().unwrap() {
                "replacement"
            } else {
                "service-uid"
            };
            let mut list = json!({"apiVersion":"v1","kind":"ServiceList","metadata":{},"items":[{"metadata":{"name":"orka","namespace":"control","uid":uid},"spec":{"selector":{"app":"orka"},"ports":[{"name":"api","port":80,"targetPort":"api"}]}}]});
            if *self.unrelated_api.lock().unwrap() {
                list["items"].as_array_mut().unwrap().push(json!({"metadata":{"name":"other","namespace":"control","uid":"other-service"},"spec":{"selector":{"app":"other"},"ports":[{"name":"api","port":80,"targetPort":9000}]}}));
            }
            return (200, list);
        }
        if path.ends_with("/deployments") {
            let args = if *self.no_watch.lock().unwrap() {
                json!(["--api-port=8080"])
            } else if *self.changed_watch.lock().unwrap() {
                json!(["--api-port=8080", "--watch-namespace=other-agents"])
            } else {
                json!(["--api-port=8080", "--watch-namespace=agents"])
            };
            let mut list = json!({"apiVersion":"apps/v1","kind":"DeploymentList","metadata":{},"items":[{"metadata":{"name":"controller","namespace":"control","uid":"deployment-uid"},"spec":{"selector":{"matchLabels":{"app":"orka"}},"template":{"metadata":{"labels":{"app":"orka"}},"spec":{"containers":[{"name":"controller","image":"ghcr.io/orka-agents/orka:v99","args":args,"ports":[{"name":"api","containerPort":8080}]}]}}}}]});
            if *self.deployment_replacement.lock().unwrap() {
                list["items"][0]["metadata"]["uid"] = json!("replacement-deployment");
            }
            if *self.unrelated_api.lock().unwrap() {
                list["items"][0]["spec"]["template"]["metadata"]["labels"]
                    ["app.kubernetes.io/name"] = json!("orka");
                list["items"][0]["spec"]["template"]["spec"]["containers"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name":"sidecar","image":"example/sidecar:v1"}));
                list["items"].as_array_mut().unwrap().push(json!({"metadata":{"name":"other","namespace":"control","uid":"other-deployment"},"spec":{"selector":{"matchLabels":{"app":"other"}},"template":{"metadata":{"labels":{"app":"other"}},"spec":{"containers":[{"name":"server","image":"example/api:v1","args":["--api-port=9000","--watch-namespace=agents"]}]}}}}));
            }
            return (200, list);
        }
        panic!("unexpected Kubernetes path {path}");
    }
}

type OrkaCall = (OrkaResource, String, String, Option<String>);
struct OrkaFixture {
    calls: Mutex<Vec<OrkaCall>>,
    providers: Mutex<Option<Value>>,
    agents: Mutex<Option<Value>>,
    inventory_pages: Mutex<Vec<Value>>,
    inventory_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    inventory_gate_context: Mutex<Option<String>>,
    identity: Mutex<Value>,
    fail: Mutex<Option<AccessOutcome>>,
    fail_at: Mutex<Option<(OrkaResource, AccessOutcome)>>,
    // Endless Provider pages, each slower than a third of the operation budget.
    slow_pages: Mutex<bool>,
    echo: Mutex<bool>,
    paged: Mutex<bool>,
}
impl Default for OrkaFixture {
    fn default() -> Self {
        Self {
            calls: Mutex::default(),
            providers: Mutex::new(None),
            agents: Mutex::new(None),
            inventory_pages: Mutex::default(),
            inventory_gate: Mutex::default(),
            inventory_gate_context: Mutex::default(),
            identity: Mutex::new(
                json!({"authenticated":true,"authType":"tokenReview","namespace":"agents"}),
            ),
            fail: Mutex::new(None),
            fail_at: Mutex::new(None),
            slow_pages: Mutex::new(false),
            echo: Mutex::new(false),
            paged: Mutex::new(false),
        }
    }
}
impl OrkaRead for OrkaFixture {
    fn get<'a>(
        &'a self,
        _: &'a kube::Client,
        binding: &'a TargetBinding,
        bearer: &'a str,
        resource: OrkaResource,
        namespace: &'a str,
        after: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, AccessOutcome>> + Send + 'a>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((
                resource,
                bearer.into(),
                namespace.into(),
                after.map(str::to_string),
            ));
            if let Some(error) = *self.fail.lock().unwrap() {
                return Err(error);
            }
            if let Some((at, error)) = *self.fail_at.lock().unwrap() {
                if at == resource {
                    return Err(error);
                }
            }
            if resource == OrkaResource::Providers && *self.slow_pages.lock().unwrap() {
                tokio::time::sleep(std::time::Duration::from_secs(9)).await;
                let next = format!("page-{}", self.calls.lock().unwrap().len());
                return Ok(json!({"items":[],"metadata":{"continue":next}}));
            }
            if *self.echo.lock().unwrap() {
                return Ok(json!({"echo":bearer}));
            }
            // Hold the reply after capturing its snapshot, so tests can reorder responses.
            let inventory = (resource == OrkaResource::AgentInventory).then(|| {
                self.agents
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| json!({"items":[],"metadata":{}}))
            });
            if resource == OrkaResource::AgentInventory {
                let gate = self.inventory_gate.lock().unwrap().clone();
                let gated = self
                    .inventory_gate_context
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_none_or(|context| context == &binding.context_name);
                if let Some(gate) = gate.filter(|_| gated) {
                    gate.notified().await;
                }
                let pages = self.inventory_pages.lock().unwrap();
                if !pages.is_empty() {
                    return Ok(pages[usize::from(after.is_some())].clone());
                }
            }
            Ok(match resource {
                OrkaResource::WhoAmI if bearer == "issued-fixture" => json!({"authenticated":true,"authType":"tokenReview","namespace":"agents"}),
                OrkaResource::WhoAmI => self.identity.lock().unwrap().clone(),
                OrkaResource::Providers => self.providers.lock().unwrap().clone().unwrap_or_else(|| {
                    json!({"items":[{"apiVersion":"core.orka.ai/v1alpha1","kind":"Provider","metadata":{"name":"model","namespace":"agents"},"spec":{"type":"openai","defaultModel":"fixture-model"},"status":{"ready":true}}],"metadata":{}})
                }),
                OrkaResource::Tools if *self.paged.lock().unwrap() && after.is_none() => {
                    json!({"items":[{"name":"custom","builtin":false}],"metadata":{"continue":"next"}})
                }
                OrkaResource::Tools => {
                    json!({"items":[{"name":"web_fetch","builtin":true}],"metadata":{}})
                }
                OrkaResource::Agents => self.agents.lock().unwrap().clone().unwrap_or_else(|| json!({"items":[],"metadata":{}})),
                OrkaResource::AgentInventory => inventory.unwrap(),
            })
        })
    }
}
fn failure(check: Check, result: CheckResult) -> ClusterFailure {
    ClusterFailure { check, result }
}
fn failed(check: Check, result: CheckResult) -> AccessOutcome {
    AccessOutcome::Failed(failure(check, result))
}
fn config(server: &str, auth: Value) -> Kubeconfig {
    serde_json::from_value(json!({"apiVersion":"v1","kind":"Config","current-context":"wrong","clusters":[{"name":"fixture","cluster":{"server":server}}],"users":[{"name":"user","user":auth}],"contexts":[{"name":"selected","context":{"cluster":"fixture","user":"user"}},{"name":"exec","context":{"cluster":"fixture","user":"helper"}}]})).unwrap()
}
fn client(api: &Arc<ApiFixture>, orka: &Arc<OrkaFixture>, auth: Value) -> ClusterClient {
    ClusterClient::new(config(&api.start(), auth), orka.clone())
}
async fn target(client: &mut ClusterClient) -> TargetId {
    let context = client.contexts().unwrap()[0].id;
    client.discover(context).await.unwrap()[0].id
}

#[tokio::test]
async fn creator_bearer_echo_after_serviceaccount_fallback_is_not_exported() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":""});
    *orka.providers.lock().unwrap() = Some(
        json!({"items":[{"metadata":{"name":"creator-fixture","namespace":"agents"},"status":{"ready":true}}],"metadata":{"continue":null}}),
    );
    let mut client = client(&api, &orka, json!({"token":"creator-fixture"}));
    let id = target(&mut client).await;
    assert_eq!(
        client.connect(id, Some("existing".into())).await.err(),
        Some(failed(Check::Providers, CheckResult::Unknown))
    );
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|c| c.1.ends_with("/token")).count(), 1);
    assert!(calls.iter().all(|c| c.0 == "GET"
        || c.1.ends_with("/token")
        || c.1.ends_with("/selfsubjectaccessreviews")));
}

#[tokio::test]
async fn omitted_provider_readiness_is_false_beside_a_ready_provider() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.providers.lock().unwrap() = Some(json!({"items":[
        {"metadata":{"name":"ready","namespace":"agents"},"status":{"ready":true}},
        {"metadata":{"name":"pending","namespace":"agents"},"status":{}}
    ],"metadata":{"continue":null}}));
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    assert_eq!(
        access
            .providers()
            .iter()
            .map(|p| (p.name.as_str(), p.ready))
            .collect::<Vec<_>>(),
        [("ready", true), ("pending", false)]
    );
    *orka.providers.lock().unwrap() = Some(
        json!({"items":[{"metadata":{"name":"invalid","namespace":"agents"},"status":{"ready":"false"}}],"metadata":{"continue":null}}),
    );
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(failed(Check::Providers, CheckResult::Unknown))
    );
}

#[tokio::test]
async fn contexts_are_offline_and_explicit_discovery_only_gets() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let views = client.contexts().unwrap();
    assert_eq!(
        views.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
        ["selected", "exec"]
    );
    assert!(api.calls.lock().unwrap().is_empty());
    let found = client.discover(views[0].id).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].watched_namespace, "agents");
    assert_eq!(found[0].service_namespace, "control");
    assert_eq!(found[0].image, "ghcr.io/orka-agents/orka:v99");
    assert!(api.calls.lock().unwrap().iter().all(|c| c.0 == "GET"));
    assert!(orka.calls.lock().unwrap().is_empty());
    let serialized = serde_json::to_string(&found).unwrap();
    assert!(!serialized.contains("static-fixture"));
    assert!(!serialized.contains("http://"));
}
#[tokio::test]
async fn unrelated_api_workload_does_not_hide_orka_or_break_revalidation() {
    let api = Arc::new(ApiFixture::default());
    *api.unrelated_api.lock().unwrap() = true;
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let views = client
        .discover(client.contexts().unwrap()[0].id)
        .await
        .unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].service_name, "orka");
    let access = client.connect(views[0].id, None).await.unwrap();
    client.revalidate(&access).await.unwrap();
    assert!(api
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.0 == "GET" || c.1.ends_with("/selfsubjectaccessreviews")));
}

#[tokio::test]
async fn missing_watch_namespace_is_not_guessed() {
    let api = Arc::new(ApiFixture::default());
    *api.no_watch.lock().unwrap() = true;
    let mut client = client(
        &api,
        &Arc::new(OrkaFixture::default()),
        json!({"token":"static-fixture"}),
    );
    let id = client.contexts().unwrap()[0].id;
    assert_eq!(
        client.discover(id).await.err(),
        Some(failure(Check::Compatibility, CheckResult::Incompatible))
    );
}
#[tokio::test]
async fn static_namespace_bearer_no_tokenrequest_and_same_bearer_permission_check() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(
        &api,
        &orka,
        json!({"token":"static-fixture","exec":{"command":"must-not-execute"}}),
    );
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    let providers = access.providers();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].name, "model");
    assert!(providers[0].ready);
    access
        .require_builtins(&["web_fetch".into()])
        .await
        .unwrap();
    let calls = api.calls.lock().unwrap();
    assert!(!calls.iter().any(|c| c.1.ends_with("/token")));
    let reviews: Vec<_> = calls
        .iter()
        .filter(|c| c.1.ends_with("/selfsubjectaccessreviews"))
        .collect();
    assert_eq!(reviews.len(), 10);
    assert!(reviews.iter().all(|c| c.2 == "Bearer static-fixture"));
    assert!(reviews.iter().any(
        |c| c.3["spec"]["resourceAttributes"]["resource"] == "agents"
            && c.3["spec"]["resourceAttributes"]["verb"] == "create"
    ));
}
#[tokio::test]
async fn oidc_no_sar_and_denial_does_not_substitute_identity() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"oidc","namespace":"agents"});
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    client.connect(id, None).await.unwrap();
    assert!(api.calls.lock().unwrap().iter().all(|c| c.0 == "GET"));
    let rejected = failed(Check::OrkaIdentity, CheckResult::AuthenticationRejected);
    *orka.fail.lock().unwrap() = Some(rejected);
    assert_eq!(
        client.connect(id, Some("existing".into())).await.err(),
        Some(rejected)
    );
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
}
#[tokio::test]
async fn namespace_less_tokenreview_requests_named_existing_sa_only_on_connect() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":""});
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(AccessOutcome::NeedServiceAccount)
    );
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":"agents"});
    // A namespaced creator no longer needs substitution, even if the caller supplied a name.
    client.connect(id, Some("existing".into())).await.unwrap();
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
}
#[tokio::test]
async fn sa_name_not_uri_no_request() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    let before = api.calls.lock().unwrap().len();
    for bad in [
        "https://host/token",
        "agents/name",
        " name",
        "name:other",
        "",
        "a..b",
        "Bad",
        "-name",
    ] {
        assert_eq!(
            client.connect(id, Some(bad.into())).await.err(),
            Some(failed(Check::ServiceAccount, CheckResult::Unknown))
        );
    }
    assert_eq!(api.calls.lock().unwrap().len(), before);
    assert!(orka.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn denied_create_reports_denied_no_product_post() {
    let api = Arc::new(ApiFixture::default());
    *api.deny.lock().unwrap() = true;
    let mut client = client(
        &api,
        &Arc::new(OrkaFixture::default()),
        json!({"token":"static-fixture"}),
    );
    let id = target(&mut client).await;
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(failed(Check::Permissions, CheckResult::PermissionDenied))
    );
    assert!(api
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.0 == "GET" || c.1.ends_with("/selfsubjectaccessreviews")));
}
#[tokio::test]
async fn effective_exec_visible_but_unavailable_and_credential_echo_not_exported() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"exec":{"command":"must-not-execute"}}));
    let id = client.contexts().unwrap()[0].id;
    assert_eq!(
        client.discover(id).await.err(),
        Some(failure(
            Check::Credentials,
            CheckResult::UnsupportedCredential
        ))
    );
    assert!(api.calls.lock().unwrap().is_empty());
    let mut client = ClusterClient::new(
        config(&api.start(), json!({"token":"static-fixture"})),
        orka.clone(),
    );
    let id = target(&mut client).await;
    *orka.echo.lock().unwrap() = true;
    let result = client.connect(id, None).await;
    assert_eq!(
        result.err(),
        Some(failed(Check::OrkaIdentity, CheckResult::Unknown))
    );
}
#[tokio::test]
async fn exec_creator_preserves_impersonation_but_ssar_is_token_only() {
    let api = Arc::new(ApiFixture::default());
    let version = "client.authentication.k8s.io/v1";
    let mut auth = credential_fixture::exec(version, credential_fixture::response(version));
    auth["as"] = json!("restricted-user");
    auth["as-uid"] = json!("restricted-uid");
    auth["as-groups"] = json!(["restricted-group"]);
    auth["as-user-extra"] = json!({"purpose":["fixture"]});
    let mut client = client(&api, &Arc::new(OrkaFixture::default()), auth);
    let target = target(&mut client).await;
    client.connect(target, None).await.unwrap();
    let headers = api.impersonation.lock().unwrap();
    assert!(headers
        .iter()
        .any(|(path, _)| path.ends_with("/selfsubjectaccessreviews")));
    for (path, values) in headers.iter() {
        if path.ends_with("/selfsubjectaccessreviews") {
            assert!(
                values.is_empty(),
                "bearer-only SSAR carried creator impersonation"
            );
        } else {
            let mut values = values.clone();
            values.sort();
            assert_eq!(
                values,
                vec![
                    ("impersonate-extra-purpose".into(), "fixture".into()),
                    ("impersonate-group".into(), "restricted-group".into()),
                    ("impersonate-uid".into(), "restricted-uid".into()),
                    ("impersonate-user".into(), "restricted-user".into())
                ]
            );
        }
    }
}

#[tokio::test]
async fn exec_certificate_creator_uses_existing_account_without_export_or_tls_relaxation() {
    use base64::Engine;
    let api = Arc::new(ApiFixture::default());
    let tls = tls_fixture::TlsFixture::start(&api.start()).await;
    let version = "client.authentication.k8s.io/v1";
    let decode = |key: &str| {
        String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(tls.auth[key].as_str().unwrap())
                .unwrap(),
        )
        .unwrap()
    };
    let result = json!({"kind":"ExecCredential","apiVersion":version,"status":{"clientCertificateData":decode("client-certificate-data"),"clientKeyData":decode("client-key-data")}});
    let mut cfg = config(&tls.server, credential_fixture::exec(version, result));
    cfg.clusters[0]
        .cluster
        .as_mut()
        .unwrap()
        .certificate_authority_data = Some(tls.ca.clone());
    cfg.clusters[0].cluster.as_mut().unwrap().tls_server_name = Some("fixture-server".into());
    let mut client = ClusterClient::new(cfg, Arc::new(OrkaFixture::default()));
    let target = target(&mut client).await;
    assert_eq!(
        client.connect(target, None).await.err(),
        Some(AccessOutcome::NeedServiceAccount)
    );
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    let access = client
        .connect(target, Some("existing".into()))
        .await
        .unwrap();
    assert!(access.expiry.is_some());
    let calls = api.calls.lock().unwrap();
    let peers = tls.peers.lock().unwrap();
    for (i, call) in calls.iter().enumerate() {
        if call.1.ends_with("/selfsubjectaccessreviews") {
            assert!(call.2 == "Bearer issued-fixture");
            assert!(!peers[i], "SSAR inherited the plugin certificate");
        }
    }
}

#[tokio::test]
async fn exec_mixed_credentials_preserve_creator_tls_but_ssar_uses_only_bearer() {
    use base64::Engine;
    for token in ["", "mixed-fixture"] {
        let api = Arc::new(ApiFixture::default());
        let tls = tls_fixture::TlsFixture::start(&api.start()).await;
        let version = "client.authentication.k8s.io/v1";
        let decode = |key: &str| {
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(tls.auth[key].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap()
        };
        let output = json!({"kind":"ExecCredential","apiVersion":version,"status":{"token":token,"clientCertificateData":decode("client-certificate-data"),"clientKeyData":decode("client-key-data")}});
        let mut cfg = config(&tls.server, credential_fixture::exec(version, output));
        cfg.clusters[0]
            .cluster
            .as_mut()
            .unwrap()
            .certificate_authority_data = Some(tls.ca.clone());
        cfg.clusters[0].cluster.as_mut().unwrap().tls_server_name = Some("fixture-server".into());
        let mut client = ClusterClient::new(cfg, Arc::new(OrkaFixture::default()));
        let target = target(&mut client).await;
        assert!(tls.peers.lock().unwrap().iter().all(|peer| *peer));
        if token.is_empty() {
            assert_eq!(
                client.connect(target, None).await.err(),
                Some(AccessOutcome::NeedServiceAccount)
            );
        } else {
            client.connect(target, None).await.unwrap();
            let calls = api.calls.lock().unwrap();
            assert!(!calls.iter().any(|c| c.1.ends_with("/token")));
            let peers = tls.peers.lock().unwrap();
            for (i, call) in calls.iter().enumerate() {
                assert!(call.2 == "Bearer mixed-fixture");
                assert_eq!(peers[i], !call.1.ends_with("/selfsubjectaccessreviews"));
            }
        }
    }
}

#[tokio::test]
async fn certificate_creator_existing_sa_and_issued_ssar_has_no_creator_tls_certificate() {
    let api = Arc::new(ApiFixture::default());
    let tls = tls_fixture::TlsFixture::start(&api.start()).await;
    let orka = Arc::new(OrkaFixture::default());
    let mut cfg = config(&tls.server, tls.auth.clone());
    let cluster = cfg.clusters[0].cluster.as_mut().unwrap();
    cluster.certificate_authority_data = Some(tls.ca.clone());
    cluster.tls_server_name = Some("fixture-server".into());
    let mut exec_cfg = cfg.clone();
    exec_cfg.auth_infos[0].auth_info.as_mut().unwrap().exec = Some(kube::config::ExecConfig {
        command: Some("must-not-execute".into()),
        ..Default::default()
    });
    let mut exec_client = ClusterClient::new(exec_cfg, orka.clone());
    let exec_id = exec_client.contexts().unwrap()[0].id;
    assert_eq!(
        exec_client.discover(exec_id).await.err(),
        Some(failure(
            Check::Credentials,
            CheckResult::UnsupportedCredential
        ))
    );
    assert!(api.calls.lock().unwrap().is_empty());
    let mut client = ClusterClient::new(cfg, orka.clone());
    let id = target(&mut client).await;
    assert!(tls.peers.lock().unwrap().iter().all(|peer| *peer));
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(AccessOutcome::NeedServiceAccount)
    );
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    let access = client
        .connect(id, Some("existing.account".into()))
        .await
        .unwrap();
    assert!(access.expiry.is_some());
    let calls = api.calls.lock().unwrap();
    let requests: Vec<_> = calls.iter().filter(|c| c.1.ends_with("/token")).collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].1,
        "/api/v1/namespaces/agents/serviceaccounts/existing.account/token"
    );
    assert_eq!(requests[0].3["spec"]["expirationSeconds"], 600);
    assert_eq!(requests[0].3["spec"]["audiences"], json!([]));
    assert!(requests[0].2.is_empty());
    let reviews: Vec<_> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.1.ends_with("/selfsubjectaccessreviews"))
        .collect();
    assert_eq!(reviews.len(), 10);
    for (i, call) in reviews {
        assert_eq!(call.2, "Bearer issued-fixture");
        assert!(
            !tls.peers.lock().unwrap()[i],
            "SSAR TLS request still used creator certificate"
        );
    }
    assert!(orka
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.1 == "issued-fixture"));
}
#[tokio::test]
async fn stronger_credentials_do_not_run_exec_or_auth_provider_helpers() {
    let api = Arc::new(ApiFixture::default());
    for auth in [
        json!({"client-certificate-data":"invalid","client-key-data":"invalid","exec":{"command":"must-not-execute"}}),
        json!({"token":"static-fixture","auth-provider":{"name":"gcp","config":{"cmd-path":"must-not-execute"}}}),
        json!({"token":"static-fixture","username":"basic","password":"fixture"}),
    ] {
        let mut client = client(&api, &Arc::new(OrkaFixture::default()), auth);
        let id = client.contexts().unwrap()[0].id;
        assert_eq!(
            client.discover(id).await.err(),
            Some(failure(
                Check::Credentials,
                CheckResult::UnsupportedCredential
            ))
        );
    }
    assert!(api.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn namespace_less_bearer_issues_only_after_confirmed_name() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":""});
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(AccessOutcome::NeedServiceAccount)
    );
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1.ends_with("/token")));
    client.connect(id, Some("existing".into())).await.unwrap();
    let calls = api.calls.lock().unwrap();
    assert_eq!(calls.iter().filter(|c| c.1.ends_with("/token")).count(), 1);
    assert!(calls
        .iter()
        .filter(|c| c.1.ends_with("/selfsubjectaccessreviews"))
        .all(|c| c.2 == "Bearer issued-fixture"));
}
#[tokio::test]
async fn token_file_capture_is_frozen_and_unused_exec_never_runs() {
    use std::io::Write as _;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(b"file-fixture").unwrap();
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(
        &api,
        &orka,
        json!({"tokenFile":file.path(),"exec":{"command":"must-not-execute"}}),
    );
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    std::fs::write(file.path(), b"replacement-fixture").unwrap();
    // kube5 refreshes token files after 50 seconds (60-second cache minus its 10-second margin).
    tokio::time::sleep(std::time::Duration::from_secs(51)).await;
    client.revalidate(&access).await.unwrap();
    assert!(api
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.2 == "Bearer file-fixture"));
    access
        .require_builtins(&["web_fetch".into()])
        .await
        .unwrap();
    assert!(orka
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.1 == "file-fixture"));
}
#[tokio::test]
async fn full_api_address_replacement_cannot_revalidate_existing_access() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let server = api.start();
    let cfg = config(&server, json!({"token":"static-fixture"}));
    let mut client = ClusterClient::new(cfg.clone(), orka.clone());
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    client.revalidate(&access).await.unwrap();
    let mut replacement = cfg;
    replacement.clusters[0].cluster.as_mut().unwrap().server =
        Some(format!("{server}/different-api-root"));
    let changed = ClusterClient::new(replacement, orka);
    let before = api.calls.lock().unwrap().len();
    assert_eq!(
        changed.revalidate(&access).await.err(),
        Some(ClusterError::Unavailable)
    );
    assert_eq!(api.calls.lock().unwrap().len(), before);
}
#[tokio::test]
async fn ssar_http_403_is_denied_and_tokenrequest_503_is_not_retried() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    *api.override_response.lock().unwrap() = Some((
        "/selfsubjectaccessreviews".into(),
        403,
        json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"static-fixture","reason":"Forbidden","code":403}),
    ));
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(failed(Check::Permissions, CheckResult::PermissionDenied))
    );
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":""});
    *api.override_response.lock().unwrap() = Some((
        "/token".into(),
        503,
        json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"issued-fixture","reason":"Unavailable","code":503}),
    ));
    assert_eq!(
        client.connect(id, Some("existing".into())).await.err(),
        Some(failed(Check::ServiceAccount, CheckResult::Unknown))
    );
    assert_eq!(
        api.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.1.ends_with("/token"))
            .count(),
        1
    );
}
#[tokio::test]
async fn binding_changed_before_access_and_missing_builtin_cannot_pass() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    let access = client.connect(id, None).await.unwrap();
    *orka.paged.lock().unwrap() = true;
    access
        .require_builtins(&["web_fetch".into()])
        .await
        .unwrap();
    assert!(orka
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.0 == OrkaResource::Tools && c.3.as_deref() == Some("next")));
    assert_eq!(
        access.require_builtins(&["missing".into()]).await.err(),
        Some(ClusterError::MissingTool)
    );
    *api.replacement.lock().unwrap() = true;
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(failed(Check::ClusterApi, CheckResult::Unknown))
    );
}

type Fresh = Arc<Mutex<Option<Kubeconfig>>>;
fn reading_session(
    cfg: Kubeconfig,
    orka: &Arc<OrkaFixture>,
) -> (bridge_engine::clusters::commands::ClusterSession, Fresh) {
    let fresh: Fresh = Arc::new(Mutex::new(Some(cfg.clone())));
    let reader = {
        let fresh = fresh.clone();
        Arc::new(move || {
            fresh
                .lock()
                .unwrap()
                .clone()
                .ok_or(ClusterError::Unavailable)
        })
    };
    let session = bridge_engine::clusters::commands::ClusterSession::with_reader(
        ClusterClient::new(cfg, orka.clone()),
        reader,
    );
    (session, fresh)
}
fn id_of(value: impl serde::Serialize) -> String {
    serde_json::to_value(value).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}
fn reply(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}
async fn until(done: impl Fn() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while !done() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition was not reached");
}
fn stale() -> Value {
    json!({"check":"configuration","result":"unknown"})
}

#[tokio::test]
async fn reload_rereads_corrected_config_and_invalidates_old_ids_offline() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let cfg = config(&api.start(), json!({"token":"static-fixture"}));
    let (session, fresh) = reading_session(cfg.clone(), &orka);
    let old = session.contexts().await.unwrap();
    let old_ids: Vec<_> = old.iter().map(id_of).collect();
    let old_target = id_of(&session.discover(&old_ids[0]).await.unwrap()[0]);
    let used = reply(session.use_target(&old_target, None).await);
    assert_eq!(used["status"], "connected");
    assert_eq!(used["failure"], Value::Null);
    let mut corrected = cfg;
    corrected.contexts[0].name = "corrected".into();
    *fresh.lock().unwrap() = Some(corrected);
    let calls = (
        api.calls.lock().unwrap().len(),
        orka.calls.lock().unwrap().len(),
    );
    let reloaded = session.reload().await.unwrap();
    assert_eq!(
        reloaded.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["corrected", "exec"]
    );
    assert!(reloaded.iter().all(|c| !old_ids.contains(&id_of(c))));
    // Old IDs and confirmed access are gone; reload reached neither the cluster nor Orka.
    assert_eq!(
        reply(session.discover(&old_ids[0]).await.unwrap_err()),
        stale()
    );
    let old_use = reply(session.use_target(&old_target, None).await);
    assert_eq!(old_use["status"], "unavailable");
    assert_eq!(old_use["failure"], stale());
    assert_eq!(
        (
            api.calls.lock().unwrap().len(),
            orka.calls.lock().unwrap().len()
        ),
        calls
    );
    // An explicit new selection and Use are required.
    let target = id_of(&session.discover(&id_of(&reloaded[0])).await.unwrap()[0]);
    assert_ne!(target, old_target);
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
}

#[tokio::test]
async fn failed_reload_invalidates_and_keeps_the_injected_orka_reader() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let cfg = config(&api.start(), json!({"token":"static-fixture"}));
    let (session, fresh) = reading_session(cfg.clone(), &orka);
    let old_context = id_of(&session.contexts().await.unwrap()[0]);
    let old_target = id_of(&session.discover(&old_context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&old_target, None).await)["status"],
        "connected"
    );
    *fresh.lock().unwrap() = None;
    let calls = api.calls.lock().unwrap().len();
    assert_eq!(reply(session.reload().await.unwrap_err()), stale());
    assert_eq!(
        reply(session.discover(&old_context).await.unwrap_err()),
        stale()
    );
    assert_eq!(
        reply(session.use_target(&old_target, None).await)["failure"],
        stale()
    );
    assert_eq!(reply(session.contexts().await.unwrap_err()), stale());
    assert_eq!(api.calls.lock().unwrap().len(), calls);
    *fresh.lock().unwrap() = Some(cfg);
    let context = id_of(&session.contexts().await.unwrap()[0]);
    assert_ne!(context, old_context);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    let orka_calls = orka.calls.lock().unwrap().len();
    assert_eq!(
        reply(session.use_target(&target, None).await)["status"],
        "connected"
    );
    assert!(orka.calls.lock().unwrap().len() > orka_calls);
}

#[tokio::test]
async fn timed_out_reload_does_not_accumulate_blocked_readers() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let session = bridge_engine::clusters::commands::ClusterSession::with_reader(
        ClusterClient::new(Kubeconfig::default(), Arc::new(OrkaFixture::default())),
        Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_secs(2));
            Ok(Kubeconfig::default())
        }),
    );
    let first = session.clone();
    let first = tokio::spawn(async move { first.reload().await });
    let ready = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) == 0 {
        assert!(std::time::Instant::now() < ready);
        tokio::task::yield_now().await;
    }
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    assert!(first.await.unwrap().is_err());
    let again = session.clone();
    let again = tokio::spawn(async move { again.reload().await });
    while !again.is_finished() && calls.load(Ordering::SeqCst) < 2 {
        assert!(std::time::Instant::now() < ready);
        tokio::task::yield_now().await;
    }
    let outstanding = calls.load(Ordering::SeqCst);
    tokio::time::advance(std::time::Duration::from_secs(11)).await;
    assert!(again.await.unwrap().is_err());
    assert_eq!(
        outstanding, 1,
        "a timed-out reader must not admit another read"
    );
}

#[tokio::test]
async fn reload_waits_for_discovery_and_use_and_late_replies_are_stale() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _fresh) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let services = || {
        let calls = api.calls.lock().unwrap();
        calls.iter().filter(|c| c.1.ends_with("/services")).count()
    };
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let (release, gate) = std::sync::mpsc::channel();
    *api.gate.lock().unwrap() = Some(gate);
    let discovery = tokio::spawn({
        let session = session.clone();
        async move { session.discover(&context).await }
    });
    until(|| services() == 1).await;
    let reload = tokio::spawn({
        let session = session.clone();
        async move { session.reload().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!reload.is_finished(), "reload must queue behind discovery");
    release.send(()).unwrap();
    let late = id_of(&discovery.await.unwrap().unwrap()[0]);
    let contexts = reload.await.unwrap().unwrap();
    assert_eq!(
        reply(session.use_target(&late, None).await)["failure"],
        stale()
    );

    let target = id_of(&session.discover(&id_of(&contexts[0])).await.unwrap()[0]);
    let (release, gate) = std::sync::mpsc::channel();
    *api.gate.lock().unwrap() = Some(gate);
    let before = services();
    let using = tokio::spawn({
        let session = session.clone();
        let target = target.clone();
        async move { reply(session.use_target(&target, None).await) }
    });
    until(|| services() > before).await;
    let reload = tokio::spawn({
        let session = session.clone();
        async move { session.reload().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!reload.is_finished(), "reload must queue behind Use");
    release.send(()).unwrap();
    assert_eq!(using.await.unwrap()["status"], "connected");
    reload.await.unwrap().unwrap();
    // The access confirmed just before reload is no longer selectable.
    assert_eq!(
        reply(session.use_target(&target, None).await)["failure"],
        stale()
    );
}

#[tokio::test]
async fn reload_never_runs_the_plugin_and_keeps_its_shutdown_owner() {
    let api = Arc::new(ApiFixture::default());
    let dir = tempfile::tempdir().unwrap();
    let count = dir.path().join("calls");
    let version = "client.authentication.k8s.io/v1";
    let mut response = credential_fixture::response(version);
    response["status"]["token"] = json!("creator-fixture");
    let mut auth = credential_fixture::exec(version, response);
    auth["exec"]["env"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"RECORD_FILE","value":count}));
    let (session, _fresh) = reading_session(
        config(&api.start(), auth),
        &Arc::new(OrkaFixture::default()),
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    session.discover(&context).await.unwrap();
    assert_eq!(std::fs::read(&count).unwrap().len(), 1);
    let contexts = session.reload().await.unwrap();
    assert_eq!(std::fs::read(&count).unwrap().len(), 1);
    // The retained owner still refuses plugin work after app shutdown.
    session.shutdown().await;
    assert_eq!(
        reply(session.discover(&id_of(&contexts[0])).await.unwrap_err()),
        json!({"check":"credentials","result":"unknown"})
    );
    assert_eq!(std::fs::read(&count).unwrap().len(), 1);
}

#[tokio::test]
async fn discovery_reports_its_first_failed_check() {
    let orka = Arc::new(OrkaFixture::default());
    for (suffix, code, check, result) in [
        (
            "/services",
            401,
            Check::ClusterApi,
            CheckResult::AuthenticationRejected,
        ),
        (
            "/services",
            403,
            Check::ClusterApi,
            CheckResult::PermissionDenied,
        ),
        ("/deployments", 500, Check::ClusterApi, CheckResult::Unknown),
        (
            "agents.core.orka.ai",
            404,
            Check::Compatibility,
            CheckResult::Incompatible,
        ),
        (
            "tools.core.orka.ai",
            403,
            Check::Compatibility,
            CheckResult::PermissionDenied,
        ),
    ] {
        let api = Arc::new(ApiFixture::default());
        *api.override_response.lock().unwrap() = Some((
            suffix.into(),
            code,
            json!({"kind":"Status","apiVersion":"v1","status":"Failure","message":"static-fixture","reason":"Fixture","code":code}),
        ));
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = client.contexts().unwrap()[0].id;
        assert_eq!(
            client.discover(id).await.err(),
            Some(failure(check, result)),
            "{suffix} {code}"
        );
    }
    // A refused connection is established as such; it is not a permission problem.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let mut client = ClusterClient::new(config(&server, json!({"token":"static-fixture"})), orka);
    let id = client.contexts().unwrap()[0].id;
    assert_eq!(
        client.discover(id).await.err(),
        Some(failure(Check::ClusterApi, CheckResult::ConnectionFailed))
    );
}

#[tokio::test]
async fn discovery_deadline_keeps_the_compatibility_check_in_progress() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = client.contexts().unwrap()[0].id;
    let (steps, received) = std::sync::mpsc::channel();
    *api.step_gate.lock().unwrap() = Some(steps);
    tokio::time::pause();
    let task = tokio::spawn(async move { client.discover(id).await });
    let mut last_release = None;
    let wall_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    for stage in 0..4 {
        let (path, release) = loop {
            if let Ok(step) = received.try_recv() {
                break step;
            }
            assert!(
                !task.is_finished(),
                "discovery ended before gated request {stage}"
            );
            assert!(
                std::time::Instant::now() < wall_deadline,
                "gated discovery request not received"
            );
            tokio::task::yield_now().await;
        };
        if stage == 3 {
            assert!(path.contains("/customresourcedefinitions/"));
        }
        tokio::time::advance(std::time::Duration::from_secs(if stage == 3 {
            4
        } else {
            9
        }))
        .await;
        if stage == 3 {
            last_release = Some(release);
        } else {
            let _ = release.send(());
        }
    }
    let result = task.await.unwrap();
    let _ = last_release.unwrap().send(());
    assert_eq!(
        result.err(),
        Some(failure(Check::Compatibility, CheckResult::Unknown))
    );
}

#[tokio::test]
async fn use_deadline_during_pod_opening_keeps_the_connection_stage() {
    let api = Arc::new(ApiFixture::default());
    let mut client = ClusterClient::new(
        config(&api.start(), json!({"token":"static-fixture"})),
        Arc::new(PodOrkaRead),
    );
    let target = target(&mut client).await;
    let (steps, received) = std::sync::mpsc::channel();
    *api.step_gate.lock().unwrap() = Some(steps);
    tokio::time::pause();
    let task = tokio::spawn(async move { client.connect(target, None).await.err() });
    let wall_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let last_release = loop {
        let (path, release) = loop {
            if let Ok(step) = received.try_recv() {
                break step;
            }
            assert!(
                !task.is_finished(),
                "Use ended before the Pod opening request"
            );
            assert!(
                std::time::Instant::now() < wall_deadline,
                "gated Use request not received"
            );
            tokio::task::yield_now().await;
        };
        if path.ends_with("/pods") {
            tokio::time::advance(std::time::Duration::from_millis(5600)).await;
            break release;
        }
        tokio::time::advance(std::time::Duration::from_millis(3500)).await;
        let _ = release.send(());
    };
    let result = task.await.unwrap();
    let _ = last_release.send(());
    assert_eq!(
        result,
        Some(failed(Check::OrkaConnection, CheckResult::Unknown))
    );
}

type Arrange = fn(&ApiFixture, &OrkaFixture);
#[tokio::test]
async fn use_keeps_kubernetes_permission_apart_from_orka_identity() {
    let cases: [(Arrange, Check, CheckResult); 9] = [
        (
            |_, orka| {
                *orka.identity.lock().unwrap() = json!({"authType":"oidc","namespace":"agents"})
            },
            Check::OrkaIdentity,
            CheckResult::Unknown,
        ),
        (
            |_, orka| {
                *orka.identity.lock().unwrap() =
                    json!({"authenticated":"true","authType":"oidc","namespace":"agents"})
            },
            Check::OrkaIdentity,
            CheckResult::Unknown,
        ),
        (
            |_, orka| *orka.agents.lock().unwrap() = Some(json!({"items":"not-an-array"})),
            Check::Agents,
            CheckResult::Unknown,
        ),
        (
            |api, _| {
                *api.override_response.lock().unwrap() = Some((
                    "/selfsubjectaccessreviews".into(),
                    200,
                    json!({"apiVersion":"authorization.k8s.io/v1","kind":"SelfSubjectAccessReview","spec":{},"status":{"allowed":false,"evaluationError":"static-fixture"}}),
                ))
            },
            Check::Permissions,
            CheckResult::Unknown,
        ),
        (
            |_, orka| *orka.identity.lock().unwrap() = json!({"authenticated":false}),
            Check::OrkaIdentity,
            CheckResult::IdentityRejected,
        ),
        (
            |_, orka| {
                *orka.identity.lock().unwrap() =
                    json!({"authenticated":true,"authType":"tokenReview","namespace":"other"})
            },
            Check::OrkaIdentity,
            CheckResult::IdentityRejected,
        ),
        (
            |_, orka| {
                *orka.identity.lock().unwrap() =
                    json!({"authenticated":true,"authType":7,"namespace":"agents"})
            },
            Check::OrkaIdentity,
            CheckResult::Unknown,
        ),
        (
            |_, orka| {
                *orka.identity.lock().unwrap() =
                    json!({"authenticated":true,"authType":"tokenReview"})
            },
            Check::OrkaIdentity,
            CheckResult::Unknown,
        ),
        (
            |_, orka| *orka.providers.lock().unwrap() = Some(json!({"items":[],"metadata":{}})),
            Check::Providers,
            CheckResult::NoProviders,
        ),
    ];
    for (i, (arrange, check, result)) in cases.into_iter().enumerate() {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let (session, _fresh) = reading_session(
            config(&api.start(), json!({"token":"static-fixture"})),
            &orka,
        );
        let context = id_of(&session.contexts().await.unwrap()[0]);
        let target = id_of(&session.discover(&context).await.unwrap()[0]);
        arrange(&api, &orka);
        let view = reply(session.use_target(&target, None).await);
        assert_eq!(view["failure"], reply(failure(check, result)), "case {i}");
        let status = match result {
            CheckResult::IdentityRejected => "denied",
            _ => "unavailable",
        };
        assert_eq!(view["status"], status, "case {i}");
        assert_eq!(api.wire.posts(), 0);
    }
}

#[tokio::test]
async fn need_service_account_has_no_failure() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"tokenReview","namespace":""});
    let (session, _fresh) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let target = id_of(&session.discover(&context).await.unwrap()[0]);
    let view = reply(session.use_target(&target, None).await);
    assert_eq!(view["status"], "needServiceAccount");
    assert_eq!(view["failure"], Value::Null);
}

#[tokio::test]
async fn use_deadline_is_unknown_at_the_provider_page_in_progress() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    *orka.slow_pages.lock().unwrap() = true;
    let start = std::time::Instant::now();
    assert_eq!(
        client.connect(id, None).await.err(),
        Some(failed(Check::Providers, CheckResult::Unknown))
    );
    let elapsed = start.elapsed().as_secs();
    assert!((29..33).contains(&elapsed), "{elapsed}s");
}
