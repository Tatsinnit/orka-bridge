use super::snapshot::Snapshot;
use crate::engine::types::EngineAction;
use bridge_runtime_protocol::*;
use std::{
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
};
/// Implementations must enforce native per-I/O deadlines. Construct only after
/// the R1 serving-stage handshake and Ready; never from renderer-owned handles.
pub trait GuestChannel: Read + Write + Send {}
impl<T: Read + Write + Send> GuestChannel for T {}
pub struct VmClient {
    channel: Option<BufReader<Box<dyn GuestChannel>>>,
    nonce: Uuid,
    next: u64,
    run_cleanup: bool,
}
fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}
impl VmClient {
    pub fn established(channel: Box<dyn GuestChannel>, nonce: Uuid) -> io::Result<Self> {
        if nonce.is_nil() {
            return Err(invalid());
        }
        Ok(Self {
            channel: Some(BufReader::new(channel)),
            nonce,
            next: 3,
            run_cleanup: false,
        })
    }
    /// Capabilities come from the validated serving hello, never a probe that
    /// could poison an older strict guest's sole channel.
    pub fn established_with_hello(
        channel: Box<dyn GuestChannel>,
        nonce: Uuid,
        hello: &StageHello,
    ) -> io::Result<Self> {
        validate_hello(hello, Stage::Serve, nonce).map_err(|_| invalid())?;
        let mut client = Self::established(channel, nonce)?;
        client.run_cleanup = hello.run_cleanup;
        Ok(client)
    }
    pub fn cleanup_run_agent(&mut self, uid: &str) -> io::Result<CleanupRunReply> {
        if !self.run_cleanup {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let action = EngineActionWire::CleanupRunAgent { uid: uid.into() };
        validate_action(&action).map_err(|_| invalid())?;
        let operation = Uuid::new_v4();
        match self.call(RequestBody::Execute {
            operation,
            action,
            snapshot: None,
        })? {
            ResultBody::Operation { operation: id, .. } if id == operation => {}
            _ => return Err(invalid()),
        }
        let result = self.result(operation)?;
        if result.exit != 0 {
            return Err(invalid());
        }
        serde_json::from_slice(&result.stdout).map_err(|_| invalid())
    }
    fn call(&mut self, body: RequestBody) -> io::Result<ResultBody> {
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or_else(invalid)?;
        let bytes = encode(&Frame::Request(WireRequest {
            version: VERSION,
            boot_nonce: self.nonce,
            request_id: id,
            body,
        }))
        .map_err(|_| invalid())?;
        let result = (|| {
            let io = self.channel.as_mut().ok_or_else(invalid)?;
            io.get_mut().write_all(&bytes)?;
            io.get_mut().flush()?;
            let mut bytes = Vec::new();
            loop {
                let buffer = io.fill_buf()?;
                if buffer.is_empty() {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                }
                let count = buffer
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(buffer.len(), |n| n + 1);
                if bytes.len() + count > MAX_FRAME {
                    return Err(invalid());
                }
                let done = buffer[count - 1] == b'\n';
                bytes.extend_from_slice(&buffer[..count]);
                io.consume(count);
                if done {
                    break;
                }
            }
            if bytes.last() != Some(&b'\n') {
                return Err(invalid());
            }
            let frame: Frame = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            frame.validate().map_err(|_| invalid())?;
            match frame {
                Frame::Result(r) if r.request_id == id && r.boot_nonce == self.nonce => Ok(r.body),
                _ => Err(invalid()),
            }
        })();
        if result.is_err() {
            self.channel = None;
        }
        result
    }
    pub fn boot_nonce(&self) -> Uuid {
        self.nonce
    }

    /// Probe before loading a stored secret; an old strict peer may close here.
    pub fn model_status(&mut self) -> io::Result<ModelState> {
        self.model_call(RequestBody::ModelStatus {})
    }
    pub fn connect_model(
        &mut self,
        generation: Uuid,
        selection: ModelSelection,
    ) -> io::Result<ModelState> {
        if generation.is_nil() || !selection.valid() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let state = self.model_call(RequestBody::ModelConnect {
            generation,
            selection: selection.clone(),
        })?;
        if state
            .connection
            .as_ref()
            .is_some_and(|c| c.generation != generation || c.selection != selection)
        {
            self.channel = None;
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(state)
    }
    pub fn disconnect_retained_model(&mut self, connection: &ModelConnection) -> io::Result<()> {
        self.ready_call(RequestBody::ModelDisconnectRetained {
            connection: connection.clone(),
        })
    }
    pub fn disconnect_model(&mut self, connection: &ModelConnection) -> io::Result<ModelState> {
        let state = self.model_call(RequestBody::ModelDisconnect {
            connection: connection.id,
        })?;
        if state.credential.generation != Some(connection.generation)
            || state.connection.as_ref().is_some_and(|c| c != connection)
        {
            self.channel = None;
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(state)
    }
    fn model_call(&mut self, request: RequestBody) -> io::Result<ModelState> {
        match self.call(request)? {
            ResultBody::ModelState(state) => Ok(state),
            ResultBody::Error {
                code: ErrorCode::Unavailable,
            } => Err(io::ErrorKind::Unsupported.into()),
            _ => {
                self.channel = None;
                Err(io::ErrorKind::InvalidData.into())
            }
        }
    }
    fn credential_call(&mut self, request: RequestBody) -> io::Result<CredentialState> {
        match self.call(request)? {
            ResultBody::CredentialState(state) => Ok(state),
            ResultBody::Error { .. } => Err(io::ErrorKind::Unsupported.into()),
            _ => {
                self.channel = None;
                Err(invalid())
            }
        }
    }
    pub fn readiness(&mut self) -> io::Result<InfrastructureState> {
        match self.call(RequestBody::Readiness {})? {
            ResultBody::Infrastructure(state) => Ok(state),
            ResultBody::Error { .. } => Err(io::Error::other("guest readiness refused")),
            _ => {
                self.channel = None;
                Err(invalid())
            }
        }
    }
    // Lifecycle shutdown is routed by the native Windows session; other
    // platform session adapters are not connected in this slice.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn shutdown(&mut self) -> io::Result<()> {
        self.ready_call(RequestBody::Shutdown {})
    }
    fn ready_call(&mut self, body: RequestBody) -> io::Result<()> {
        match self.call(body)? {
            ResultBody::Ready {} => Ok(()),
            ResultBody::Error { .. } => Err(io::Error::other("guest runtime refused request")),
            _ => {
                self.channel = None;
                Err(invalid())
            }
        }
    }
    /// Nonsecret negotiation. An old guest may close its strict protocol session.
    pub fn credential_status(&mut self) -> io::Result<CredentialState> {
        self.credential_call(RequestBody::CredentialStatus {})
    }
    /// One attempt only. A failed delivery may have committed; never replay it.
    pub fn deliver_credential(
        &mut self,
        token: &CredentialPayload,
        expected: Option<Uuid>,
    ) -> io::Result<CredentialState> {
        let before = self.credential_status()?;
        if before.generation != expected || before.installed {
            return Err(invalid());
        }
        let state = self.credential_call(RequestBody::CredentialPublish {
            profile: CREDENTIAL_PROFILE,
            expected_generation: expected,
            token: token.clone(),
        })?;
        if !state.installed || state.generation.is_none() || state.generation == expected {
            self.channel = None;
            return Err(invalid());
        }
        Ok(state)
    }
    /// File-only primitive; caller must observe gateway Disconnect before clearing.
    pub fn clear_credential_file(&mut self, expected: Uuid) -> io::Result<CredentialState> {
        if self.credential_status()?.generation != Some(expected) {
            return Err(invalid());
        }
        let state = self.credential_call(RequestBody::CredentialClear {
            expected_generation: expected,
        })?;
        if state.installed || state.generation.is_none() || state.generation == Some(expected) {
            self.channel = None;
            return Err(invalid());
        }
        Ok(state)
    }
    pub fn stage_snapshot(&mut self, s: &Snapshot) -> io::Result<SnapshotId> {
        let upload = Uuid::new_v4();
        let snapshot = s.id();
        if !matches!(
            self.call(RequestBody::StageBegin {
                upload,
                snapshot: snapshot.clone()
            })?,
            ResultBody::Ready {}
        ) {
            return Err(invalid());
        }
        for slot in [FileSlot::Agent, FileSlot::Tests] {
            for (n, data) in s.file(slot).chunks(MAX_CHUNK).enumerate() {
                if !matches!(
                    self.call(RequestBody::StageChunk {
                        upload,
                        slot,
                        offset: n * MAX_CHUNK,
                        data: encode_chunk(data)
                    })?,
                    ResultBody::Ready {}
                ) {
                    return Err(invalid());
                }
            }
        }
        match self.call(RequestBody::StageCommit { upload })? {
            ResultBody::Snapshot { snapshot: s } if s == snapshot => Ok(s),
            _ => Err(invalid()),
        }
    }
    pub fn execute(
        &mut self,
        action: EngineAction,
        snapshot: Option<SnapshotId>,
    ) -> io::Result<Uuid> {
        if self.channel.is_none() {
            return Err(invalid());
        }
        let operation = Uuid::new_v4();
        let action = match action {
            EngineAction::Version {} => EngineActionWire::Version {},
            EngineAction::ListAgents { context, namespace } => {
                EngineActionWire::ListAgents { context, namespace }
            }
        };
        // An unsuccessful write/read may have executed. Keep the ID, never resend.
        let reply = self.call(RequestBody::Execute {
            operation,
            action,
            snapshot,
        });
        if let Ok(body) = reply {
            match body {
                ResultBody::Operation { operation: id, .. } if id == operation => {}
                // A framed guest error is not desynchronization. It can follow
                // a failed durable result write, so retain Unknown, not retry.
                ResultBody::Error { .. } => {}
                _ => {
                    self.channel = None;
                }
            }
        }
        Ok(operation)
    }
    /// Trusted native Run only; uses the original request sequence and never retries a POST.
    pub fn orka_request(&mut self, request: OrkaRequest) -> io::Result<OrkaReply> {
        if request.validate().is_err() || self.channel.is_none() {
            return Err(invalid());
        }
        let operation = Uuid::new_v4();
        match self.call(RequestBody::Execute {
            operation,
            action: EngineActionWire::Orka { request },
            snapshot: None,
        })? {
            ResultBody::Operation { operation: id, .. } if id == operation => {}
            _ => return Err(invalid()),
        }
        let result = self.result(operation)?;
        if result.exit != 0 {
            return Err(invalid());
        }
        serde_json::from_slice(&result.stdout).map_err(|_| invalid())
    }
    /// Native Chat only. Preserve DELETE/404 status for explicit reconciliation.
    pub fn chat_request(&mut self, request: ChatRequest) -> io::Result<OrkaReply> {
        if request.validate().is_err() || self.channel.is_none() {
            return Err(invalid());
        }
        let operation = Uuid::new_v4();
        match self.call(RequestBody::Execute {
            operation,
            action: EngineActionWire::OrkaChat { request },
            snapshot: None,
        })? {
            ResultBody::Operation { operation: id, .. } if id == operation => {}
            ResultBody::Error { .. } => return Err(io::ErrorKind::Unsupported.into()),
            _ => return Err(invalid()),
        }
        let result = self.result(operation)?;
        if result.exit != 0 {
            return Err(io::ErrorKind::Unsupported.into());
        }
        serde_json::from_slice(&result.stdout).map_err(|_| invalid())
    }
    /// Trusted native catalog only; no renderer-owned request or target.
    pub fn workspace_catalog(&mut self) -> io::Result<WorkspaceCatalog> {
        let operation = Uuid::new_v4();
        match self.call(RequestBody::Execute {
            operation,
            action: EngineActionWire::WorkspaceCatalog {},
            snapshot: None,
        })? {
            ResultBody::Operation { operation: id, .. } if id == operation => {}
            ResultBody::Error { .. } => return Err(io::ErrorKind::Unsupported.into()),
            _ => return Err(invalid()),
        }
        let result = self.result(operation)?;
        if result.exit != 0 {
            return Err(invalid());
        }
        let catalog: WorkspaceCatalog =
            serde_json::from_slice(&result.stdout).map_err(|_| invalid())?;
        if !catalog.valid() {
            return Err(invalid());
        }
        Ok(catalog)
    }
    pub fn status(&mut self, operation: Uuid) -> io::Result<OperationInfo> {
        let unknown = || OperationInfo {
            state: OperationState::Unknown,
            exit: None,
            bytes: 0,
            has_diagnostics: false,
        };
        if self.channel.is_none() {
            return Ok(unknown());
        }
        match self.call(RequestBody::Status { operation })? {
            ResultBody::Operation {
                operation: id,
                info,
            } if id == operation && info.bytes <= MAX_FILE => Ok(info),
            _ => Err(invalid()),
        }
    }
    pub fn result(&mut self, id: Uuid) -> io::Result<EngineResult> {
        let info = self.status(id)?;
        if !matches!(
            info.state,
            OperationState::Completed | OperationState::Failed
        ) {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        while bytes.len() < info.bytes {
            match self.call(RequestBody::ReadResult {
                operation: id,
                offset: bytes.len(),
            })? {
                ResultBody::Chunk { data } => {
                    let chunk = decode_chunk(&data).map_err(|_| invalid())?;
                    if chunk.is_empty() || bytes.len() + chunk.len() > info.bytes {
                        return Err(invalid());
                    }
                    bytes.extend(chunk)
                }
                _ => return Err(invalid()),
            }
        }
        Ok(EngineResult {
            exit: info.exit.ok_or_else(invalid)?,
            stdout: bytes,
            has_diagnostics: info.has_diagnostics,
        })
    }
    pub fn store_result(root: &Path, id: Uuid, bytes: &[u8]) -> io::Result<PathBuf> {
        if id.is_nil() || bytes.len() > MAX_FILE || !fs::symlink_metadata(root)?.is_dir() {
            return Err(invalid());
        }
        Self::publish_result(root, id, |file| file.write_all(bytes))
    }
    fn publish_result(
        root: &Path,
        id: Uuid,
        write: impl FnOnce(&mut fs::File) -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        let path = root.join(id.to_string());
        let temporary = root.join(format!("{id}.{}.tmp", Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let outcome = write(&mut file).and_then(|_| file.sync_all());
        drop(file);
        // Hard-link publication is atomic and refuses an existing destination on supported filesystems.
        let outcome = outcome.and_then(|_| fs::hard_link(&temporary, &path).map(|_| path));
        let _ = fs::remove_file(&temporary);
        outcome
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_result_write_leaves_no_final_or_temporary_and_allows_retry() {
        let root = tempfile::tempdir().unwrap();
        let id = Uuid::new_v4();
        let result = VmClient::publish_result(root.path(), id, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::from(io::ErrorKind::WriteZero))
        });
        assert!(result.is_err());
        assert!(!root.path().join(id.to_string()).exists());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        let final_path = VmClient::store_result(root.path(), id, b"complete").unwrap();
        assert_eq!(fs::read(&final_path).unwrap(), b"complete");
        assert_eq!(
            VmClient::store_result(root.path(), id, b"replacement")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(final_path).unwrap(), b"complete");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}
