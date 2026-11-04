use super::{types::*, EngineTransport, TransportFuture};

pub struct VmTransport;
impl EngineTransport for VmTransport {
    fn execute(&self, _request: EngineRequest) -> TransportFuture<'_> {
        Box::pin(async { Err(EngineError::new(ErrorKind::Unavailable)) })
    }
}
