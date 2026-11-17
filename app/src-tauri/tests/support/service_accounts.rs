//! Picker boundary tests share the existing fake Kubernetes HTTP server.
use super::*;
use tokio::time::{Duration, Instant};

const LIST: &str = "/api/v1/namespaces/agents/serviceaccounts";
const REVIEW: &str = "/apis/authorization.k8s.io/v1/namespaces/agents/localsubjectaccessreviews";

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}
fn unknown() -> Value {
    json!({"check":"serviceAccount","result":"unknown"})
}
fn assert_no_credentials_or_orka(api: &ApiFixture, orka: &OrkaFixture) {
    let calls = api.calls.lock().unwrap();
    let token_requests = calls.iter().filter(|c| c.1.ends_with("/token")).count();
    assert_eq!(token_requests, 0);
    assert!(orka.calls.lock().unwrap().is_empty());
    assert!(calls.iter().all(|c| c.0 == "GET" || c.1 == REVIEW));
}
fn page(names: &[&str], after: &str) -> Value {
    json!({"metadata":{"continue":after},"items":names.iter().map(|name| json!({"metadata":{"name":name,"namespace":"agents"}})).collect::<Vec<_>>()})
}

#[tokio::test]
async fn service_account_list_and_preview_accept_kubernetes_long_subdomain_parts() {
    for name in [
        "a".repeat(64),
        format!("{}.runner", "a".repeat(64)),
        "a".repeat(253),
    ] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        *api.override_response.lock().unwrap() = Some((LIST.into(), 200, page(&[&name], "")));
        assert_eq!(
            reply(client.service_accounts(id, deadline()).await.unwrap())["names"],
            json!([name])
        );
        *api.override_response.lock().unwrap() = None;
        assert_eq!(
            reply(
                client
                    .service_account_preview(id, &name, deadline())
                    .await
                    .unwrap()
            )["status"],
            "allowed"
        );
    }
}

#[tokio::test]
async fn service_account_list_never_exports_an_echoed_creator_bearer() {
    for name in ["static-fixture", "prefix-static-fixture-suffix"] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        api.calls.lock().unwrap().clear();
        *api.override_response.lock().unwrap() = Some((LIST.into(), 200, page(&[name], "")));
        assert_eq!(
            reply(client.service_accounts(id, deadline()).await.unwrap_err()),
            unknown()
        );
        assert_no_credentials_or_orka(&api, &orka);
    }
}

#[tokio::test]
async fn service_account_list_preserves_discovery_revalidation_failure() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    api.calls.lock().unwrap().clear();
    *api.override_response.lock().unwrap() = Some((
        "/services".into(),
        403,
        json!({"kind":"Status","apiVersion":"v1","reason":"Forbidden","code":403}),
    ));
    assert_eq!(
        reply(client.service_accounts(id, deadline()).await.unwrap_err()),
        json!({"check":"clusterApi","result":"permissionDenied"})
    );
    assert!(!api.calls.lock().unwrap().iter().any(|c| c.1 == LIST));
    assert_no_credentials_or_orka(&api, &orka);
}

#[tokio::test]
async fn service_account_list_is_namespace_only_and_never_uses_credentials() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    api.calls.lock().unwrap().clear();
    assert_eq!(
        reply(client.service_accounts(id, deadline()).await.unwrap()),
        json!({"names":["runner","default"],"truncated":false})
    );
    let calls = api.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.1.contains("serviceaccounts"))
            .map(|c| (&c.0, c.1.as_str(), c.2.as_str()))
            .collect::<Vec<_>>(),
        vec![(&"GET".to_string(), LIST, "Bearer static-fixture")]
    );
    drop(calls);
    assert_no_credentials_or_orka(&api, &orka);
}

