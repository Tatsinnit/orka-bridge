use bridge_engine::engine::{decode, types::*, EngineState, EngineTransport};
use std::{path::PathBuf, process::Command, sync::OnceLock};
#[cfg(debug_assertions)]
use tokio::sync::watch;

fn output(stdout: &str, stderr: &str, exit: i32) -> EngineOutput {
    EngineOutput {
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
        exit,
    }
}
fn list() -> EngineAction {
    EngineAction::ListAgents {
        context: "team context;literal".into(),
        namespace: "demo".into(),
    }
}
const AGENTS: &str = r#"{"apiVersion":"core.orka.ai/v1alpha1","kind":"AgentList","metadata":{"resourceVersion":"42"},"items":[{"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":{"name":"helper","namespace":"demo","uid":"uid-1","annotations":{"token":"SECRET_MARKER"}},"spec":{"secret":"SECRET_MARKER"}}]}"#;

#[test]
fn version_projects_only_validated_token_not_annotations_or_diagnostics() {
    let reply = decode(
        &EngineAction::Version {},
        output(
            "kmx v0.9.0-rc.6 (SECRET_MARKER)\n  orka SECRET_MARKER\n  model SECRET_MARKER\n",
            "warning SECRET_MARKER",
            0,
        ),
    )
    .unwrap();
    assert_eq!(
        reply,
        EngineReply::Version {
            version: "v0.9.0-rc.6".into(),
            has_diagnostics: true
        }
    );
    assert!(!serde_json::to_string(&reply)
        .unwrap()
        .contains("SECRET_MARKER"));
    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        serde_json::json!({
            "kind": "version", "version": "v0.9.0-rc.6", "hasDiagnostics": true
        })
    );
}

#[test]
fn list_projects_identities_without_private_resource_fields() {
    let reply = decode(&list(), output(AGENTS, "warning SECRET_MARKER", 0)).unwrap();
    assert_eq!(
        reply,
        EngineReply::Agents {
            agents: vec![AgentSummary {
                name: "helper".into(),
                namespace: "demo".into(),
                uid: "uid-1".into()
            }],
            has_diagnostics: true,
        }
    );
    assert!(!serde_json::to_string(&reply)
        .unwrap()
        .contains("SECRET_MARKER"));
    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        serde_json::json!({
            "kind": "agents", "agents": [{"name": "helper", "namespace": "demo", "uid": "uid-1"}],
            "hasDiagnostics": true
        })
    );
}
#[test]
fn real_empty_list_is_success_but_missing_null_and_truncated_are_not() {
    let empty = r#"{"apiVersion":"core.orka.ai/v1alpha1","kind":"AgentList","items":[]}"#;
    assert_eq!(
        decode(&list(), output(empty, "", 0)).unwrap(),
        EngineReply::Agents {
            agents: vec![],
            has_diagnostics: false
        }
    );
    for bad in [
        "{}",
        "null",
        "{",
        &empty.replace("[]", "null"),
        &AGENTS[..AGENTS.len() - 1],
        &AGENTS.replace("\"uid\":\"uid-1\"", "\"uid\":null"),
        &AGENTS.replace("\"namespace\":\"demo\"", "\"namespace\":\"\""),
        &AGENTS.replace("\"resourceVersion\":\"42\"", "\"continue\":\"next\""),
    ] {
        assert_eq!(
            decode(&list(), output(bad, "", 0)).unwrap_err().kind,
            ErrorKind::InvalidResponse,
            "{bad}"
        );
    }
}
#[test]
fn kubectl_generic_list_accepts_empty_and_populated_envelopes() {
    let empty = r#"{"apiVersion":"v1","kind":"List","metadata":{},"items":[]}"#;
    let populated = r#"{"apiVersion":"v1","kind":"List","metadata":{},"items":[{"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":{"name":"helper","namespace":"demo","uid":"uid-1"}}]}"#;
    assert_eq!(
        decode(&list(), output(empty, "", 0)).unwrap(),
        EngineReply::Agents {
            agents: vec![],
            has_diagnostics: false
        }
    );
    assert_eq!(
        decode(&list(), output(populated, "", 0)).unwrap(),
        EngineReply::Agents {
            agents: vec![AgentSummary {
                name: "helper".into(),
                namespace: "demo".into(),
                uid: "uid-1".into()
            }],
            has_diagnostics: false
        }
    );
}

