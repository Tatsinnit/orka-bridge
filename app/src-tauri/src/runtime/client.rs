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
        })
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
