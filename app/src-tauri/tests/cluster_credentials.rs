use bridge_engine::clusters::*;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

#[path = "support/exec_credential_fixture.rs"]
mod fixture;
use fixture::*;
fn config(auth: Value) -> kube::config::Kubeconfig {
    serde_json::from_value(json!({"clusters":[{"name":"cluster","cluster":{"server":"https://fixture.invalid"}}],"users":[{"name":"user","user":auth}],"contexts":[{"name":"selected","context":{"cluster":"cluster","user":"user"}}]})).unwrap()
}
fn client(cfg: kube::config::Kubeconfig) -> ClusterClient {
    ClusterClient::new(cfg, Arc::new(PodOrkaRead))
}
async fn resolve(client: &ClusterClient) -> Result<ResolvedCreator, ClusterError> {
    client
        .resolve_creator(
            client.contexts().unwrap()[0].id,
            Instant::now() + Duration::from_secs(10),
        )
        .await
}

#[tokio::test]
async fn configured_path_origin_and_drop_env_contract() {
    let version = "client.authentication.k8s.io/v1";
    let mut auth = exec(version, response(version));
    auth["exec"]["command"] = json!(plugin().file_name().unwrap().to_str().unwrap());
    auth["exec"]["env"].as_array_mut().unwrap().extend([
        json!({"name":"PATH","value":plugin().parent().unwrap()}),
        json!({"name":"REMOVE_ME","value":"fixture-value"}),
        json!({"name":"EXPECT_ABSENT","value":"REMOVE_ME"}),
    ]);
    let mut cfg = config(auth);
    cfg.auth_infos[0]
        .auth_info
        .as_mut()
        .unwrap()
        .exec
        .as_mut()
        .unwrap()
        .drop_env = Some(vec!["REMOVE_ME".into()]);
    assert!(resolve(&client(cfg)).await.is_ok());
    let mut auth = exec(version, response(version));
    auth["exec"]["command"] = json!(format!(
        ".{}{}",
        std::path::MAIN_SEPARATOR,
        plugin().file_name().unwrap().to_str().unwrap()
    ));
    let cfg = config(auth);
    let file = plugin().parent().unwrap().join("fixture-kubeconfig.json");
    std::fs::write(&file, serde_json::to_vec(&cfg).unwrap()).unwrap();
    assert!(
        resolve(&client(kube::config::Kubeconfig::read_from(&file).unwrap()))
            .await
            .is_ok()
    );
}

#[cfg(windows)]
#[tokio::test]
async fn plugin_launch_has_no_window_or_inherited_console() {
    // CI may start detached. Isolate an explicitly attached parent so inheriting
    // its console fails even on a runner without an interactive desktop.
    const CHILD: &str = "BRIDGE_TEST_CONSOLE_PARENT";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "plugin_launch_has_no_window_or_inherited_console",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AllocConsole() -> i32;
        fn FreeConsole() -> i32;
        fn GetConsoleProcessList(processes: *mut u32, count: u32) -> u32;
    }
    unsafe {
        AllocConsole();
    }
    let mut pid = 0;
    assert!(
        unsafe { GetConsoleProcessList(&mut pid, 1) } > 0,
        "fixture parent has no console"
    );
    let version = "client.authentication.k8s.io/v1";
    let mut auth = exec(version, response(version));
    auth["exec"]["args"] = json!(["no-console"]);
    auth["exec"]["env"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"CONSOLE_PARENT_PID","value":std::process::id().to_string()}));
    let result = resolve(&client(config(auth))).await;
    unsafe {
        FreeConsole();
    }
    match result.unwrap().credential {
        Credential::Bearer { value, .. } => assert_eq!(value, "fixture-console-isolated"),
        _ => panic!("missing console fixture observation"),
    }
}

#[tokio::test]
async fn exec_v1_and_beta_capture_bearer_and_actual_expiry() {
    for version in [
        "client.authentication.k8s.io/v1",
        "client.authentication.k8s.io/v1beta1",
    ] {
        let creator = resolve(&client(config(exec(version, response(version)))))
            .await
            .unwrap();
        assert!(creator.config.auth_info.exec.is_none());
        match creator.credential {
            Credential::Bearer {
                value,
                known_expiry,
            } => {
                assert!(value == "fixture-bearer");
                assert_eq!(known_expiry.unwrap().0.to_string(), "2099-01-01T00:00:00Z");
            }
            _ => panic!("missing bearer"),
        }
    }
}
#[tokio::test]
async fn malformed_expired_and_mismatched_results_never_construct_access() {
    let version = "client.authentication.k8s.io/v1";
    for value in [
        json!({}),
        json!({"kind":"Other","apiVersion":version,"status":{"token":"x"}}),
        response("client.authentication.k8s.io/v1beta1"),
        json!({"kind":"ExecCredential","apiVersion":version,"status":{"token":""}}),
        json!({"kind":"ExecCredential","apiVersion":version,"status":{"token":"x","expirationTimestamp":"2020-01-01T00:00:00Z"}}),
        json!({"kind":"ExecCredential","apiVersion":version,"status":{"token":"x","expirationTimestamp":"not-time"}}),
        json!({"kind":"ExecCredential","apiVersion":version,"status":{"clientCertificateData":"pem"}}),
        json!({"kind":"ExecCredential","apiVersion":version,"status":{"token":"x\nAuthorization: bad"}}),
    ] {
        assert_eq!(
            resolve(&client(config(exec(version, value)))).await.err(),
            Some(ClusterError::Unavailable)
        );
    }
}
#[tokio::test]
async fn unused_empty_certificate_fields_do_not_hide_a_valid_token() {
    let version = "client.authentication.k8s.io/v1";
    let mut value = response(version);
    value["status"]["clientCertificateData"] = json!("");
    value["status"]["clientKeyData"] = json!("");
    assert!(matches!(
        resolve(&client(config(exec(version, value))))
            .await
            .unwrap()
            .credential,
        Credential::Bearer { .. }
    ));
}

