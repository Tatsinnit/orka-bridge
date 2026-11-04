use super::{types::*, EngineTransport, TransportFuture};
use std::{path::PathBuf, process::Stdio};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::watch,
};

pub struct ProcessTransport {
    path: PathBuf,
    closed: watch::Receiver<bool>,
}
impl ProcessTransport {
    pub fn new(path: PathBuf, closed: watch::Receiver<bool>) -> Self {
        Self { path, closed }
    }
}
async fn bounded(reader: impl AsyncRead + Unpin) -> Result<Vec<u8>, EngineError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_OUTPUT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| EngineError::new(ErrorKind::Execution))?;
    if bytes.len() > MAX_OUTPUT {
        return Err(EngineError::new(ErrorKind::InvalidResponse));
    }
    Ok(bytes)
}
impl EngineTransport for ProcessTransport {
    fn execute(&self, request: EngineRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            let mut closed = self.closed.clone();
            if *closed.borrow() {
                return Err(EngineError::cancelled());
            }
            let mut child = Command::new(&self.path)
                .args(request.action.argv())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| {
                    EngineError::new(if error.kind() == std::io::ErrorKind::NotFound {
                        ErrorKind::Unavailable
                    } else {
                        ErrorKind::Execution
                    })
                })?;
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            // Drain both pipes concurrently. On either bound/error, drop readers,
            // then stop and reap the owned child; never wait on a full other pipe.
            let collected = tokio::select! {
                biased;
                _ = closed.changed() => Err(EngineError::cancelled()),
                result = async { tokio::try_join!(bounded(stdout), bounded(stderr)) } => result,
            };
            let (stdout, stderr) = match collected {
                Ok(data) => data,
                Err(error) => {
                    let _ = child.kill().await;
                    return Err(error);
                }
            };
            let status = tokio::select! {
                biased;
                _ = closed.changed() => { let _ = child.kill().await; return Err(EngineError::cancelled()); }
                result = child.wait() => result.map_err(|_| EngineError::new(ErrorKind::Execution))?,
            };
            Ok(EngineOutput {
                exit: status.code().unwrap_or(-1),
                stdout,
                stderr,
            })
        })
    }
}
