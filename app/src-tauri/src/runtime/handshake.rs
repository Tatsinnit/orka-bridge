//! Host boot exchange over the existing private guest protocol.
use super::{
    client::{GuestChannel, VmClient},
    types::RuntimeError,
    RuntimeWorker,
};
use bridge_runtime_protocol::{encode, Decoder, Frame, ResultBody, Stage, Uuid};
use std::io::{self, BufReader, Read, Write};

struct BufferedChannel(BufReader<Box<dyn GuestChannel>>);
impl Read for BufferedChannel {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.read(bytes)
    }
}
impl Write for BufferedChannel {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.get_mut().write(bytes)
    }
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.0.get_mut().write_all(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.get_mut().flush()
    }
}
fn receive(
    reader: &mut BufReader<Box<dyn GuestChannel>>,
    decoder: &mut Decoder,
) -> Result<Frame, RuntimeError> {
    loop {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        if let Some(frame) = decoder.push(byte[0])? {
            return Ok(frame);
        }
    }
}
pub fn establish(
    worker: &mut RuntimeWorker,
    channel: Box<dyn GuestChannel>,
    host_utc: u64,
) -> Result<VmClient, RuntimeError> {
    let nonce = worker
        .lease()
        .ok_or(RuntimeError::Unavailable)?
        .spec
        .boot_nonce;
    let result = exchange(worker, channel, nonce, host_utc);
    if let Err(ref exchange) = result {
        if let Err(stop) = worker.stop() {
            return Err(RuntimeError::StopUnconfirmed(format!(
                "boot exchange failed: {exchange}; stop unobserved: {stop}"
            )));
        }
    }
    result
}
fn exchange(
    worker: &mut RuntimeWorker,
    channel: Box<dyn GuestChannel>,
    nonce: Uuid,
    host_utc: u64,
) -> Result<VmClient, RuntimeError> {
    let mut reader = BufReader::new(channel);
    let mut serving_hello = None;
    for stage in [Stage::Early, Stage::Serve] {
        // A distinct expected stage, not a poisoned decoder reset. The shared
        // worker validates the actual nonce transition and durable New authority.
        let mut decoder = Decoder::new(stage, nonce);
        let Frame::Hello(hello) = receive(&mut reader, &mut decoder)? else {
            return Err(RuntimeError::Unavailable);
        };
        let request = worker.boot_request(&hello, host_utc)?;
        if stage == Stage::Serve {
            serving_hello = Some(hello);
        }
        reader
            .get_mut()
            .write_all(&encode(&Frame::Request(request.clone()))?)?;
        reader.get_mut().flush()?;
        match receive(&mut reader, &mut decoder)? {
            Frame::Result(reply)
                if reply.request_id == request.request_id
                    && matches!(reply.body, ResultBody::Ready {}) => {}
            _ => return Err(RuntimeError::Unavailable),
        }
    }
    // Carry all prefetched bytes through the existing client constructor.
    VmClient::established_with_hello(
        Box::new(BufferedChannel(reader)),
        nonce,
        serving_hello.as_ref().ok_or(RuntimeError::Unavailable)?,
    )
    .map_err(Into::into)
}