#[tokio::test]
async fn always_and_invalid_contract_do_not_start_plugin() {
    for auth in [
        json!({"exec":{"apiVersion":"client.authentication.k8s.io/v1","command":"must-not-start","interactiveMode":"Always"}}),
        json!({"exec":{"apiVersion":"client.authentication.k8s.io/v1","command":"must-not-start"}}),
        json!({"exec":{"apiVersion":"unknown","command":"must-not-start","interactiveMode":"Never"}}),
    ] {
        assert_eq!(
            resolve(&client(config(auth))).await.err(),
            Some(ClusterError::UnsupportedCredential)
        );
    }
}
#[tokio::test]
async fn child_deadline_output_and_exit_failures_are_finite() {
    let version = "client.authentication.k8s.io/v1";
    for mode in ["hang", "overflow", "stderr", "fail"] {
        let mut auth = exec(version, response(version));
        auth["exec"]["args"] = json!([mode]);
        let client = client(config(auth));
        let start = Instant::now();
        let result = client
            .resolve_creator(
                client.contexts().unwrap()[0].id,
                start + Duration::from_millis(300),
            )
            .await;
        assert_eq!(result.err(), Some(ClusterError::Unavailable));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}
#[tokio::test]
async fn terminal_shutdown_reaps_inflight_child_and_refuses_new_resolution() {
    let version = "client.authentication.k8s.io/v1";
    let mut auth = exec(version, response(version));
    auth["exec"]["args"] = json!(["hang"]);
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    auth["exec"]["env"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"PID_FILE","value":pid_file}));
    let client = client(config(auth));
    let session = commands::ClusterSession::new(client.clone());
    let running = session.clone();
    let context_id = serde_json::to_value(&client.contexts().unwrap()[0]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let job = tokio::spawn(async move { running.discover(&context_id).await.is_err() });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !pid_file.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let vm = bridge_engine::local_vm::LocalVm::unavailable();
    let engine = bridge_engine::engine::EngineState::with_transport(vm.clone());
    let send = bridge_engine::send::SendOwner::new(
        Arc::new(bridge_engine::agent_folder::AgentFolder::new(None)),
        session.clone(),
    );
    let shutdown = bridge_engine::shutdown::TerminalShutdown::default();
    assert!(shutdown.request(&vm, &engine));
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        shutdown.finish(&session, &send, &vm)
    )
    .await
    .unwrap());
    assert!(shutdown.completed());
    #[cfg(unix)]
    {
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "child still exists, possibly as a zombie"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
    assert!(job.await.unwrap());
    assert_eq!(
        resolve(&client).await.err(),
        Some(ClusterError::Unavailable)
    );
}
#[tokio::test]
async fn static_and_token_file_do_not_execute_unused_plugin() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut file, b"fixture-file-bearer").unwrap();
    for auth in [
        json!({"token":"fixture-static","exec":{"command":"must-not-start"}}),
        json!({"tokenFile":file.path(),"exec":{"command":"must-not-start"}}),
    ] {
        let creator = resolve(&client(config(auth))).await.unwrap();
        assert!(matches!(
            creator.credential,
            Credential::Bearer {
                known_expiry: None,
                ..
            }
        ));
        assert!(creator.config.auth_info.exec.is_none());
        assert!(creator.config.auth_info.token_file.is_none());
    }
}
#[tokio::test]
async fn cluster_info_ifavailable_and_explicit_proxy_are_preserved() {
    let version = "client.authentication.k8s.io/v1";
    let mut auth = exec(version, response(version));
    auth["exec"]["interactiveMode"] = json!("IfAvailable");
    auth["exec"]["provideClusterInfo"] = json!(true);
    let info = json!({"apiVersion":version,"kind":"ExecCredential","spec":{"interactive":false,"cluster":{"server":"https://fixture.invalid","insecure-skip-tls-verify":true,"proxy-url":"http://127.0.0.1:9988","tls-server-name":"fixture-server"}}});
    auth["exec"]["env"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"EXPECTED_INFO","value":info.to_string()}));
    let mut cfg = config(auth);
    let cluster = cfg.clusters[0].cluster.as_mut().unwrap();
    cluster.insecure_skip_tls_verify = Some(true);
    cluster.tls_server_name = Some("fixture-server".into());
    cluster.proxy_url = Some("http://127.0.0.1:9988".into());
    let creator = resolve(&client(cfg)).await.unwrap();
    assert!(creator.config.accept_invalid_certs);
    assert_eq!(
        creator.config.tls_server_name.as_deref(),
        Some("fixture-server")
    );
    assert_eq!(
        creator.config.proxy_url.unwrap().to_string(),
        "http://127.0.0.1:9988/"
    );
}
