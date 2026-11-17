use bridge_runtime_protocol::*;

const MODEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub enum ModelCommand {
    Connect {
        generation: Uuid,
        selection: ModelSelection,
    },
    Disconnect {
        connection: ModelConnection,
    },
}
pub struct Outcome {
    pub connection: Option<ModelConnection>,
    pub success: bool,
    pub diagnostic: Option<ModelDiagnostic>,
}
pub trait Runner {
    fn execute(&mut self, command: &ModelCommand) -> Outcome;
}
pub struct Kmx {
    installation: Uuid,
}
impl Kmx {
    pub fn new(installation: Uuid) -> Self {
        Self { installation }
    }
    fn command(&self, command: &ModelCommand) -> std::process::Command {
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let cluster = format!("bridge-{}", self.installation.simple());
        let mut child = Command::new("/usr/local/bin/kmx");
        child
            .args(argv(command, &cluster))
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
            .env("HOME", "/root")
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C")
            .env("KUBECONFIG", "/var/lib/bridge/kubeconfig")
            .env("KMX_HOME", "/var/lib/bridge/kmx")
            .env("KIND_CLUSTER", cluster)
            .env("CONTAINER_ENGINE", "docker")
            .env("KIND_EXPERIMENTAL_PROVIDER", "docker")
            .env("DOCKER_HOST", "unix:///var/run/docker.sock")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        child
    }
}
pub struct ModelLifecycle<R = Kmx> {
    runner: R,
    phase: ModelPhase,
    connection: Option<ModelConnection>,
    attempted: Option<(Uuid, ModelSelection)>,
    diagnostic: Option<ModelDiagnostic>,
    // Same-boot acknowledgment survives a failed host metadata write, not a VM reboot.
    retained: Option<(ModelConnection, bool)>,
}
impl Runner for Kmx {
    fn execute(&mut self, command: &ModelCommand) -> Outcome {
        let start = std::time::Instant::now();
        match self.command(command).spawn() {
            Ok(child) => collected_reply(
                command,
                crate::engine::collect_diagnostic(child, MODEL_TIMEOUT),
            ),
            Err(_) => collected_reply(
                command,
                crate::engine::Collected {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    elapsed_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                    exit_code: None,
                    error: Some(std::io::ErrorKind::Other.into()),
                },
            ),
        }
    }
}
fn collected_reply(command: &ModelCommand, collected: crate::engine::Collected) -> Outcome {
    // A terminal success cannot discard stage evidence until collection also succeeded.
    let mut out = parse_reply(
        command,
        if collected.error.is_some() {
            -1
        } else {
            collected.exit_code.unwrap_or(-1)
        },
        &collected.stdout,
    );
    if let Some(error) = collected.error {
        out.success = false;
        out.diagnostic
            .get_or_insert(ModelDiagnostic {
                classification: ModelDiagnosticClass::Process,
                elapsed_ms: 0,
                exit_code: None,
                stage: None,
                error_code: None,
                stderr: None,
            })
            .classification = match error.kind() {
            std::io::ErrorKind::TimedOut => ModelDiagnosticClass::Timeout,
            std::io::ErrorKind::FileTooLarge => ModelDiagnosticClass::OutputLimit,
            _ => ModelDiagnosticClass::Process,
        };
    }
    if let Some(diagnostic) = &mut out.diagnostic {
        diagnostic.elapsed_ms = collected.elapsed_ms;
        diagnostic.exit_code = collected.exit_code;
        if !collected.stderr.is_empty() {
            // Match the entire message. In particular, never strip Error:/ANSI/provider prefixes.
            let bytes = collected
                .stderr
                .strip_suffix(b"\n")
                .unwrap_or(&collected.stderr);
            diagnostic.stderr = Some(
                std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|s| serde_json::from_value(serde_json::Value::String(s.into())).ok())
                    .unwrap_or(ModelStderr::Withheld),
            );
        }
    }
    out
}
fn argv(command: &ModelCommand, cluster: &str) -> Vec<String> {
    let mut args: Vec<String> = ["--container-engine=docker", "cluster", "orka", "model"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    args.insert(0, format!("--context=kind-{cluster}"));
    match command {
        ModelCommand::Connect {
            generation,
            selection,
        } => {
            args.extend([
                "connect".into(),
                "--namespace=orka-system".into(),
                format!(
                    "--route={}",
                    if selection.vendor.is_some() {
                        "key"
                    } else {
                        "copilot"
                    }
                ),
                format!("--provider={}", selection.provider),
                format!("--model={}", selection.model),
                format!("--credential-generation={generation}"),
            ]);
            if let Some(vendor) = selection.vendor {
                args.push(format!("--vendor={}", vendor.as_str()));
            }
        }
        ModelCommand::Disconnect { connection } => args.extend([
            "disconnect".into(),
            "--namespace=orka-system".into(),
            format!("--provider={}", connection.selection.provider),
            format!("--connection={}", connection.id),
        ]),
    }
    args.push("-o=jsonl".into());
    args
}
impl<R: Runner> ModelLifecycle<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            phase: ModelPhase::Idle,
            connection: None,
            attempted: None,
            diagnostic: None,
            retained: None,
        }
    }
    pub fn ready_connection(&self) -> Option<&ModelConnection> {
        self.connection
            .as_ref()
            .filter(|_| self.phase == ModelPhase::Ready)
    }
    pub fn allows_credential_mutation(&self) -> bool {
        self.phase == ModelPhase::Idle && self.retained.as_ref().is_none_or(|(_, done)| *done)
    }
    // The serving loop supplies actual store status and guards physical-root dispatch.
    pub fn handle(
        &mut self,
        credential: CredentialState,
        request: &RequestBody,
    ) -> Option<ResultBody> {
        match request {
            RequestBody::ModelStatus {} => {}
            RequestBody::ModelDisconnectRetained { connection } => {
                // No token or current-boot model may coexist with prior-boot cleanup.
                let pristine = self.phase == ModelPhase::Idle
                    && self.attempted.is_none()
                    && !credential.installed
                    && credential.generation.is_none();
                if !pristine
                    || self
                        .retained
                        .as_ref()
                        .is_some_and(|(known, _)| known != connection)
                {
                    return Some(ResultBody::Error {
                        code: ErrorCode::Unavailable,
                    });
                }
                if !self.retained.as_ref().is_some_and(|(_, done)| *done) {
                    self.retained = Some((connection.clone(), false));
                    let result = self.runner.execute(&ModelCommand::Disconnect {
                        connection: connection.clone(),
                    });
                    let done = result.success && result.connection.as_ref() == Some(connection);
                    self.retained = Some((connection.clone(), done));
                    if !done {
                        return Some(ResultBody::Error {
                            code: ErrorCode::Unavailable,
                        });
                    }
                }
                return Some(ResultBody::Ready {});
            }
            RequestBody::ModelConnect {
                generation,
                selection,
            } => {
                let same_attempt = self
                    .attempted
                    .as_ref()
                    .is_some_and(|(g, s)| g == generation && s == selection);
                if (self.phase == ModelPhase::Idle
                    || (self.phase == ModelPhase::Unknown
                        && self.connection.is_none()
                        && same_attempt))
                    && self.retained.as_ref().is_none_or(|(_, done)| *done)
                    && credential.installed
                    && credential.profile == selection.credential_profile()
                    && credential.generation == Some(*generation)
                    && selection.valid()
                {
                    // Only an explicit identical attempt may consult kmx's retained-state guard.
                    self.attempted = Some((*generation, selection.clone()));
                    self.phase = ModelPhase::Unknown;
                    let result = self.runner.execute(&ModelCommand::Connect {
                        generation: *generation,
                        selection: selection.clone(),
                    });
                    self.connection = result.connection;
                    self.diagnostic = result.diagnostic;
                    if result.success && self.connection.is_some() {
                        self.phase = ModelPhase::Ready;
                    }
                }
            }
            RequestBody::ModelDisconnect { connection } => {
                if self.phase != ModelPhase::Idle {
                    if let Some(known) = self.connection.clone().filter(|c| c.id == *connection) {
                        self.phase = ModelPhase::Unknown;
                        let result = self.runner.execute(&ModelCommand::Disconnect {
                            connection: known.clone(),
                        });
                        self.diagnostic = result.diagnostic;
                        if result.success && result.connection == Some(known) {
                            self.phase = ModelPhase::Idle;
                            self.connection = None;
                            self.attempted = None;
                        }
                    }
                }
            }
            _ => return None,
        }
        Some(ResultBody::ModelState(ModelState {
            retained_cleanup: true,
            phase: self.phase,
            connection: self.connection.clone(),
            credential,
            diagnostic: self.diagnostic.clone(),
        }))
    }
}