#[tokio::test]
async fn service_account_preview_reviews_only_named_subject_and_exact_matrix() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    let preview = reply(
        client
            .service_account_preview(id, "task-runner", deadline())
            .await
            .unwrap(),
    );
    assert_eq!(preview["status"], "allowed");
    let matrix = [
        ("agents", "create"),
        ("agents", "get"),
        ("agents", "list"),
        ("tasks", "create"),
        ("tasks", "get"),
        ("tasks", "list"),
        ("providers", "get"),
        ("providers", "list"),
        ("tools", "get"),
        ("tools", "list"),
    ];
    let calls = api.calls.lock().unwrap();
    let reviews: Vec<_> = calls.iter().filter(|c| c.1 == REVIEW).collect();
    assert_eq!(reviews.len(), 10);
    assert_eq!(preview["checks"].as_array().unwrap().len(), 10);
    for (i, (resource, verb)) in matrix.iter().enumerate() {
        let call = reviews[i];
        assert_eq!(call.0, "POST");
        assert_eq!(call.2, "Bearer static-fixture");
        assert_eq!(
            call.3,
            json!({"apiVersion":"authorization.k8s.io/v1","kind":"LocalSubjectAccessReview","spec":{"user":"system:serviceaccount:agents:task-runner","groups":["system:serviceaccounts","system:serviceaccounts:agents","system:authenticated"],"resourceAttributes":{"namespace":"agents","group":"core.orka.ai","resource":resource,"verb":verb}}})
        );
        assert_eq!(
            preview["checks"][i],
            json!({"resource":resource,"verb":verb,"status":"allowed"})
        );
    }
    assert!(!calls.iter().any(|c| c.1 == LIST
        || c.1.contains("selfsubject")
        || c.1.contains("/pods")
        || c.1.contains("/replicasets")));
    drop(calls);
    assert_no_credentials_or_orka(&api, &orka);
}

#[tokio::test]
async fn service_account_preview_distinguishes_api_forbidden_denied_and_unknown() {
    for (code, response, want) in [
        (
            403,
            json!({"message":"static-fixture","status":{"allowed":true}}),
            "unknown",
        ),
        (401, json!({"message":"static-fixture"}), "unknown"),
        (500, json!({"message":"static-fixture"}), "unknown"),
        (
            200,
            json!({"status":{"allowed":false,"denied":true}}),
            "denied",
        ),
        (200, json!({"status":{"allowed":false}}), "denied"),
        (
            200,
            json!({"status":{"allowed":true,"denied":true}}),
            "unknown",
        ),
        (
            200,
            json!({"status":{"allowed":true,"evaluationError":"private"}}),
            "unknown",
        ),
        (200, json!({"status":{"allowed":"yes"}}), "unknown"),
        (200, json!({"status":{"denied":false}}), "unknown"),
        (200, json!({}), "unknown"),
    ] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        *api.override_response.lock().unwrap() = Some((REVIEW.into(), code, response));
        let result = reply(
            client
                .service_account_preview(id, "runner", deadline())
                .await
                .unwrap(),
        );
        assert_eq!(result["status"], want);
        assert_eq!(result["checks"].as_array().unwrap().len(), 10);
        assert_eq!(result["checks"][0]["status"], want);
        assert!(result["checks"].as_array().unwrap()[1..]
            .iter()
            .all(|c| c["status"] == "notChecked"));
        assert_eq!(
            api.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.1 == REVIEW)
                .count(),
            1
        );
        assert_no_credentials_or_orka(&api, &orka);
    }
}

#[tokio::test]
async fn service_account_preview_retains_completed_checks_before_stop() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
    let id = target(&mut client).await;
    let mut reviewed = 0;
    *api.response_hook.lock().unwrap() = Some(Box::new(move |path, value| {
        if path == REVIEW {
            reviewed += 1;
            if reviewed == 3 {
                *value = json!({"status":{"allowed":false}});
            }
        }
    }));
    let result = reply(
        client
            .service_account_preview(id, "runner", deadline())
            .await
            .unwrap(),
    );
    assert_eq!(result["status"], "denied");
    let states: Vec<_> = result["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        [
            "allowed",
            "allowed",
            "denied",
            "notChecked",
            "notChecked",
            "notChecked",
            "notChecked",
            "notChecked",
            "notChecked",
            "notChecked"
        ]
    );
    assert_no_credentials_or_orka(&api, &orka);
}

