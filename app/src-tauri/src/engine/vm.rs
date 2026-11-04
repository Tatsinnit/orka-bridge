use super::{types::*, EngineTransport, TransportFuture};
use crate::runtime::client::VmClient;
use std::sync::Arc;
use tokio::sync::Mutex;
/// A live client is supplied only by the owned runtime, never an environment override.
#[derive(Default)]
pub struct VmTransport(Option<Arc<Mutex<VmClient>>>);
impl VmTransport {
    pub fn with_client(client: VmClient) -> Self {
        Self(Some(Arc::new(Mutex::new(client))))
    }
}
impl EngineTransport for VmTransport {
    fn execute(&self, request: EngineRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            let client = self
                .0
                .clone()
                .ok_or_else(|| EngineError::new(ErrorKind::Unavailable))?;
            // Refuse excess work before consuming a shared blocking-pool worker.
            let mut client = client
                .try_lock_owned()
                .map_err(|_| EngineError::new(ErrorKind::Execution))?;
            tokio::task::spawn_blocking(move || {
                let id = client
                    .execute(request.action, None)
                    .map_err(|_| EngineError::new(ErrorKind::Execution))?;
                let result = client
                    .result(id)
                    .map_err(|_| EngineError::new(ErrorKind::Execution))?;
                Ok(EngineOutput {
                    exit: result.exit,
                    stdout: result.stdout,
                    stderr: if result.has_diagnostics {
                        vec![1]
                    } else {
                        Vec::new()
                    },
                })
            })
            .await
            .map_err(|_| EngineError::new(ErrorKind::Execution))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{self, Read, Write},
        time::Duration,
    };
    struct Closed;
    impl Read for Closed {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }
    }
    impl Write for Closed {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn concurrent_admission_never_parks_a_blocking_worker_on_the_client_lock() {
        let vm = VmTransport::with_client(
            VmClient::established(Box::new(Closed), bridge_runtime_protocol::Uuid::new_v4())
                .unwrap(),
        );
        let client = vm.0.clone().unwrap();
        let _held = client.try_lock().unwrap();
        let response = tokio::time::timeout(
            Duration::from_millis(100),
            vm.execute(EngineRequest {
                action: EngineAction::Version {},
            }),
        )
        .await;
        assert!(
            response.is_ok(),
            "a busy client must be refused before spawning blocking work"
        );
        assert_eq!(response.unwrap().unwrap_err().kind, ErrorKind::Execution);
    }
}
