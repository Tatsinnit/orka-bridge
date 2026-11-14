use crate::early::{invalid, validate};
use bridge_runtime_protocol::*;
use std::io;

/// A new object per observed port reopen: partial frames cannot cross stages.
pub struct Session {
    stage: Stage,
    saved: Option<BootDescriptor>,
    hello: StageHello,
    bytes: Vec<u8>,
    last: u64,
    booted: bool,
    poisoned: bool,
}
impl Session {
    pub fn early() -> Self {
        Self::new(Stage::Early, None)
    }
    pub fn serve(saved: Option<BootDescriptor>) -> io::Result<Self> {
        let b = saved.as_ref().ok_or_else(invalid)?;
        validate(b)?;
        Ok(Self::new(Stage::Serve, saved))
    }
    fn new(stage: Stage, saved: Option<BootDescriptor>) -> Self {
        Self {
            stage,
            hello: StageHello {
                version: VERSION,
                stage,
                guest_nonce: Uuid::new_v4(),
                accepted_boot_nonce: saved.as_ref().map(|b| b.boot_nonce),
            },
            saved,
            bytes: Vec::new(),
            last: 0,
            booted: false,
            poisoned: false,
        }
    }
    pub fn local(saved: BootDescriptor) -> io::Result<Self> {
        let mut s = Self::serve(Some(saved))?;
        s.booted = true;
        s.last = 2;
        Ok(s)
    }
    pub fn hello(&self) -> &StageHello {
        &self.hello
    }
    pub fn push(&mut self, byte: u8) -> io::Result<Option<WireRequest>> {
        if self.poisoned {
            return Err(invalid());
        }
        let result = self.push_inner(byte);
        if let Err(e) = &result {
            crate::diagnostics::note(
                crate::diagnostics::Context::new(self.stage_label(), "frame-validation"),
                e,
            );
            self.poisoned = true;
            self.bytes.clear();
        }
        result
    }
    fn stage_label(&self) -> &'static str {
        match self.stage {
            Stage::Early => "early",
            Stage::Serve => "serve",
        }
    }
    fn push_inner(&mut self, byte: u8) -> io::Result<Option<WireRequest>> {
        if byte != b'\n' {
            if self.bytes.len() >= MAX_FRAME - 1 {
                return Err(invalid());
            }
            self.bytes.push(byte);
            return Ok(None);
        }
        let frame = serde_json::from_slice::<Frame>(&self.bytes).map_err(|_| invalid())?;
        self.bytes.clear();
        self.accept(frame).map(Some)
    }
    pub fn accept(&mut self, frame: Frame) -> io::Result<WireRequest> {
        frame.validate().map_err(|_| invalid())?;
        let Frame::Request(r) = frame else {
            return Err(invalid());
        };
        if r.request_id <= self.last {
            return Err(invalid());
        }
        if !self.booted {
            if r.request_id != if self.stage == Stage::Early { 1 } else { 2 } {
                return Err(invalid());
            }
            let RequestBody::Boot(b) = &r.body else {
                return Err(invalid());
            };
            if let Some(old) = &self.saved {
                if b.state_creation != StateCreation::Existing
                    || b.installation_id != old.installation_id
                    || b.generation != old.generation
                    || b.boot_nonce != old.boot_nonce
                    || b.state_fs_uuid != old.state_fs_uuid
                    || b.state_bytes != old.state_bytes
                {
                    return Err(invalid());
                }
            }
            self.saved = Some(b.clone());
            self.booted = true;
        } else {
            if self.stage != Stage::Serve
                || r.boot_nonce != self.saved.as_ref().ok_or_else(invalid)?.boot_nonce
                || matches!(r.body, RequestBody::Boot(_))
            {
                return Err(invalid());
            }
        }
        self.last = r.request_id;
        Ok(r)
    }
}
pub fn handle(
    system: &mut impl crate::early::BootSystem,
    b: &BootDescriptor,
    r: &WireRequest,
) -> (ResultBody, bool) {
    match &r.body {
        RequestBody::Readiness {} => {
            match system.ready(b).and_then(|()| system.infrastructure(b)) {
                Ok(state) => (ResultBody::Infrastructure(state), false),
                Err(_) => (
                    ResultBody::Error {
                        code: ErrorCode::IncompatibleState,
                    },
                    false,
                ),
            }
        }
        RequestBody::Shutdown {} => match system.shutdown() {
            Ok(()) => (ResultBody::Ready {}, true),
            Err(_) => (
                ResultBody::Error {
                    code: ErrorCode::Unavailable,
                },
                false,
            ),
        },
        _ => (
            ResultBody::Error {
                code: ErrorCode::Unavailable,
            },
            false,
        ),
    }
}
/// Authoring and catalogs share physical-channel authority, not the root-local socket.
pub fn authoring_allowed(host_channel: bool, request: &RequestBody) -> bool {
    host_channel
        || !matches!(
            request,
            RequestBody::Execute {
                action: EngineActionWire::Orka { .. }
                    | EngineActionWire::OrkaChat { .. }
                    | EngineActionWire::RunWorker { .. }
                    | EngineActionWire::WorkspaceCatalog {},
                ..
            } | RequestBody::Engine(
                EngineActionWire::Orka { .. }
                    | EngineActionWire::OrkaChat { .. }
                    | EngineActionWire::RunWorker { .. }
                    | EngineActionWire::WorkspaceCatalog {}
            )
        )
}
/// Credential authority is the owned physical channel, not the local kmx socket.
pub fn credential_request(
    host_channel: bool,
    request: &RequestBody,
    run: impl FnOnce() -> ResultBody,
) -> Option<ResultBody> {
    if matches!(
        request,
        RequestBody::CredentialStatus {}
            | RequestBody::CredentialPublish { .. }
            | RequestBody::CredentialClear { .. }
            | RequestBody::ModelStatus {}
            | RequestBody::ModelConnect { .. }
            | RequestBody::ModelDisconnect { .. }
            | RequestBody::ModelDisconnectRetained { .. }
    ) {
        Some(if host_channel {
            run()
        } else {
            ResultBody::Error {
                code: ErrorCode::Unavailable,
            }
        })
    } else {
        None
    }
}

