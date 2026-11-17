use bridge_engine::{
    agent_folder::AgentFolder,
    engine::run::{ParsedAgent, ParsedPair},
    example_run::ExampleRun,
    local_vm::LocalVm,
    runtime::client::VmClient,
};
use bridge_runtime_protocol::*;
use serde_json::{json, Value};
use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};

const AGENT: &str = "---\nname: reader\ndescription: Read pages\nmodel: fake-model\nkmx:\n  schemaVersion: 1\n  runtime: orka\n  tools:\n    - kind: builtin\n      name: web_fetch\n  soul:\n    inline: Be kind\n---\nRead the page.\n";
const TESTS: &str = "---\nschemaVersion: 1\n---\n```yaml agent-test\nid: fetch\ninput: Read example.org\nassertions:\n  - type: contains\n    id: answer\n    value: example\n```\n";

#[test]
fn agent_only_mapping_preserves_run_prompt_tools_soul_and_provider() {
    let agent = ParsedAgent::parse(AGENT.as_bytes()).unwrap();
    assert_eq!(
        agent.spec("fake-provider"),
        json!({
            "providerRef":{"name":"fake-provider"}, "model":{"name":"fake-model"},
            "systemPrompt":{"inline":"Read the page.\n"}, "tools":[{"name":"web_fetch"}],
            "soul":{"inline":"Be kind"}
        })
    );
    let pair = ParsedPair::parse(AGENT.as_bytes(), TESTS.as_bytes()).unwrap();
    assert_eq!(pair.prompt(), agent.prompt());
    assert_eq!(pair.model(), agent.model());
    assert_eq!(pair.tools(), agent.tools());
    assert_eq!(pair.name(), Some("reader"));
    assert!(ParsedPair::parse(AGENT.as_bytes(), b"not test cases").is_err());
    assert_eq!(pair.cases().len(), 1);
}

#[test]
fn agent_only_parser_keeps_existing_validation() {
    for invalid in [
        AGENT.replace("schemaVersion: 1", "schemaVersion: 2"),
        AGENT.replace("runtime: orka", "runtime: other"),
        AGENT.replace("Be kind", ""),
        AGENT.replace("Read the page.\n", ""),
        AGENT.replace("name: web_fetch", "name: ''"),
        AGENT.replace(
            "  soul:",
            "    - kind: builtin\n      name: web_fetch\n  soul:",
        ),
    ] {
        assert!(ParsedAgent::parse(invalid.as_bytes()).is_err());
    }
}

#[test]
fn saved_agent_capture_does_not_require_valid_test_cases() {
    let home = tempfile::tempdir().unwrap();
    let folder = AgentFolder::new(Some(home.path().into()));
    let opened = folder.open().unwrap();
    let root = std::path::Path::new(opened.path.as_ref().unwrap())
        .parent()
        .unwrap();
    std::fs::write(
        root.join("web-reader.agent.test.md"),
        "not executable tests",
    )
    .unwrap();
    folder.save(AGENT.into()).unwrap();
    assert_eq!(folder.capture_agent().unwrap(), AGENT.as_bytes());
    assert_eq!(
        std::fs::read_to_string(root.join("web-reader.agent.test.md")).unwrap(),
        "not executable tests"
    );
    std::fs::write(root.join("web-reader.agent.md"), vec![b'x'; MAX_FILE + 1]).unwrap();
    assert!(folder.capture_agent().is_err());
}

#[tokio::test]
async fn held_chat_admission_blocks_run_without_replacing_prior_run_state() {
    let vm = LocalVm::unavailable();
    let home = tempfile::tempdir().unwrap();
    let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
    let run = ExampleRun::new(tokio::runtime::Handle::current(), true, folder);
    let lease = vm.try_authoring().unwrap();
    assert!(vm.try_authoring().is_none());
    let refused = run.start(vm.clone());
    assert_eq!(refused.state, bridge_engine::example_run::RunState::Idle);
    assert!(!refused.can_run);
    assert!(!run.is_active());
    drop(lease);
    assert!(vm.try_authoring().is_some());
}

struct Wire {
    pending: io::Cursor<Vec<u8>>,
    requests: Arc<Mutex<Vec<EngineActionWire>>>,
    status: u16,
    unsupported: bool,
    output: Vec<u8>,
}
impl Read for Wire {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.pending.read(b)
    }
}
impl Write for Wire {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Frame::Request(request) = serde_json::from_slice::<Frame>(bytes).unwrap() else {
            panic!("expected request")
        };
        let body = match request.body {
            RequestBody::Execute {
                operation, action, ..
            } => {
                self.requests.lock().unwrap().push(action);
                if self.unsupported {
                    ResultBody::Error {
                        code: ErrorCode::Unavailable,
                    }
                } else {
                    self.output = serde_json::to_vec(&OrkaReply {
                        status: self.status,
                        body: Value::Null,
                    })
                    .unwrap();
                    ResultBody::Operation {
                        operation,
                        info: OperationInfo {
                            state: OperationState::Completed,
                            exit: Some(0),
                            bytes: self.output.len(),
                            has_diagnostics: false,
                        },
                    }
                }
            }
            RequestBody::Status { operation } => ResultBody::Operation {
                operation,
                info: OperationInfo {
                    state: OperationState::Completed,
                    exit: Some(0),
                    bytes: self.output.len(),
                    has_diagnostics: false,
                },
            },
            RequestBody::ReadResult { offset, .. } => ResultBody::Chunk {
                data: encode_chunk(&self.output[offset..]),
            },
            _ => panic!("unexpected request"),
        };
        self.pending = io::Cursor::new(
            encode(&Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: request.boot_nonce,
                request_id: request.request_id,
                body,
            }))
            .unwrap(),
        );
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn chat_wire_preserves_delete_and_missing_status_without_run_fallback() {
    for (status, unsupported) in [(204, false), (404, false), (200, false), (204, true)] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let mut client = VmClient::established(
            Box::new(Wire {
                pending: io::Cursor::new(vec![]),
                requests: requests.clone(),
                status,
                unsupported,
                output: vec![],
            }),
            Uuid::new_v4(),
        )
        .unwrap();
        let result = client.chat_request(ChatRequest {
            method: ChatMethod::Delete,
            path: format!("tasks/bridge-chat-task-{}", Uuid::new_v4().simple()),
            after: None,
            body: None,
        });
        if unsupported {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap().status, status);
        }
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert!(matches!(
            requests.lock().unwrap()[0],
            EngineActionWire::OrkaChat { .. }
        ));
        assert!(client
            .chat_request(ChatRequest {
                method: ChatMethod::Delete,
                path: "sessions/arbitrary".into(),
                after: None,
                body: None
            })
            .is_err());
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
