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
        if result.is_err() {
            self.poisoned = true;
            self.bytes.clear();
        }
        result
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
        RequestBody::Readiness {} => match system.ready(b) {
            Ok(()) => (ResultBody::Ready {}, false),
            Err(_) => (
                ResultBody::Error {
                    code: ErrorCode::IncompatibleState,
                },
                false,
            ),
        },
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
pub fn result(r: &WireRequest, body: ResultBody) -> Frame {
    Frame::Result(WireResult {
        version: VERSION,
        boot_nonce: r.boot_nonce,
        request_id: r.request_id,
        body,
    })
}
