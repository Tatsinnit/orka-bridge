use bridge_engine::{
    agent_folder::AgentFolder,
    local_vm::{LocalFactory, LocalSession, LocalVm, LocalVmStatus, StartFuture},
    runtime::{client::VmClient, types::RuntimeError},
};
use bridge_runtime_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub struct Guest {
    pub requests: Vec<ChatRequest>,
    pub objects: HashMap<String, Value>,
    pub phase: String,
    pub omit_phase: bool,
    pub release: bool,
    pub transcript: Option<String>,
    pub missing_session: bool,
    pub lose_create: Option<&'static str>,
    pub vanish_task: bool,
    pub reject_create: bool,
    pub old_guest: bool,
    pub broken_wire: bool,
    pub delete_status: u16,
    pub delete_applies: bool,
    pub wrong_events: bool,
    pub bad_cursor: bool,
    pub event_total: u64,
    pub fail_events: bool,
    pub fail_result: bool,
    pub no_result: bool,
    pub model_ready: bool,
    pub provider: String,
    pub boot: Uuid,
    pub session_reads: usize,
    pub result_reads: usize,
}
impl Guest {
    fn new() -> Self {
        Self {
            requests: vec![],
            objects: HashMap::new(),
            phase: "Succeeded".into(),
            omit_phase: false,
            release: true,
            transcript: None,
            missing_session: false,
            lose_create: None,
            vanish_task: false,
            reject_create: false,
            old_guest: false,
            broken_wire: false,
            delete_status: 204,
            delete_applies: true,
            wrong_events: false,
            bad_cursor: false,
            event_total: 2,
            fail_events: false,
            fail_result: false,
            no_result: false,
            model_ready: true,
            provider: "copilot".into(),
            boot: Uuid::new_v4(),
            session_reads: 0,
            result_reads: 0,
        }
    }
    pub fn posts(&self, path: &str) -> Vec<Value> {
        self.requests
            .iter()
            .filter(|r| r.method == ChatMethod::Post && r.path == path)
            .map(|r| r.body.clone().unwrap())
            .collect()
    }
    pub fn deletes(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| r.method == ChatMethod::Delete)
            .count()
    }
    fn reply(&mut self, r: ChatRequest) -> (u16, Value) {
        r.validate().unwrap();
        self.requests.push(r.clone());
        if r.method == ChatMethod::Post {
            if self.reject_create {
                return (400, json!({"error":"rejected"}));
            }
            let b = r.body.unwrap();
            let name = b["name"].as_str().unwrap();
            let mut object = json!({"apiVersion":"core.orka.ai/v1alpha1", "kind":if r.path=="agents" {"Agent"} else {"Task"},
                "metadata":{"name":name,"namespace":"orka-system","uid":format!("uid-{name}"),"generation":1},
                "spec":b["spec"],"status":{"phase":self.phase}});
            if r.path == "tasks" {
                object["spec"]["timeout"] = json!("2m0s");
                object["spec"]["retryPolicy"]
                    .as_object_mut()
                    .unwrap()
                    .remove("maxRetries");
                let session = b["spec"]["sessionRef"]["name"].as_str().unwrap();
                self.objects.entry(format!("sessions/{session}")).or_insert_with(||
                    json!({"name":session,"namespace":"orka-system","activeTask":name,"transcript":""}))["activeTask"] = json!(name);
            }
            self.objects
                .insert(format!("{}/{name}", r.path), object.clone());
            if self.lose_create == Some(r.path.as_str()) {
                self.lose_create = None;
                if r.path == "tasks" && self.vanish_task {
                    self.objects.remove(&format!("tasks/{name}"));
                }
                return (500, json!({"error":"lost reply"}));
            }
            return (201, object);
        }
        if r.method == ChatMethod::Delete {
            if self.delete_applies {
                self.objects.remove(&r.path);
            }
            return (self.delete_status, Value::Null);
        }
        if r.path.starts_with("sessions/") {
            self.session_reads += 1;
            if self.missing_session {
                return (404, json!({}));
            }
            let Some(mut s) = self.objects.get(&r.path).cloned() else {
                return (404, json!({}));
            };
            if self.release {
                s["activeTask"] = json!("");
                let mut lines: Vec<String> = vec![];
                for task in self.posts("tasks") {
                    if task["spec"]["sessionRef"]["name"] != s["name"] {
                        continue;
                    }
                    let name = task["name"].as_str().unwrap();
                    if !self.objects.contains_key(&format!("tasks/{name}")) {
                        continue;
                    }
                    let uid = format!("uid-{name}");
                    lines.push(json!({"id":format!("task:{uid}:user"),"role":"user","content":task["spec"]["prompt"]}).to_string());
                    lines.push(json!({"id":format!("task:{uid}:assistant"),"role":"assistant","content":"A real observed answer."}).to_string());
                }
                s["transcript"] = json!(lines.join("\n"));
            }
            if let Some(text) = &self.transcript {
                s["transcript"] = json!(text);
            }
            return (200, s);
        }
        if r.path.ends_with("/events") {
            if self.fail_events {
                return (503, json!({}));
            }
            let name = r.path.split('/').nth(1).unwrap();
            let agent = self.objects[&format!("tasks/{name}")]["spec"]["agentRef"]["name"].clone();
            let after = r.after.unwrap();
            // Deliberately paginated: the latest cursor is ahead of the first page.
            let events = if after < self.event_total {
                vec![
                    json!({"namespace":"orka-system","streamType":"task","streamID":name,
                "seq":if self.bad_cursor {after} else {after+1},"type":if after==0 {"ToolCallStarted"} else {"ToolCallCompleted"},
                "taskName":name,"agentName":agent,"toolName":"web_fetch","toolCallID":"call-1"}),
                ]
            } else {
                vec![]
            };
            return (
                200,
                json!({"namespace":"orka-system","streamType":"task","streamID":if self.wrong_events {"foreign"} else {name},
                "afterSeq":after,"latestSeq":self.event_total,"events":events}),
            );
        }
        if r.path.ends_with("/result") {
            self.result_reads += 1;
            return if self.no_result {
                (404, json!({}))
            } else if self.fail_result {
                (503, json!({}))
            } else {
                (200, json!({"result":"A real observed answer."}))
            };
        }
        match self.objects.get(&r.path) {
            Some(object) => {
                let mut object = object.clone();
                if r.path.starts_with("tasks/") {
                    if self.omit_phase {
                        object["status"] = json!({});
                    } else {
                        object["status"]["phase"] = json!(self.phase);
                    }
                }
                (200, object)
            }
            None => (404, json!({})),
        }
    }
}
struct Channel {
    guest: Arc<Mutex<Guest>>,
    input: io::Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl Write for Channel {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Frame::Request(r) = serde_json::from_slice(bytes)? else {
            panic!("expected native request")
        };
        let mut guest = self.guest.lock().unwrap();
        if guest.broken_wire {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let result = match r.body {
            RequestBody::ModelStatus {} => ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                diagnostic: None,
                phase: if guest.model_ready {
                    ModelPhase::Ready
                } else {
                    ModelPhase::Idle
                },
                credential: CredentialState {
                    profile: CREDENTIAL_PROFILE,
                    generation: Some(r.boot_nonce),
                    installed: true,
                },
                connection: Some(ModelConnection {
                    id: r.boot_nonce,
                    generation: r.boot_nonce,
                    selection: ModelSelection {
                        vendor: None,
                        provider: guest.provider.clone(),
                        model: "gpt-5.6-terra".into(),
                    },
                }),
            }),
            RequestBody::Execute {
                operation,
                action: EngineActionWire::OrkaChat { request },
                snapshot,
            } => {
                assert!(snapshot.is_none());
                if guest.old_guest {
                    ResultBody::Error {
                        code: ErrorCode::Unavailable,
                    }
                } else {
                    let (status, body) = guest.reply(request);
                    self.output = serde_json::to_vec(&json!({"status":status,"body":body}))?;
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
                data: encode_chunk(&self.output[offset..self.output.len().min(offset + MAX_CHUNK)]),
            },
            _ => panic!("Chat must not use a Run action, credential, or host fallback"),
        };
        let mut bytes_out = serde_json::to_vec(&Frame::Result(WireResult {
            version: VERSION,
            boot_nonce: r.boot_nonce,
            request_id: r.request_id,
            body: result,
        }))?;
        bytes_out.push(b'\n');
        self.input = io::Cursor::new(bytes_out);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Read for Channel {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.input.read(b)
    }
}
struct Session(VmClient, Uuid);
impl LocalSession for Session {
    fn with_client(
        &mut self,
        work: &mut dyn FnMut(Uuid, &mut VmClient) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        work(self.1, &mut self.0)
    }
    fn execute(
        &mut self,
        _: bridge_engine::engine::types::EngineRequest,
    ) -> bridge_engine::engine::TransportFuture<'_> {
        Box::pin(async {
            Err(bridge_engine::engine::types::EngineError::new(
                bridge_engine::engine::types::ErrorKind::Unavailable,
            ))
        })
    }
    fn stop(&mut self) -> Result<(), RuntimeError> {
        Ok(())
    }
}
struct Factory(Arc<Mutex<Guest>>, Uuid);
impl LocalFactory for Factory {
    fn launch(
        &self,
        _: watch::Receiver<bool>,
        _: Arc<dyn Fn(bridge_engine::local_vm::LocalProgress) + Send + Sync>,
    ) -> StartFuture<'_> {
        Box::pin(async {
            let nonce = self.0.lock().unwrap().boot;
            let client = VmClient::established(
                Box::new(Channel {
                    guest: self.0.clone(),
                    input: io::Cursor::new(vec![]),
                    output: vec![],
                }),
                nonce,
            )
            .unwrap();
            Ok(Box::new(Session(client, self.1)) as Box<dyn LocalSession>)
        })
    }
}
pub struct Fixture {
    pub vm: Arc<LocalVm>,
    pub guest: Arc<Mutex<Guest>>,
    pub folder: Arc<AgentFolder>,
    pub _home: tempfile::TempDir,
}
impl Fixture {
    pub async fn new() -> Self {
        let guest = Arc::new(Mutex::new(Guest::new()));
        let vm = LocalVm::new(Arc::new(Factory(guest.clone(), Uuid::new_v4())));
        vm.start();
        for _ in 0..500 {
            if vm.status() == LocalVmStatus::Ready {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(vm.status(), LocalVmStatus::Ready);
        let home = tempfile::tempdir().unwrap();
        let folder = Arc::new(AgentFolder::new(Some(home.path().into())));
        folder.open().unwrap();
        Self {
            vm,
            guest,
            folder,
            _home: home,
        }
    }
}