#[tokio::test]
async fn service_account_list_pages_encodes_continuation_and_caps_at_five() {
    for finish in [2, 5, 6] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        let mut n = 0;
        *api.response_hook.lock().unwrap() = Some(Box::new(move |path, value| {
            if path == LIST {
                n += 1;
                let names: Vec<_> = (0..200).map(|i| format!("account-{n}-{i}")).collect();
                *value = page(
                    &names.iter().map(String::as_str).collect::<Vec<_>>(),
                    &if n == finish {
                        String::new()
                    } else {
                        format!("page {n}&more")
                    },
                );
            }
        }));
        let result = reply(client.service_accounts(id, deadline()).await.unwrap());
        assert_eq!(result["truncated"], finish == 6);
        assert_eq!(
            result["names"].as_array().unwrap().len(),
            finish.min(5) * 200
        );
        let queries = api.queries.lock().unwrap();
        let queries: Vec<_> = queries.iter().filter(|q| q.starts_with(LIST)).collect();
        assert_eq!(queries.len(), finish.min(5));
        for (i, q) in queries.iter().enumerate() {
            let url = reqwest::Url::parse(&format!("http://fixture{q}")).unwrap();
            assert_eq!(
                url.query_pairs().find(|(k, _)| k == "limit").unwrap().1,
                "200"
            );
            if i > 0 {
                assert_eq!(
                    url.query_pairs().find(|(k, _)| k == "continue").unwrap().1,
                    format!("page {i}&more")
                );
            }
        }
        assert_no_credentials_or_orka(&api, &orka);
    }
}

#[tokio::test]
async fn service_account_list_rejects_malformed_duplicate_and_unbounded_pages() {
    let too_many = (0..1001)
        .map(|i| json!({"metadata":{"name":format!("account-{i}"),"namespace":"agents"}}))
        .collect::<Vec<_>>();
    for bad in [
        json!({"items":null,"metadata":{}}),
        json!({"items":[]}),
        json!({"items":[],"metadata":{"continue":3}}),
        page(&["same", "same"], ""),
        page(&["Bad"], ""),
        page(&["../escape"], ""),
        json!({"metadata":{},"items":[{"metadata":{"name":"runner","namespace":"other"}}]}),
        json!({"metadata":{},"items":[{"metadata":{"name":"runner"}}]}),
        json!({"metadata":{},"items":too_many[..201]}),
        json!({"metadata":{},"items":too_many}),
    ] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        *api.override_response.lock().unwrap() = Some((LIST.into(), 200, bad));
        assert_eq!(
            reply(client.service_accounts(id, deadline()).await.err().unwrap()),
            unknown()
        );
        assert_no_credentials_or_orka(&api, &orka);
    }
    for duplicate in [false, true] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        let mut n = 0;
        *api.response_hook.lock().unwrap() = Some(Box::new(move |path, value| {
            if path == LIST {
                n += 1;
                *value = page(
                    &[if duplicate {
                        "same"
                    } else if n == 1 {
                        "first"
                    } else {
                        "second"
                    }],
                    "repeated",
                );
            }
        }));
        assert!(client.service_accounts(id, deadline()).await.is_err());
        assert_eq!(
            api.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.1 == LIST)
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn service_account_list_empty_and_finite_http_failures() {
    for (code, want) in [
        (200, ""),
        (401, "authenticationRejected"),
        (403, "permissionDenied"),
        (500, "unknown"),
    ] {
        let api = Arc::new(ApiFixture::default());
        let orka = Arc::new(OrkaFixture::default());
        let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
        let id = target(&mut client).await;
        *api.override_response.lock().unwrap() = Some((LIST.into(), code, page(&[], "")));
        let result = client.service_accounts(id, deadline()).await;
        if code == 200 {
            assert_eq!(
                reply(result.unwrap()),
                json!({"names":[],"truncated":false})
            );
        } else {
            assert_eq!(
                reply(result.err().unwrap()),
                json!({"check":"serviceAccount","result":want})
            );
        }
        assert_no_credentials_or_orka(&api, &orka);
    }
}