fn canonical_uuid(value: &serde_json::Value) -> Option<Uuid> {
    let text = value.as_str()?;
    let id = Uuid::parse_str(text).ok()?;
    (id.to_string() == text && id.get_version_num() == 4 && id.as_bytes()[8] & 0xc0 == 0x80)
        .then_some(id)
}
fn parse_reply(command: &ModelCommand, exit: i32, bytes: &[u8]) -> Outcome {
    use serde_json::Value;
    let (operation, expected, selection, generation) = match command {
        ModelCommand::Connect {
            generation,
            selection,
        } => (
            "cluster.orka.model.connect",
            "model_ready",
            selection,
            *generation,
        ),
        ModelCommand::Disconnect { connection } => (
            "cluster.orka.model.disconnect",
            "gateway_stopped",
            &connection.selection,
            connection.generation,
        ),
    };
    let mut out = Outcome {
        connection: None,
        success: false,
        diagnostic: Some(ModelDiagnostic {
            classification: ModelDiagnosticClass::InvalidReply,
            elapsed_ms: 0,
            exit_code: Some(exit),
            stage: None,
            error_code: None,
            stderr: None,
        }),
    };
    let valid = (|| {
        let oversized = bytes.len() > MAX_FILE;
        let bytes = &bytes[..bytes.len().min(MAX_FILE)];
        let (mut invocation, mut terminal, mut success) = (None, false, false);
        // Validate each row independently so an incomplete/invalid tail cannot erase prior evidence.
        for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
            let row: Value = serde_json::from_slice(line).ok()?;
            let id = canonical_uuid(&row["operationId"])?;
            if terminal
                || row["schemaVersion"] != 1
                || row["operation"] != operation
                || row["seq"].as_u64() != Some(index as u64 + 1)
                || *invocation.get_or_insert(id) != id
            {
                return None;
            }
            match row["kind"].as_str()? {
                "kmx.event" => {
                    if let Ok(stage) = serde_json::from_value(row["stage"].clone()) {
                        out.diagnostic.as_mut()?.stage = Some(stage);
                    }
                }
                "kmx.result" => {
                    terminal = true;
                    let diagnostic = out.diagnostic.as_mut()?;
                    if let Ok(stage) = serde_json::from_value(row["error"]["stage"].clone()) {
                        diagnostic.stage = Some(stage);
                    }
                    diagnostic.error_code =
                        serde_json::from_value(row["error"]["code"].clone()).ok();
                    if !row["connection"].is_null() {
                        let c = &row["connection"];
                        let id = canonical_uuid(&c["id"])?;
                        let identity_matches = match selection.vendor {
                            Some(vendor) => c["route"] == "key" && c["vendor"] == vendor.as_str(),
                            None => c["route"] == "copilot" && c.get("vendor").is_none(),
                        };
                        if !identity_matches
                            || c["provider"] != selection.provider
                            || c["model"] != selection.model
                            || c["credentialGeneration"] != generation.to_string()
                        {
                            return None;
                        }
                        out.connection = Some(ModelConnection {
                            id,
                            selection: selection.clone(),
                            generation,
                        });
                        if matches!(command, ModelCommand::Disconnect { connection } if connection.id != id)
                        {
                            return None;
                        }
                    }
                    let outcome = row["outcome"].as_str()?;
                    if !matches!(outcome, "complete" | "failed" | "cancelled") {
                        return None;
                    }
                    success = exit == 0
                        && row["exitCode"].as_i64() == Some(0)
                        && outcome == "complete"
                        && out.connection.is_some()
                        && row["connection"]["state"] == expected;
                }
                _ => return None,
            }
        }
        (!oversized && terminal).then_some(success)
    })();
    out.success = valid.unwrap_or(false);
    if out.success {
        out.diagnostic = None;
    } else if valid.is_some() {
        out.diagnostic
            .as_mut()
            .expect("failure diagnostic")
            .classification = ModelDiagnosticClass::Exit;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake {
        calls: Vec<ModelCommand>,
        outcomes: VecDeque<Outcome>,
    }
    impl Runner for Fake {
        fn execute(&mut self, command: &ModelCommand) -> Outcome {
            self.calls.push(command.clone());
            self.outcomes
                .pop_front()
                .expect("unexpected external command")
        }
    }
    fn id(n: u8) -> Uuid {
        Uuid::parse_str(&format!("00000000-0000-4000-8000-{n:012}")).unwrap()
    }
    fn selection() -> ModelSelection {
        ModelSelection {
            vendor: None,
            provider: "copilot".into(),
            model: "gpt-4.1".into(),
        }
    }
    fn credential() -> CredentialState {
        CredentialState {
            profile: CREDENTIAL_PROFILE,
            installed: true,
            generation: Some(id(1)),
        }
    }
    fn connection() -> ModelConnection {
        ModelConnection {
            id: id(2),
            selection: selection(),
            generation: id(1),
        }
    }
    fn connect() -> RequestBody {
        RequestBody::ModelConnect {
            generation: id(1),
            selection: selection(),
        }
    }
    fn lifecycle(outcomes: Vec<Outcome>) -> ModelLifecycle<Fake> {
        ModelLifecycle::new(Fake {
            calls: vec![],
            outcomes: outcomes.into(),
        })
    }
    fn state(m: &mut ModelLifecycle<Fake>, request: &RequestBody) -> ModelState {
        match m.handle(credential(), request).unwrap() {
            ResultBody::ModelState(s) => s,
            _ => panic!("expected model state"),
        }
    }
    // fc811: machine_output.go envelope + machine_setup.go flattened OrkaModelResult.
    const READY: &str = r#"{"schemaVersion":1,"kind":"kmx.event","operation":"cluster.orka.model.connect","operationId":"00000000-0000-4000-8000-000000000004","seq":1,"elapsedMs":0,"event":"stage.started","stage":"gateway"}
{"schemaVersion":1,"kind":"kmx.result","operation":"cluster.orka.model.connect","operationId":"00000000-0000-4000-8000-000000000004","seq":2,"elapsedMs":2,"outcome":"complete","verdict":"not_applicable","exitCode":0,"mode":"apply","effects":[],"connection":{"id":"00000000-0000-4000-8000-000000000002","route":"copilot","provider":"copilot","model":"gpt-4.1","credentialGeneration":"00000000-0000-4000-8000-000000000001","state":"model_ready"}}
"#;
    fn command() -> ModelCommand {
        ModelCommand::Connect {
            generation: id(1),
            selection: selection(),
        }
    }
    fn key_command(vendor: ModelVendor) -> ModelCommand {
        ModelCommand::Connect {
            generation: id(1),
            selection: ModelSelection {
                vendor: Some(vendor),
                provider: "workspace-model".into(),
                model: "gpt-4.1".into(),
            },
        }
    }
    fn key_reply(vendor: ModelVendor) -> String {
        READY
            .replace(
                r#""route":"copilot""#,
                &format!(r#""route":"key","vendor":"{}""#, vendor.as_str()),
            )
            .replace(r#""provider":"copilot""#, r#""provider":"workspace-model""#)
    }
    #[test]
    fn key_routes_bind_vendor_profile_and_reply_identity_through_disconnect() {
        for vendor in [ModelVendor::Openai, ModelVendor::Anthropic] {
            let command = key_command(vendor);
            let args = argv(&command, "bridge-owned");
            assert_eq!(args[0], "--context=kind-bridge-owned");
            for want in [
                "--container-engine=docker",
                "--namespace=orka-system",
                "--route=key",
                "--provider=workspace-model",
                "--model=gpt-4.1",
                "-o=jsonl",
            ] {
                assert!(args.iter().any(|a| a == want), "{want}");
            }
            assert!(args.contains(&format!("--vendor={}", vendor.as_str())));
            assert!(!args.iter().any(|a| a.contains("synthetic-key")));
            let reply = key_reply(vendor);
            let outcome = parse_reply(&command, 0, reply.as_bytes());
            assert!(outcome.success);
            let retained = outcome.connection.clone().unwrap();
            assert_eq!(retained.selection.vendor, Some(vendor));
            let disconnect = ModelCommand::Disconnect {
                connection: retained.clone(),
            };
            let stopped = reply
                .replace(
                    "cluster.orka.model.connect",
                    "cluster.orka.model.disconnect",
                )
                .replace("model_ready", "gateway_stopped");
            assert!(parse_reply(&disconnect, 0, stopped.as_bytes()).success);
            for bad in [
                reply.replace(r#""route":"key""#, r#""route":"copilot""#),
                reply.replace(&format!(r#","vendor":"{}""#, vendor.as_str()), ""),
                reply.replace(
                    &format!(r#""vendor":"{}""#, vendor.as_str()),
                    r#""vendor":"other""#,
                ),
            ] {
                let out = parse_reply(&command, 0, bad.as_bytes());
                assert!(!out.success);
                assert!(out.connection.is_none());
                let bad_stop = bad
                    .replace(
                        "cluster.orka.model.connect",
                        "cluster.orka.model.disconnect",
                    )
                    .replace("model_ready", "gateway_stopped");
                assert!(!parse_reply(&disconnect, 0, bad_stop.as_bytes()).success);
            }
            let ModelCommand::Connect { selection, .. } = command else {
                unreachable!()
            };
            let request = RequestBody::ModelConnect {
                generation: id(1),
                selection,
            };
            let mut m = lifecycle(vec![outcome]);
            m.handle(credential(), &request);
            assert!(
                m.runner.calls.is_empty(),
                "key must not consume Copilot credential"
            );
            let key_credential = CredentialState {
                profile: 2,
                ..credential()
            };
            m.handle(key_credential.clone(), &connect());
            assert!(
                m.runner.calls.is_empty(),
                "Copilot must not consume key credential"
            );
            let Some(ResultBody::ModelState(s)) = m.handle(key_credential, &request) else {
                panic!("state")
            };
            assert_eq!(s.phase, ModelPhase::Ready);
            let mut reboot = lifecycle(vec![parse_reply(&disconnect, 0, stopped.as_bytes())]);
            assert_eq!(
                reboot.handle(
                    pristine(),
                    &RequestBody::ModelDisconnectRetained {
                        connection: retained
                    }
                ),
                Some(ResultBody::Ready {})
            );
        }
        for vendor in ["", "openai"] {
            let bad = READY.replace(
                r#""route":"copilot""#,
                &format!(r#""route":"copilot","vendor":"{vendor}""#),
            );
            assert!(!parse_reply(&command(), 0, bad.as_bytes()).success);
        }
    }
    #[test]
    fn key_failures_keep_safe_codes_and_stderr_without_vendor_output() {
        for vendor in [ModelVendor::Openai, ModelVendor::Anthropic] {
            for (code, message) in [
                (
                    "credential_rejected",
                    "The model company rejected the credential.",
                ),
                (
                    "provider_unreachable",
                    "The workspace could not reach the model company.",
                ),
                (
                    "model_unavailable",
                    "The selected model is not advertised for this credential.",
                ),
            ] {
                let reply = key_reply(vendor).replace(r#""outcome":"complete""#, &format!(r#""error":{{"code":"{code}","stage":"gateway","message":"synthetic-private-output"}},"outcome":"failed""#)).replace(r#""exitCode":0""#, r#""exitCode":1""#);
                for stderr in [message, "synthetic-private-output"] {
                    let out = collected_reply(
                        &key_command(vendor),
                        crate::engine::Collected {
                            stdout: reply.as_bytes().to_vec(),
                            stderr: format!("{stderr}\n").into_bytes(),
                            elapsed_ms: 1,
                            exit_code: Some(1),
                            error: None,
                        },
                    );
                    assert!(!out.success);
                    assert!(
                        out.connection.is_some(),
                        "partial failure retains checked identity"
                    );
                    let diagnostic = serde_json::to_value(out.diagnostic.unwrap()).unwrap();
                    assert_eq!(diagnostic["errorCode"], code);
                    assert_eq!(
                        diagnostic["stderr"],
                        if stderr == message {
                            message
                        } else {
                            "Stderr withheld."
                        }
                    );
                    assert!(!diagnostic.to_string().contains("synthetic-private-output"));
                }
            }
        }
    }
    fn retained_request() -> RequestBody {
        serde_json::from_value(serde_json::json!({"op": "modelDisconnectRetained", "body": {"connection": connection()}})).unwrap()
    }
    fn pristine() -> CredentialState {
        CredentialState {
            profile: CREDENTIAL_PROFILE,
            installed: false,
            generation: None,
        }
    }
    #[test]
    fn retained_cleanup_uses_verified_disconnect_and_replays_ack_without_another_command() {
        let mut m = lifecycle(vec![Outcome {
            connection: Some(connection()),
            success: true,
            diagnostic: None,
        }]);
        assert!(matches!(
            m.handle(pristine(), &RequestBody::ModelStatus {}),
            Some(ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                ..
            }))
        ));
        for _ in 0..2 {
            assert_eq!(
                m.handle(pristine(), &retained_request()),
                Some(ResultBody::Ready {})
            );
        }
        assert_eq!(
            m.runner.calls,
            vec![ModelCommand::Disconnect {
                connection: connection()
            }]
        );
        assert!(m.ready_connection().is_none());
    }
    #[test]
    fn retained_cleanup_then_connect_uses_the_new_generation() {
        let old = connection();
        let mut next = old.clone();
        next.generation = id(7);
        next.id = id(8);
        let mut m = lifecycle(vec![
            Outcome {
                connection: Some(old.clone()),
                success: true,
                diagnostic: None,
            },
            Outcome {
                connection: Some(next.clone()),
                success: true,
                diagnostic: None,
            },
        ]);
        assert_eq!(
            m.handle(pristine(), &retained_request()),
            Some(ResultBody::Ready {})
        );
        assert!(m.allows_credential_mutation());
        let result = m.handle(
            CredentialState {
                generation: Some(id(7)),
                ..credential()
            },
            &RequestBody::ModelConnect {
                generation: id(7),
                selection: selection(),
            },
        );
        assert!(matches!(
            result,
            Some(ResultBody::ModelState(ModelState {
                phase: ModelPhase::Ready,
                ..
            }))
        ));
        assert_eq!(m.ready_connection(), Some(&next));
        assert_eq!(
            m.runner.calls,
            vec![
                ModelCommand::Disconnect { connection: old },
                ModelCommand::Connect {
                    generation: id(7),
                    selection: selection()
                }
            ]
        );
    }
    #[test]
    fn retained_cleanup_refuses_current_boot_credentials_or_a_different_identity() {
        let mut m = lifecycle(vec![Outcome {
            connection: Some(connection()),
            success: true,
            diagnostic: None,
        }]);
        for current in [
            credential(),
            CredentialState {
                installed: false,
                ..credential()
            },
        ] {
            assert_eq!(
                m.handle(current, &retained_request()),
                Some(ResultBody::Error {
                    code: ErrorCode::Unavailable
                })
            );
        }
        assert!(m.runner.calls.is_empty());
        assert_eq!(
            m.handle(pristine(), &retained_request()),
            Some(ResultBody::Ready {})
        );
        let mut other = connection();
        other.id = id(8);
        let request = serde_json::from_value(
            serde_json::json!({"op": "modelDisconnectRetained", "body": {"connection": other}}),
        )
        .unwrap();
        assert_eq!(
            m.handle(pristine(), &request),
            Some(ResultBody::Error {
                code: ErrorCode::Unavailable
            })
        );
        assert_eq!(m.runner.calls.len(), 1);
    }
    #[test]
    fn failed_retained_cleanup_is_not_acknowledged_and_can_only_retry_same_identity() {
        let mut m = lifecycle(vec![
            Outcome {
                connection: Some(connection()),
                success: false,
                diagnostic: None,
            },
            Outcome {
                connection: Some(connection()),
                success: true,
                diagnostic: None,
            },
        ]);
        assert_eq!(
            m.handle(pristine(), &retained_request()),
            Some(ResultBody::Error {
                code: ErrorCode::Unavailable
            })
        );
        assert!(!m.allows_credential_mutation());
        assert_eq!(
            m.handle(pristine(), &retained_request()),
            Some(ResultBody::Ready {})
        );
        assert!(m.allows_credential_mutation());
        assert_eq!(m.runner.calls.len(), 2);
    }
    #[test]
    fn wrong_generation_absent_credentials_and_invalid_selection_never_run() {
        let cases = [
            (
                credential(),
                RequestBody::ModelConnect {
                    generation: id(3),
                    selection: selection(),
                },
            ),
            (
                CredentialState {
                    installed: false,
                    ..credential()
                },
                connect(),
            ),
            (
                credential(),
                RequestBody::ModelConnect {
                    generation: id(1),
                    selection: ModelSelection {
                        vendor: None,
                        provider: "".into(),
                        model: "x".into(),
                    },
                },
            ),
            (
                credential(),
                RequestBody::ModelConnect {
                    generation: id(1),
                    selection: ModelSelection {
                        vendor: None,
                        provider: "x".into(),
                        model: "x\0".into(),
                    },
                },
            ),
            (
                credential(),
                RequestBody::ModelConnect {
                    generation: id(1),
                    selection: ModelSelection {
                        vendor: None,
                        provider: "x".into(),
                        model: "x".repeat(4097),
                    },
                },
            ),
        ];
        for (credential, request) in cases {
            let mut m = lifecycle(vec![]);
            let ResultBody::ModelState(s) = m.handle(credential.clone(), &request).unwrap() else {
                panic!()
            };
            assert_eq!(s.phase, ModelPhase::Idle);
            assert_eq!(s.credential, credential);
            assert!(m.runner.calls.is_empty());
        }
    }
    #[test]
    fn explicit_same_attempt_retry_can_reconcile_unknown_without_a_connection() {
        let mut m = lifecycle(vec![
            Outcome {
                connection: None,
                success: false,
                diagnostic: None,
            },
            parse_reply(&command(), 0, READY.as_bytes()),
        ]);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        assert_eq!(
            state(&mut m, &RequestBody::ModelStatus {}).phase,
            ModelPhase::Unknown
        );
        assert_eq!(m.runner.calls.len(), 1);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Ready);
        assert_eq!(m.runner.calls.len(), 2);
    }
    const REFUSED: &str = r#"{"schemaVersion":1,"kind":"kmx.event","operation":"cluster.orka.model.connect","operationId":"00000000-0000-4000-8000-000000000004","seq":1,"elapsedMs":0,"event":"stage.started","stage":"gateway"}
{"schemaVersion":1,"kind":"kmx.result","operation":"cluster.orka.model.connect","operationId":"00000000-0000-4000-8000-000000000004","seq":2,"elapsedMs":2,"outcome":"failed","verdict":"not_applicable","exitCode":1,"error":{"code":"execution_unknown","stage":"preflight","message":"The execution outcome is unknown.","recovery":"inspect_execution"},"mode":"apply","effects":[],"remoteState":"unknown"}
"#;
    fn failed_state(outcome: Outcome) -> serde_json::Value {
        serde_json::to_value(state(&mut lifecycle(vec![outcome]), &connect())).unwrap()
    }
    #[test]
    fn completed_parent_with_open_pipe_retains_validated_stage_on_timeout() {
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
            time::Duration,
        };
        let child = Command::new("/bin/sh")
            .args(["-c", "printf '%s' \"$EVENT\"; sleep 60 & exit 0"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("EVENT", READY)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let collected = crate::engine::collect_diagnostic(child, Duration::from_millis(100));
        assert_eq!(
            collected.error.as_ref().unwrap().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert_eq!(collected.exit_code, Some(0));
        let state = failed_state(collected_reply(&command(), collected));
        assert_eq!(state["phase"], "unknown");
        assert_eq!(state["diagnostic"]["classification"], "timeout");
        assert_eq!(state["diagnostic"]["stage"], "gateway");
    }

    #[test]
    fn timeout_retains_partial_stage_and_withholds_stderr_without_stripping_prefixes() {
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
            time::Duration,
        };
        let event = READY.lines().next().unwrap();
        let child = Command::new("/bin/sh")
            .args(["-c", "printf '%s\\n' \"$EVENT\"; printf '%s\\n' 'Error: synthetic-private-output' >&2; sleep 60"])
            .env_clear().env("PATH", "/usr/bin:/bin").env("EVENT", event)
            .stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0).spawn().unwrap();
        let collected = crate::engine::collect_diagnostic(child, Duration::from_millis(100));
        assert_eq!(
            collected.error.as_ref().unwrap().kind(),
            std::io::ErrorKind::TimedOut
        );
        let s = failed_state(collected_reply(&command(), collected));
        assert_eq!(s["diagnostic"]["classification"], "timeout");
        assert_eq!(s["diagnostic"]["stage"], "gateway");
        assert_eq!(s["diagnostic"]["stderr"], "Stderr withheld.");
        assert!(s["diagnostic"]["elapsedMs"].as_u64().unwrap() >= 100);
        assert!(!s.to_string().contains("synthetic-private-output"));
    }
    #[test]
    fn stderr_allows_only_whole_fixed_messages_and_success_clears_diagnostics() {
        for (stderr, expected) in [
            (
                b"The execution outcome is unknown.\n".as_slice(),
                Some("The execution outcome is unknown."),
            ),
            (
                b"Error: The execution outcome is unknown.\n".as_slice(),
                Some("Stderr withheld."),
            ),
            (
                b"The execution outcome is unknown.\nsynthetic-private-output",
                Some("Stderr withheld."),
            ),
            (
                b"\x1b[31mThe execution outcome is unknown.",
                Some("Stderr withheld."),
            ),
            (b"\xff", Some("Stderr withheld.")),
            (b"", None),
        ] {
            let out = collected_reply(
                &command(),
                crate::engine::Collected {
                    stdout: REFUSED.as_bytes().to_vec(),
                    stderr: stderr.to_vec(),
                    elapsed_ms: 12,
                    exit_code: Some(1),
                    error: None,
                },
            );
            let s = failed_state(out);
            assert_eq!(s["diagnostic"]["stderr"].as_str(), expected);
            assert!(!s.to_string().contains("synthetic-private-output"));
        }
        let mut m = lifecycle(vec![
            parse_reply(&command(), 1, REFUSED.as_bytes()),
            parse_reply(&command(), 0, READY.as_bytes()),
        ]);
        assert!(state(&mut m, &connect()).diagnostic.is_some());
        assert!(state(&mut m, &connect()).diagnostic.is_none());
    }
    #[test]
    fn malformed_tail_and_output_limit_do_not_erase_prior_validated_stage() {
        let event = READY.lines().next().unwrap();
        let mut bytes = format!("{event}\n").into_bytes();
        bytes.push(0xff);
        let s = failed_state(parse_reply(&command(), 0, &bytes));
        assert_eq!(s["diagnostic"]["stage"], "gateway");
        use std::{
            os::unix::process::CommandExt,
            process::{Command, Stdio},
            time::Duration,
        };
        let child = Command::new("/bin/sh")
            .args(["-c", "printf '%s\\n' \"$EVENT\"; head -c 1048577 /dev/zero"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("EVENT", event)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let collected = crate::engine::collect_diagnostic(child, Duration::from_secs(3));
        assert_eq!(collected.stdout.len(), MAX_FILE + 1);
        assert_eq!(
            collected.error.as_ref().unwrap().kind(),
            std::io::ErrorKind::FileTooLarge
        );
        let s = failed_state(collected_reply(&command(), collected));
        assert_eq!(s["diagnostic"]["classification"], "outputLimit");
        assert_eq!(s["diagnostic"]["stage"], "gateway");
    }
    #[test]
    fn acknowledged_disconnect_unlocks_a_new_generation_and_selection() {
        let stopped = READY
            .replace(
                "cluster.orka.model.connect",
                "cluster.orka.model.disconnect",
            )
            .replace("model_ready", "gateway_stopped");
        let next_selection = ModelSelection {
            model: "other".into(),
            ..selection()
        };
        let next_command = ModelCommand::Connect {
            generation: id(3),
            selection: next_selection.clone(),
        };
        let next_reply = READY
            .replace("gpt-4.1", "other")
            .replace("000000000001", "000000000003");
        let mut m = lifecycle(vec![
            parse_reply(&command(), 1, READY.as_bytes()),
            parse_reply(
                &ModelCommand::Disconnect {
                    connection: connection(),
                },
                0,
                stopped.as_bytes(),
            ),
            parse_reply(&next_command, 0, next_reply.as_bytes()),
        ]);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        assert_eq!(
            m.runner.calls.len(),
            1,
            "known connection requires explicit disconnect"
        );
        assert_eq!(
            state(&mut m, &RequestBody::ModelDisconnect { connection: id(2) }).phase,
            ModelPhase::Idle
        );
        let reply = m
            .handle(
                CredentialState {
                    generation: Some(id(3)),
                    ..credential()
                },
                &RequestBody::ModelConnect {
                    generation: id(3),
                    selection: next_selection,
                },
            )
            .unwrap();
        assert!(matches!(
            reply,
            ResultBody::ModelState(ModelState {
                phase: ModelPhase::Ready,
                ..
            })
        ));
        assert_eq!(m.runner.calls[2], next_command);
    }
    #[test]
    fn parser_retains_validated_failure_stage_and_safe_error_code() {
        let s = failed_state(parse_reply(&command(), 1, REFUSED.as_bytes()));
        assert_eq!(s["diagnostic"]["classification"], "exit");
        assert_eq!(s["diagnostic"]["stage"], "preflight");
        assert_eq!(s["diagnostic"]["errorCode"], "execution_unknown");
        assert_eq!(s["diagnostic"]["exitCode"], 1);
    }
    #[test]
    fn malformed_reply_retains_only_prior_validated_stage_and_connection() {
        let s = failed_state(parse_reply(&command(), 0, format!("{READY}{{").as_bytes()));
        assert_eq!(s["diagnostic"]["classification"], "invalidReply");
        assert_eq!(s["diagnostic"]["stage"], "gateway");
        assert_eq!(s["connection"]["id"], id(2).to_string());
        for reply in [
            READY.replace("cluster.orka.model.connect", "wrong"),
            READY.replace(r#""seq":1"#, r#""seq":3"#),
            READY.replace("000000000004", "not-a-uuid"),
        ] {
            let s = failed_state(parse_reply(&command(), 0, reply.as_bytes()));
            assert!(s["diagnostic"]["stage"].is_null());
        }
        let unsafe_reply = REFUSED
            .replace("gateway", "synthetic-private-output")
            .replace("preflight", "synthetic-private-output")
            .replace("execution_unknown", "synthetic-private-output")
            .replace(
                "The execution outcome is unknown.",
                "synthetic-private-output",
            );
        let s = failed_state(parse_reply(&command(), 1, unsafe_reply.as_bytes()));
        assert!(!s.to_string().contains("synthetic-private-output"));
    }
    #[test]
    fn explicit_retry_refusal_stays_unknown_and_rejects_mismatched_attempts() {
        let mut m = lifecycle(vec![
            parse_reply(&command(), 1, REFUSED.as_bytes()),
            parse_reply(&command(), 1, REFUSED.as_bytes()),
        ]);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        for selection in [
            ModelSelection {
                model: "other".into(),
                ..selection()
            },
            ModelSelection {
                vendor: None,
                provider: "other".into(),
                ..selection()
            },
        ] {
            state(
                &mut m,
                &RequestBody::ModelConnect {
                    generation: id(1),
                    selection,
                },
            );
        }
        let RequestBody::ModelConnect { selection, .. } = connect() else {
            unreachable!()
        };
        let reply = m
            .handle(
                CredentialState {
                    generation: Some(id(3)),
                    ..credential()
                },
                &RequestBody::ModelConnect {
                    generation: id(3),
                    selection,
                },
            )
            .unwrap();
        assert!(matches!(
            reply,
            ResultBody::ModelState(ModelState {
                phase: ModelPhase::Unknown,
                ..
            })
        ));
        m.handle(
            CredentialState {
                installed: false,
                ..credential()
            },
            &connect(),
        );
        state(&mut m, &RequestBody::ModelStatus {});
        assert_eq!(m.runner.calls.len(), 1);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        assert_eq!(m.runner.calls.len(), 2);
        assert_eq!(m.runner.calls[0], m.runner.calls[1]);
        assert!(!m.allows_credential_mutation());
    }
    #[test]
    fn unknown_without_an_id_cannot_guess_cleanup() {
        let mut m = lifecycle(vec![Outcome {
            connection: None,
            success: false,
            diagnostic: None,
        }]);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        state(&mut m, &RequestBody::ModelDisconnect { connection: id(2) });
        assert!(!m.allows_credential_mutation());
        assert_eq!(m.runner.calls.len(), 1);
    }
    #[test]
    fn failed_exit_keeps_the_validated_id_without_becoming_ready() {
        for (outcome, exit) in [("failed", 1), ("cancelled", 1), ("complete", -1)] {
            let reply = READY
                .replace(
                    r#""outcome":"complete""#,
                    &format!(r#""outcome":"{outcome}""#),
                )
                .replace(r#""exitCode":0"#, r#""exitCode":1"#)
                .replace("model_ready", "unknown");
            let parsed = parse_reply(&command(), exit, reply.as_bytes());
            assert!(!parsed.success);
            assert_eq!(parsed.connection, Some(connection()));
            let mut m = lifecycle(vec![parsed]);
            assert_eq!(state(&mut m, &connect()).connection, Some(connection()));
            assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
            assert!(!m.allows_credential_mutation());
            assert_eq!(m.runner.calls.len(), 1);
        }
    }
    #[test]
    fn malformed_or_cross_selection_replies_never_become_ready() {
        let replies = vec![
            String::new(),
            "not json".into(),
            READY.lines().next().unwrap().into(),
            format!("{READY}{{"),
            format!("{READY}{READY}"),
            READY.replace(r#""schemaVersion":1"#, r#""schemaVersion":2"#),
            READY.replace(
                "cluster.orka.model.connect",
                "cluster.orka.model.disconnect",
            ),
            READY.replace(r#""seq":2"#, r#""seq":3"#),
            READY.replacen("000000000004", "000000000005", 1),
            READY.replace("0000-4000-8000", "0000-1000-8000"),
            READY.replace(r#""kind":"kmx.result""#, r#""kind":"other""#),
            READY.replace(r#""route":"copilot""#, r#""route":"other""#),
            READY.replace(r#""provider":"copilot""#, r#""provider":"other""#),
            READY.replace("gpt-4.1", "other"),
            READY.replace("000000000001", "000000000003"),
            READY.replace("model_ready", "gateway_stopped"),
            READY.replace(r#""exitCode":0"#, r#""exitCode":1"#),
            READY.replace(r#""outcome":"complete""#, r#""outcome":"failed""#),
        ];
        for reply in replies {
            let parsed = parse_reply(&command(), 0, reply.as_bytes());
            assert!(!parsed.success);
            let mut m = lifecycle(vec![parsed]);
            assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
            state(&mut m, &RequestBody::ModelStatus {});
            assert_eq!(m.runner.calls.len(), 1);
        }
        // A valid observed ID survives a later malformed row, but never grants readiness.
        let trailing = parse_reply(&command(), 0, format!("{READY}{{").as_bytes());
        assert_eq!(trailing.connection, Some(connection()));
        assert!(!trailing.success);
        let oversized = vec![b' '; MAX_FILE + 1];
        assert!(!parse_reply(&command(), 0, &oversized).success);
        assert!(!parse_reply(&command(), 0, &[0xff]).success);
    }
    #[test]
    fn production_command_targets_the_installation_cluster_and_restricted_guest_env() {
        let installation = Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap();
        let kmx = Kmx::new(installation);
        for action in [
            command(),
            ModelCommand::Disconnect {
                connection: connection(),
            },
        ] {
            let command = kmx.command(&action);
            assert_eq!(command.get_program(), "/usr/local/bin/kmx");
            assert_eq!(command.get_current_dir(), Some(std::path::Path::new("/")));
            assert_eq!(
                command.get_args().next().unwrap(),
                "--context=kind-bridge-1234567812344abc8def123456789abc"
            );
            let env: std::collections::BTreeMap<_, _> = command
                .get_envs()
                .map(|(key, value)| (key.to_str().unwrap(), value.unwrap().to_str().unwrap()))
                .collect();
            assert_eq!(
                env,
                std::collections::BTreeMap::from([
                    ("PATH", "/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"),
                    ("HOME", "/root"),
                    ("LANG", "C.UTF-8"),
                    ("LC_ALL", "C"),
                    ("KUBECONFIG", "/var/lib/bridge/kubeconfig"),
                    ("KMX_HOME", "/var/lib/bridge/kmx"),
                    ("KIND_CLUSTER", "bridge-1234567812344abc8def123456789abc"),
                    ("CONTAINER_ENGINE", "docker"),
                    ("KIND_EXPERIMENTAL_PROVIDER", "docker"),
                    ("DOCKER_HOST", "unix:///var/run/docker.sock"),
                ])
            );
        }
    }
    #[test]
    fn argv_is_fixed_and_values_are_single_arguments_not_options() {
        let mut selection = selection();
        selection.model = "--odd-model/with:punctuation;id".into();
        assert!(selection.valid());
        let kmx = Kmx::new(Uuid::parse_str("12345678-1234-4abc-8def-123456789abc").unwrap());
        let cmd = kmx.command(&ModelCommand::Connect {
            generation: id(1),
            selection,
        });
        let args: Vec<_> = cmd.get_args().map(|s| s.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--context=kind-bridge-1234567812344abc8def123456789abc",
                "--container-engine=docker",
                "cluster",
                "orka",
                "model",
                "connect",
                "--namespace=orka-system",
                "--route=copilot",
                "--provider=copilot",
                "--model=--odd-model/with:punctuation;id",
                "--credential-generation=00000000-0000-4000-8000-000000000001",
                "-o=jsonl",
            ]
        );
        assert_eq!(
            kmx.command(&ModelCommand::Disconnect {
                connection: connection()
            })
            .get_args()
            .map(|s| s.to_str().unwrap())
            .collect::<Vec<_>>(),
            [
                "--context=kind-bridge-1234567812344abc8def123456789abc",
                "--container-engine=docker",
                "cluster",
                "orka",
                "model",
                "disconnect",
                "--namespace=orka-system",
                "--provider=copilot",
                "--connection=00000000-0000-4000-8000-000000000002",
                "-o=jsonl",
            ]
        );
    }
    #[test]
    fn ready_disconnect_keeps_credentials_installed_and_idle_disconnect_is_a_noop() {
        let mut m = lifecycle(vec![
            Outcome {
                connection: Some(connection()),
                success: true,
                diagnostic: None,
            },
            Outcome {
                connection: Some(connection()),
                success: true,
                diagnostic: None,
            },
        ]);
        state(&mut m, &RequestBody::ModelDisconnect { connection: id(2) });
        assert!(m.runner.calls.is_empty());
        state(&mut m, &connect());
        let s = state(&mut m, &RequestBody::ModelDisconnect { connection: id(2) });
        assert_eq!(s.phase, ModelPhase::Idle);
        assert!(s.connection.is_none());
        assert_eq!(s.credential, credential());
        assert!(m.allows_credential_mutation());
        assert_eq!(m.runner.calls.len(), 2);
        assert!(m
            .handle(credential(), &RequestBody::CredentialStatus {})
            .is_none());
    }
    #[test]
    fn disconnect_requires_the_actual_stopped_id() {
        let stopped = READY
            .replace(
                "cluster.orka.model.connect",
                "cluster.orka.model.disconnect",
            )
            .replace("model_ready", "gateway_stopped");
        let command = ModelCommand::Disconnect {
            connection: connection(),
        };
        assert!(parse_reply(&command, 0, stopped.as_bytes()).success);
        let wrong_id = stopped.replace("000000000002", "000000000003");
        assert!(!parse_reply(&command, 0, wrong_id.as_bytes()).success);
        assert!(!parse_reply(&command, 0, READY.as_bytes()).success);
    }
    #[test]
    fn actual_flat_jsonl_projection_keeps_only_native_connection() {
        let out = parse_reply(&command(), 0, READY.as_bytes());
        assert!(out.success);
        assert_eq!(out.connection, Some(connection()));
    }
    #[test]
    fn only_a_matching_disconnect_unlocks_credentials_after_failed_connect() {
        let mut m = lifecycle(vec![
            Outcome {
                connection: Some(connection()),
                success: false,
                diagnostic: None,
            },
            Outcome {
                connection: Some(connection()),
                success: false,
                diagnostic: None,
            },
            Outcome {
                connection: Some(connection()),
                success: true,
                diagnostic: None,
            },
        ]);
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Unknown);
        assert_eq!(state(&mut m, &connect()).connection, Some(connection()));
        assert!(!m.allows_credential_mutation());
        state(&mut m, &RequestBody::ModelDisconnect { connection: id(3) });
        assert_eq!(m.runner.calls.len(), 1);
        assert_eq!(
            state(&mut m, &RequestBody::ModelDisconnect { connection: id(2) }).phase,
            ModelPhase::Unknown
        );
        assert_eq!(m.runner.calls.len(), 2);
        assert!(!m.allows_credential_mutation());
        assert_eq!(
            state(&mut m, &RequestBody::ModelDisconnect { connection: id(2) }).phase,
            ModelPhase::Idle
        );
        assert!(m.allows_credential_mutation());
        assert_eq!(
            m.runner.calls[1],
            ModelCommand::Disconnect {
                connection: connection()
            }
        );
        assert!(state(&mut m, &RequestBody::ModelStatus {})
            .connection
            .is_none());
    }
    #[test]
    fn connects_once_and_status_never_runs_a_command() {
        let mut m = lifecycle(vec![Outcome {
            connection: Some(connection()),
            success: true,
            diagnostic: None,
        }]);
        assert_eq!(
            state(&mut m, &RequestBody::ModelStatus {}).phase,
            ModelPhase::Idle
        );
        assert!(m.allows_credential_mutation());
        assert_eq!(state(&mut m, &connect()).phase, ModelPhase::Ready);
        assert_eq!(state(&mut m, &connect()).connection, Some(connection()));
        assert_eq!(
            state(&mut m, &RequestBody::ModelStatus {}).phase,
            ModelPhase::Ready
        );
        assert!(!m.allows_credential_mutation());
        assert_eq!(
            m.runner.calls,
            vec![ModelCommand::Connect {
                generation: id(1),
                selection: selection()
            }]
        );
    }
}
