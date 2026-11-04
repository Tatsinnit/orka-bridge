//! Version-one private serial protocol. No Tauri, process runner or guest installer.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
pub use uuid::Uuid;

mod operations;
pub use operations::*;
pub fn encode_chunk(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
pub fn decode_chunk(data: &str) -> Result<Vec<u8>, ProtocolError> {
    chunk(data)?;
    STANDARD.decode(data).map_err(|_| ProtocolError::Invalid)
}
pub const VERSION: u16 = 1;
/// Includes the terminating LF. A decoder retains at most this many bytes.
pub const MAX_FRAME: usize = 128 * 1024;
pub const MAX_CHUNK: usize = 32 * 1024;
pub const STATE_BYTES: u64 = 34_359_738_368;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Early,
    Serve,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StateCreation {
    New,
    Existing,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootDescriptor {
    pub installation_id: Uuid,
    pub generation: u64,
    pub boot_nonce: Uuid,
    pub state_creation: StateCreation,
    pub state_fs_uuid: Uuid,
    pub state_bytes: u64,
    /// Seconds since the Unix epoch, in UTC; not a guest-supplied clock.
    pub host_utc: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageHello {
    pub version: u16,
    pub stage: Stage,
    pub guest_nonce: Uuid,
    pub accepted_boot_nonce: Option<Uuid>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum EngineActionWire {
    Version {},
    ListAgents { context: String, namespace: String },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "body",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum RequestBody {
    Boot(BootDescriptor),
    Engine(EngineActionWire),
    Chunk {
        data: String,
    },
    StageBegin {
        upload: Uuid,
        snapshot: SnapshotId,
    },
    StageChunk {
        upload: Uuid,
        slot: FileSlot,
        offset: usize,
        data: String,
    },
    StageCommit {
        upload: Uuid,
    },
    Execute {
        operation: Uuid,
        action: EngineActionWire,
        snapshot: Option<SnapshotId>,
    },
    Status {
        operation: Uuid,
    },
    ReadResult {
        operation: Uuid,
        offset: usize,
    },
    Readiness {},
    Shutdown {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireRequest {
    pub version: u16,
    pub boot_nonce: Uuid,
    pub request_id: u64,
    pub body: RequestBody,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "op",
    content = "body",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum ResultBody {
    Ready {},
    Snapshot {
        snapshot: SnapshotId,
    },
    Operation {
        operation: Uuid,
        info: OperationInfo,
    },
    Chunk {
        data: String,
    },
    Error {
        code: ErrorCode,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    InvalidRequest,
    IncompatibleState,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireResult {
    pub version: u16,
    pub boot_nonce: Uuid,
    pub request_id: u64,
    pub body: ResultBody,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "body",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum Frame {
    Hello(StageHello),
    Request(WireRequest),
    Result(WireResult),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Version,
    Invalid,
    Oversize,
    Stage,
    Nonce,
}
impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid runtime protocol: {self:?}")
    }
}
impl std::error::Error for ProtocolError {}

pub fn validate_hello(hello: &StageHello, stage: Stage, nonce: Uuid) -> Result<(), ProtocolError> {
    if hello.version != VERSION {
        return Err(ProtocolError::Version);
    }
    if hello.stage != stage {
        return Err(ProtocolError::Stage);
    }
    if hello.guest_nonce.is_nil()
        || match stage {
            Stage::Early => hello.accepted_boot_nonce.is_some(),
            Stage::Serve => hello.accepted_boot_nonce != Some(nonce),
        }
    {
        return Err(ProtocolError::Nonce);
    }
    Ok(())
}
pub fn validate_action(action: &EngineActionWire) -> Result<(), ProtocolError> {
    if let EngineActionWire::ListAgents { context, namespace } = action {
        if [context, namespace]
            .iter()
            .any(|s| s.trim().is_empty() || s.len() > 4096 || s.contains('\0'))
        {
            return Err(ProtocolError::Invalid);
        }
    }
    Ok(())
}
fn chunk(data: &str) -> Result<(), ProtocolError> {
    if data.len() > MAX_CHUNK.div_ceil(3) * 4 {
        return Err(ProtocolError::Oversize);
    }
    let decoded = STANDARD.decode(data).map_err(|_| ProtocolError::Invalid)?;
    if decoded.len() > MAX_CHUNK {
        return Err(ProtocolError::Oversize);
    }
    Ok(())
}
impl Frame {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let (version, nonce, id) = match self {
            Self::Hello(h) => (h.version, h.guest_nonce, 1),
            Self::Request(r) => {
                match &r.body {
                    RequestBody::Boot(b)
                        if b.installation_id.is_nil()
                            || b.state_fs_uuid != b.installation_id
                            || b.generation == 0
                            || b.boot_nonce != r.boot_nonce
                            || b.state_bytes != STATE_BYTES =>
                    {
                        return Err(ProtocolError::Invalid)
                    }
                    RequestBody::StageBegin { upload, snapshot }
                        if upload.is_nil() || !snapshot.valid() =>
                    {
                        return Err(ProtocolError::Invalid)
                    }
                    RequestBody::StageChunk {
                        upload,
                        offset,
                        data,
                        ..
                    } => {
                        if upload.is_nil() || *offset > MAX_FILE {
                            return Err(ProtocolError::Invalid);
                        }
                        chunk(data)?
                    }
                    RequestBody::StageCommit { upload } if upload.is_nil() => {
                        return Err(ProtocolError::Invalid)
                    }
                    RequestBody::Execute {
                        operation,
                        snapshot,
                        ..
                    } if operation.is_nil() || snapshot.as_ref().is_some_and(|s| !s.valid()) => {
                        return Err(ProtocolError::Invalid)
                    }
                    RequestBody::Status { operation }
                    | RequestBody::ReadResult { operation, .. }
                        if operation.is_nil() =>
                    {
                        return Err(ProtocolError::Invalid)
                    }
                    RequestBody::Execute { action, .. } => validate_action(action)?,
                    RequestBody::Chunk { data } => chunk(data)?,
                    RequestBody::Engine(EngineActionWire::ListAgents { context, namespace }) => {
                        if context.trim().is_empty() || namespace.trim().is_empty() {
                            return Err(ProtocolError::Invalid);
                        }
                        if context.len() > MAX_FRAME || namespace.len() > MAX_FRAME {
                            return Err(ProtocolError::Oversize);
                        }
                    }
                    _ => {}
                }
                (r.version, r.boot_nonce, r.request_id)
            }
            Self::Result(r) => {
                if let ResultBody::Chunk { data } = &r.body {
                    chunk(data)?;
                }
                (r.version, r.boot_nonce, r.request_id)
            }
        };
        if version != VERSION {
            return Err(ProtocolError::Version);
        }
        if nonce.is_nil() || id == 0 {
            return Err(ProtocolError::Invalid);
        }
        Ok(())
    }
}
pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    frame.validate()?;
    let mut bytes = serde_json::to_vec(frame).map_err(|_| ProtocolError::Invalid)?;
    if bytes.len() >= MAX_FRAME {
        return Err(ProtocolError::Oversize);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

/// Host receive decoder. Guest endpoint acceptance is implemented separately.
/// Feed bounded read buffers one byte at a time; results are delivered individually,
/// not accumulated into an unbounded batch. An error poisons the current stage.
pub struct Decoder {
    buffer: Vec<u8>,
    stage: Stage,
    nonce: Uuid,
    needs_hello: bool,
    previous_guest: Option<Uuid>,
    poisoned: bool,
}
impl Decoder {
    pub fn new(stage: Stage, nonce: Uuid) -> Self {
        Self {
            buffer: Vec::new(),
            stage,
            nonce,
            needs_hello: true,
            previous_guest: None,
            poisoned: false,
        }
    }
    /// Called only on an observed port close/reopen, never as error recovery.
    pub fn begin_stage(&mut self, stage: Stage) {
        self.buffer.clear();
        self.stage = stage;
        self.needs_hello = true;
        self.poisoned = false;
    }
    pub fn push(&mut self, byte: u8) -> Result<Option<Frame>, ProtocolError> {
        if self.poisoned {
            return Err(ProtocolError::Invalid);
        }
        let result = self.push_inner(byte);
        if result.is_err() {
            self.buffer.clear();
            self.poisoned = true;
        }
        result
    }
    fn push_inner(&mut self, byte: u8) -> Result<Option<Frame>, ProtocolError> {
        if self.buffer.len() >= MAX_FRAME - 1 && byte != b'\n' {
            return Err(ProtocolError::Oversize);
        }
        if byte != b'\n' {
            self.buffer.push(byte);
            return Ok(None);
        }
        let text = std::str::from_utf8(&self.buffer).map_err(|_| ProtocolError::Invalid)?;
        let frame: Frame = serde_json::from_str(text).map_err(|_| ProtocolError::Invalid)?;
        frame.validate()?;
        match &frame {
            Frame::Hello(h) if self.needs_hello => {
                validate_hello(h, self.stage, self.nonce)?;
                if self.previous_guest == Some(h.guest_nonce) {
                    return Err(ProtocolError::Nonce);
                }
                self.previous_guest = Some(h.guest_nonce);
                self.needs_hello = false;
            }
            Frame::Hello(_) => return Err(ProtocolError::Stage),
            _ if self.needs_hello => return Err(ProtocolError::Stage),
            Frame::Request(_) => return Err(ProtocolError::Stage),
            Frame::Result(r) if r.boot_nonce != self.nonce => return Err(ProtocolError::Nonce),
            _ => {}
        }
        self.buffer.clear();
        Ok(Some(frame))
    }
}