pub fn result(r: &WireRequest, body: ResultBody) -> Frame {
    Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: r.boot_nonce,
        request_id: r.request_id,
        body,
    })
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    #[test]
    fn model_operations_are_physical_only_and_do_not_fall_through() {
        let id = Uuid::new_v4();
        for request in [
            RequestBody::ModelStatus {},
            RequestBody::ModelConnect {
                generation: id,
                selection: ModelSelection {
                    provider: "copilot".into(),
                    model: "gpt-4.1".into(),
                },
            },
            RequestBody::ModelDisconnect { connection: id },
            RequestBody::ModelDisconnectRetained {
                connection: ModelConnection {
                    id,
                    generation: id,
                    selection: ModelSelection {
                        provider: "copilot".into(),
                        model: "gpt-4.1".into(),
                    },
                },
            },
        ] {
            for host in [false, true] {
                let called = std::cell::Cell::new(false);
                let reply = credential_request(host, &request, || {
                    called.set(true);
                    ResultBody::ModelState(ModelState {
                        retained_cleanup: true,
                        phase: ModelPhase::Idle,
                        connection: None,
                        diagnostic: None,
                        credential: CredentialState {
                            profile: CREDENTIAL_PROFILE,
                            generation: None,
                            installed: false,
                        },
                    })
                });
                assert!(reply.is_some(), "model operations must not fall through");
                assert_eq!(called.get(), host);
                if !host {
                    assert_eq!(
                        reply,
                        Some(ResultBody::Error {
                            code: ErrorCode::Unavailable
                        })
                    );
                }
            }
        }
        assert!(
            credential_request(true, &RequestBody::Readiness {}, || panic!(
                "non-private operation"
            ))
            .is_none()
        );
    }
    #[test]
    fn local_peer_never_reaches_a_working_credential_handler() {
        for host in [false, true] {
            let called = std::cell::Cell::new(false);
            let reply = credential_request(host, &RequestBody::CredentialStatus {}, || {
                called.set(true);
                ResultBody::Ready {}
            });
            assert_eq!(
                called.get(),
                host,
                "local request must not execute credential code"
            );
            assert!(reply.is_some());
        }
    }
}