#[test]
fn kubectl_generic_envelope_does_not_relax_item_validation() {
    let wrong = r#"{"apiVersion":"v1","kind":"List","items":[{"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"helper","namespace":"demo","uid":"uid-1"}}]}"#;
    let missing = r#"{"apiVersion":"v1","kind":"List","items":[{"apiVersion":"core.orka.ai/v1alpha1","kind":"Agent","metadata":{"name":"helper","namespace":"demo"}}]}"#;
    for bad in [wrong, missing] {
        assert_eq!(
            decode(&list(), output(bad, "", 0)).unwrap_err().kind,
            ErrorKind::InvalidResponse
        );
    }
}

#[test]
fn unknown_api_and_kind_are_unsupported_not_empty_success() {
    for bad in [
        AGENTS.replace("core.orka.ai/v1alpha1", "other/v1"),
        AGENTS.replace("AgentList", "TaskList"),
    ] {
        assert_eq!(
            decode(&list(), output(&bad, "", 0)).unwrap_err().kind,
            ErrorKind::Unsupported
        );
    }
}
#[test]
fn nonzero_and_oversized_output_do_not_certify_json_success() {
    let err = decode(&list(), output(AGENTS, "SECRET_MARKER", 9)).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Execution);
    assert!(!serde_json::to_string(&err)
        .unwrap()
        .contains("SECRET_MARKER"));
    for action in [list(), EngineAction::Version {}] {
        let big = "x".repeat(MAX_OUTPUT + 1);
        assert_eq!(
            decode(&action, output(&big, "", 0)).unwrap_err().kind,
            ErrorKind::InvalidResponse
        );
        assert_eq!(
            decode(&action, output(AGENTS, &big, 0)).unwrap_err().kind,
            ErrorKind::InvalidResponse
        );
    }
}
#[test]
fn version_rejects_unidentifiable_first_line_and_bad_utf8() {
    for bad in [
        "",
        "other v1.2.3",
        "kmx SECRET_MARKER",
        "banner\nkmx v1.2.3",
    ] {
        assert_eq!(
            decode(&EngineAction::Version {}, output(bad, "", 0))
                .unwrap_err()
                .kind,
            ErrorKind::InvalidResponse
        );
    }
    assert!(decode(&EngineAction::Version {}, output("kmx dev\n", "", 0)).is_ok());
    let bad = EngineOutput {
        stdout: vec![255],
        stderr: vec![],
        exit: 0,
    };
    assert_eq!(
        decode(&EngineAction::Version {}, bad).unwrap_err().kind,
        ErrorKind::InvalidResponse
    );
}
#[test]
fn request_schema_cannot_accept_executable_or_arbitrary_arguments() {
    for bad in [
        r#"{"action":{"type":"shell","command":"SECRET_MARKER"}}"#,
        r#"{"action":{"type":"version"},"executable":"other"}"#,
        r#"{"action":{"type":"version","argv":["delete"]}}"#,
        r#"{"action":{"type":"listAgents","context":"","namespace":"demo"}}"#,
        r#"{"action":{"type":"listAgents","context":"team","namespace":""}}"#,
        r#"{"action":{"type":"listAgents","context":"  ","namespace":"demo"}}"#,
        r#"{"action":{"type":"listAgents","context":"team","namespace":"\t "}}"#,
    ] {
        assert!(serde_json::from_str::<EngineRequest>(bad).is_err());
    }
}

fn fixture() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("engine-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("fixture.rs");
        std::fs::write(&source, include_str!("support/engine_fixture.rs")).unwrap();
        let bin = dir.join(if cfg!(windows) {
            "engine.exe"
        } else {
            "engine"
        });
        assert!(Command::new("rustc")
            .arg("--edition=2021")
            .arg(&source)
            .arg("-o")
            .arg(&bin)
            .status()
            .unwrap()
            .success());
        bin
    })
    .clone()
}
#[test]
fn factory_ignores_dev_override_in_release_and_uses_it_only_in_debug() {
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "factory_child_probe", "--nocapture"])
        .env("BRIDGE_FACTORY_PROBE", "1")
        .env("BRIDGE_DEV_KMX_PATH", fixture())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}