#[tokio::test]
async fn service_account_list_and_preview_refuse_oversize_before_body_finishes() {
    for path in [LIST, REVIEW] {
        for chunked in [false, true] {
            let api = Arc::new(ApiFixture::default());
            let orka = Arc::new(OrkaFixture::default());
            let mut client = client(&api, &orka, json!({"token":"static-fixture"}));
            let id = target(&mut client).await;
            let (release, wait) = std::sync::mpsc::channel();
            *api.raw_gate.lock().unwrap() = Some(wait);
            let bytes = if chunked {
                let mut body = if path == LIST {
                    page(&[], "").to_string()
                } else {
                    json!({"status":{"allowed":true}}).to_string()
                };
                body.extend(std::iter::repeat_n(' ', 1024 * 1024 + 1 - body.len()));
                format!(
                    "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:X}\r\n{body}\r\n",
                    body.len()
                )
                .into_bytes()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 1048577\r\n\r\n".to_vec()
            };
            *api.raw_response.lock().unwrap() = Some((path.into(), bytes));
            let read = async {
                if path == LIST {
                    reply(client.service_accounts(id, deadline()).await.err().unwrap())
                } else {
                    reply(
                        client
                            .service_account_preview(id, "runner", deadline())
                            .await
                            .unwrap(),
                    )
                }
            };
            let result = tokio::time::timeout(Duration::from_secs(2), read).await;
            let _ = release.send(());
            let result =
                result.expect("must stop at byte cap without waiting for EOF or JSON decode");
            if path == LIST {
                assert_eq!(result, unknown());
            } else {
                assert_eq!(result["status"], "unknown");
            }
            assert_no_credentials_or_orka(&api, &orka);
        }
    }
}

#[tokio::test]
async fn service_account_invalid_handles_names_and_replaced_installations_send_nothing_sensitive() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let id = id_of(&session.discover(&context).await.unwrap()[0]);
    api.calls.lock().unwrap().clear();
    for bad in ["not-a-target", "00000000-0000-0000-0000-000000000000"] {
        assert!(session.service_accounts(bad).await.is_err());
        assert!(session
            .service_account_preview(bad, "runner")
            .await
            .is_err());
    }
    for bad in ["", "UPPER", "../bad", "other:runner", "a..b"] {
        assert!(session.service_account_preview(&id, bad).await.is_err());
    }
    assert!(api.calls.lock().unwrap().is_empty());
    *api.replacement.lock().unwrap() = true;
    assert!(session.service_accounts(&id).await.is_err());
    assert!(session
        .service_account_preview(&id, "runner")
        .await
        .is_err());
    assert!(!api
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.1 == LIST || c.1 == REVIEW));
    assert_no_credentials_or_orka(&api, &orka);
}

#[tokio::test]
async fn service_account_reads_preserve_confirmed_access_without_tokens() {
    let api = Arc::new(ApiFixture::default());
    let orka = Arc::new(OrkaFixture::default());
    *orka.identity.lock().unwrap() =
        json!({"authenticated":true,"authType":"oidc","namespace":"agents"});
    let (session, _) = reading_session(
        config(&api.start(), json!({"token":"static-fixture"})),
        &orka,
    );
    let context = id_of(&session.contexts().await.unwrap()[0]);
    let id = id_of(&session.discover(&context).await.unwrap()[0]);
    assert_eq!(
        reply(session.use_target(&id, None).await)["status"],
        "connected"
    );
    let before = reply(session.places());
    orka.calls.lock().unwrap().clear();
    session.service_accounts(&id).await.unwrap();
    session
        .service_account_preview(&id, "runner")
        .await
        .unwrap();
    assert_eq!(reply(session.places()), before);
    assert_no_credentials_or_orka(&api, &orka);
}