#[test]
fn factory_child_probe() {
    if std::env::var("BRIDGE_FACTORY_PROBE").as_deref() != Ok("1") {
        return;
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let state = EngineState::configured();
    let result = rt.block_on(state.call(EngineRequest {
        action: EngineAction::Version {},
    }));
    #[cfg(debug_assertions)]
    {
        assert!(matches!(result, Ok(EngineReply::Version { .. })));
        state.shutdown();
        assert_eq!(
            rt.block_on(state.call(EngineRequest {
                action: EngineAction::Version {}
            }))
            .unwrap_err()
            .kind,
            ErrorKind::Execution
        );
    }
    #[cfg(not(debug_assertions))]
    assert_eq!(result.unwrap_err().kind, ErrorKind::Unavailable);
}
#[tokio::test]
async fn vm_boundary_is_explicitly_unavailable() {
    let transport = bridge_engine::engine::vm::VmTransport;
    assert_eq!(
        transport
            .execute(EngineRequest { action: list() })
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Unavailable
    );
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn process_uses_exact_argv_and_keeps_stderr_separate() {
    use bridge_engine::engine::process::ProcessTransport;
    let (_tx, rx) = watch::channel(false);
    let transport = ProcessTransport::new(fixture(), rx);
    let v = transport
        .execute(EngineRequest {
            action: EngineAction::Version {},
        })
        .await
        .unwrap();
    assert_eq!(v.exit, 0);
    assert!(String::from_utf8(v.stdout)
        .unwrap()
        .starts_with("kmx v0.9.0-rc.6"));
    assert_eq!(v.stderr, b"warning SECRET_MARKER\n");
    let o = transport
        .execute(EngineRequest { action: list() })
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "cluster\norka\nlist\n--context\nteam context;literal\n--namespace\ndemo\n-o\njson"
    );
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn process_nonzero_missing_and_output_bounds_fail_without_raw_diagnostics() {
    use bridge_engine::engine::process::ProcessTransport;
    let (_tx, rx) = watch::channel(false);
    let missing = ProcessTransport::new(fixture().with_file_name("missing"), rx.clone());
    assert_eq!(
        missing
            .execute(EngineRequest { action: list() })
            .await
            .unwrap_err()
            .kind,
        ErrorKind::Unavailable
    );
    let transport = ProcessTransport::new(fixture(), rx);
    for mode in ["oversize", "oversize-stderr"] {
        let request = EngineRequest {
            action: EngineAction::ListAgents {
                context: mode.into(),
                namespace: "demo".into(),
            },
        };
        assert_eq!(
            transport.execute(request).await.unwrap_err().kind,
            ErrorKind::InvalidResponse
        );
    }
    let request = EngineRequest {
        action: EngineAction::ListAgents {
            context: "exit".into(),
            namespace: "demo".into(),
        },
    };
    let o = transport.execute(request).await.unwrap();
    assert_eq!(o.exit, 7);
    assert_eq!(decode(&list(), o).unwrap_err().kind, ErrorKind::Execution);
}
#[cfg(debug_assertions)]
#[tokio::test]
async fn closing_engine_cancels_and_reaps_owned_process() {
    use bridge_engine::engine::process::ProcessTransport;
    let (tx, rx) = watch::channel(false);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid_path = fixture().with_file_name(format!("blocked-{nonce}.pid"));
    assert!(
        !pid_path.exists(),
        "readiness marker must belong to this invocation"
    );
    let transport = ProcessTransport::new(fixture(), rx);
    let request = EngineRequest {
        action: EngineAction::ListAgents {
            context: "block".into(),
            namespace: pid_path.to_string_lossy().into(),
        },
    };
    let task = tokio::spawn(async move { transport.execute(request).await });
    let pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&pid_path) {
                if let Ok(pid) = text.trim().parse::<u32>() {
                    break pid;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tx.send_replace(true);
    let err = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Execution);
    #[cfg(unix)]
    {
        assert!(!Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
    }
    #[cfg(windows)]
    {
        let listing = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .unwrap();
        assert!(listing.status.success());
        assert!(
            !String::from_utf8_lossy(&listing.stdout)
                .lines()
                .any(|line| {
                    line.split(',')
                        .nth(1)
                        .is_some_and(|field| field.trim_matches('"') == pid.to_string())
                }),
            "cancelled child is still listed"
        );
    }
    std::fs::remove_file(pid_path).unwrap();
}
