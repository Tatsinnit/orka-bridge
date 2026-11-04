use bridge_guest::{
    early::{self, BootSystem},
    linux::{self, Linux},
    peer,
    service::{self, Session},
};
use bridge_runtime_protocol::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            net::UnixListener,
        },
    },
    time::{Duration, Instant},
};

fn wait(fd: i32, event: i16, deadline: Instant) -> io::Result<()> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(early::invalid)?;
        let mut poll = libc::pollfd {
            fd,
            events: event,
            revents: 0,
        };
        // SAFETY: a single initialized pollfd is writable for the duration of poll.
        let n = unsafe {
            libc::poll(
                &mut poll,
                1,
                remaining.as_millis().min(i32::MAX as u128) as i32,
            )
        };
        if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 {
            return Err(early::invalid());
        }
        if poll.revents & event != 0 {
            return Ok(());
        }
        return Err(early::invalid());
    }
}
fn read(
    stream: &mut BufReader<impl Read + AsRawFd>,
    session: &mut Session,
) -> io::Result<WireRequest> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if stream.buffer().is_empty() {
            wait(stream.get_ref().as_raw_fd(), libc::POLLIN, deadline)?
        }
        let bytes = match stream.fill_buf() {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                continue
            }
            _ => return Err(early::invalid()),
        };
        let mut used = 0;
        let mut result = None;
        for byte in bytes {
            used += 1;
            if let Some(request) = session.push(*byte)? {
                result = Some(request);
                break;
            }
        }
        stream.consume(used);
        if let Some(request) = result {
            return Ok(request);
        }
    }
}
fn write(stream: &mut (impl Write + AsRawFd), frame: Frame) -> io::Result<()> {
    let bytes = encode(&frame).map_err(|_| early::invalid())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut offset = 0;
    while offset < bytes.len() {
        wait(stream.as_raw_fd(), libc::POLLOUT, deadline)?;
        match stream.write(&bytes[offset..]) {
            Ok(0) => return Err(early::invalid()),
            Ok(n) => offset += n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn port() -> io::Result<File> {
    let path = fs::canonicalize(linux::PORT)?;
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW)
        .open(path)?;
    let meta = f.metadata()?;
    if !meta.file_type().is_char_device() || meta.uid() != 0 || meta.mode() & 0o077 != 0 {
        return Err(early::invalid());
    }
    // Discard bounded stale kernel bytes before announcing the new stage's reader.
    let mut stale = 0;
    let mut buf = [0; 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                stale += n;
                if stale > MAX_FRAME {
                    return Err(early::invalid());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(f)
}
struct Socket {
    listener: UnixListener,
    dev: u64,
    ino: u64,
}
impl Socket {
    fn bind() -> io::Result<Self> {
        let listener = UnixListener::bind(linux::SOCKET)?;
        fs::set_permissions(linux::SOCKET, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let m = fs::symlink_metadata(linux::SOCKET)?;
        Ok(Self {
            listener,
            dev: m.dev(),
            ino: m.ino(),
        })
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        if fs::symlink_metadata(linux::SOCKET)
            .is_ok_and(|m| m.file_type().is_socket() && m.dev() == self.dev && m.ino() == self.ino)
        {
            let _ = fs::remove_file(linux::SOCKET);
        }
    }
}
fn control(
    io: &mut BufReader<impl Read + Write + AsRawFd>,
    session: &mut Session,
    system: &mut Linux,
    b: &BootDescriptor,
    operations: &mut bridge_guest::engine::Operations<bridge_guest::engine::Kmx>,
    staging: &mut bridge_guest::staging::Staging,
) -> io::Result<bool> {
    let r = read(io, session)?;
    let (body, stop) = if matches!(r.body, RequestBody::Readiness {} | RequestBody::Shutdown {}) {
        service::handle(system, b, &r)
    } else {
        (
            operations
                .handle(staging, &r.body)
                .unwrap_or(ResultBody::Error {
                    code: ErrorCode::InvalidRequest,
                }),
            false,
        )
    };
    write(io.get_mut(), service::result(&r, body))?;
    Ok(stop)
}
type Serving = (
    bridge_guest::engine::Operations<bridge_guest::engine::Kmx>,
    bridge_guest::staging::Staging,
);
fn serve_ready(
    root: &std::path::Path,
    ready: impl FnOnce() -> io::Result<()>,
) -> io::Result<Serving> {
    let operations =
        bridge_guest::engine::Operations::new(root.join("operations"), bridge_guest::engine::Kmx)?;
    let staging = bridge_guest::staging::Staging::new(root.join("snapshots"))?;
    ready()?;
    Ok((operations, staging))
}
fn run(mode: &str) -> io::Result<()> {
    // SAFETY: geteuid has no pointers or preconditions.
    if unsafe { libc::geteuid() } != 0 {
        return Err(early::invalid());
    }
    let early_mode = mode == "--early";
    let saved = if early_mode {
        None
    } else {
        Some(Linux::accepted()?)
    };
    let mut system = if early_mode {
        Linux::early()?
    } else {
        Linux::serve()?
    };
    if let Some(b) = &saved {
        system.ready(b)?;
    }
    let mut session = if early_mode {
        Session::early()
    } else {
        Session::serve(saved.clone())?
    };
    let mut channel = BufReader::new(port()?);
    write(channel.get_mut(), Frame::Hello(session.hello().clone()))?;
    let r = read(&mut channel, &mut session)?;
    let RequestBody::Boot(b) = &r.body else {
        return Err(early::invalid());
    };
    let result = if early_mode {
        Linux::set_clock(b).and_then(|_| early::apply(&mut system, b))
    } else {
        system.ready(b)
    };
    if result.is_err() {
        write(
            channel.get_mut(),
            service::result(
                &r,
                ResultBody::Error {
                    code: ErrorCode::IncompatibleState,
                },
            ),
        )?;
        return Err(early::invalid());
    }
    let socket = if early_mode {
        None
    } else {
        Some(Socket::bind()?)
    };
    if early_mode {
        write(channel.get_mut(), service::result(&r, ResultBody::Ready {}))?;
        return Ok(());
    }
    let socket = socket.ok_or_else(early::invalid)?;
    let (mut operations, mut staging) =
        serve_ready(std::path::Path::new("/var/lib/bridge"), || {
            write(channel.get_mut(), service::result(&r, ResultBody::Ready {}))
        })?;
    loop {
        if !channel.buffer().is_empty() {
            if control(
                &mut channel,
                &mut session,
                &mut system,
                b,
                &mut operations,
                &mut staging,
            )? {
                return Ok(());
            }
            continue;
        }
        let mut fds = [
            libc::pollfd {
                fd: channel.get_ref().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: socket.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: both entries are initialized and remain live through poll.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(early::invalid());
        }
        if fds[0].revents != 0
            && control(
                &mut channel,
                &mut session,
                &mut system,
                b,
                &mut operations,
                &mut staging,
            )?
        {
            return Ok(());
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let (client, _) = socket.listener.accept()?;
            if peer::check(&client).is_err() {
                continue;
            }
            client.set_nonblocking(true)?;
            let mut client = BufReader::new(client);
            let mut local = Session::local(b.clone())?;
            // A bad local peer cannot poison the serial session or its request IDs.
            if control(
                &mut client,
                &mut local,
                &mut system,
                b,
                &mut operations,
                &mut staging,
            )
            .unwrap_or(false)
            {
                return Ok(());
            }
        }
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--help" {
        println!("bridge-guest --early | --serve\nLinux guest only; fixed paths and typed kmx operations.");
        return;
    }
    if args.len() != 1 || (args[0] != "--early" && args[0] != "--serve") {
        eprintln!("usage: bridge-guest --early | --serve");
        std::process::exit(2)
    }
    if run(args[0].to_str().unwrap_or("")).is_err() {
        eprintln!("bridge-guest: incompatible state or unavailable guest operation");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    fn boot() -> BootDescriptor {
        BootDescriptor {
            installation_id: Uuid::from_u128(42),
            generation: 1,
            boot_nonce: Uuid::from_u128(99),
            state_creation: StateCreation::New,
            state_fs_uuid: Uuid::from_u128(42),
            state_bytes: STATE_BYTES,
            host_utc: 1,
        }
    }
    #[test]
    fn large_guest_request_is_buffered_and_preserves_the_next_frame() {
        struct Count {
            stream: UnixStream,
            reads: usize,
        }
        impl Read for Count {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.reads += 1;
                self.stream.read(b)
            }
        }
        impl AsRawFd for Count {
            fn as_raw_fd(&self) -> i32 {
                self.stream.as_raw_fd()
            }
        }
        let (mut host, guest) = UnixStream::pair().unwrap();
        let b = boot();
        let mut bytes = Vec::new();
        for (request_id, body) in [
            (
                3,
                RequestBody::StageChunk {
                    upload: Uuid::new_v4(),
                    slot: FileSlot::Agent,
                    offset: 0,
                    data: encode_chunk(&vec![1; MAX_CHUNK]),
                },
            ),
            (4, RequestBody::Readiness {}),
        ] {
            bytes.extend(
                encode(&Frame::Request(WireRequest {
                    version: VERSION,
                    boot_nonce: b.boot_nonce,
                    request_id,
                    body,
                }))
                .unwrap(),
            );
        }
        let sender = std::thread::spawn(move || host.write_all(&bytes).unwrap());
        let mut guest = BufReader::new(Count {
            stream: guest,
            reads: 0,
        });
        let mut session = Session::local(b).unwrap();
        assert_eq!(read(&mut guest, &mut session).unwrap().request_id, 3);
        assert_eq!(read(&mut guest, &mut session).unwrap().request_id, 4);
        assert!(guest.get_ref().reads < 16);
        sender.join().unwrap();
    }
    #[test]
    fn readiness_is_not_acknowledged_if_serving_store_initialization_fails() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("operations"), b"not a directory").unwrap();
        let mut acknowledged = false;
        assert!(serve_ready(root.path(), || {
            acknowledged = true;
            Ok(())
        })
        .is_err());
        assert!(!acknowledged);
        fs::remove_file(root.path().join("operations")).unwrap();
        assert!(serve_ready(root.path(), || {
            acknowledged = true;
            Ok(())
        })
        .is_ok());
        assert!(acknowledged);
    }
    #[test]
    fn real_socket_io_accepts_the_host_boot_frame() {
        let (mut host, guest) = UnixStream::pair().unwrap();
        let b = boot();
        let frame = Frame::Request(WireRequest {
            version: VERSION,
            boot_nonce: b.boot_nonce,
            request_id: 1,
            body: RequestBody::Boot(b.clone()),
        });
        let bytes = encode(&frame).unwrap();
        let sender = std::thread::spawn(move || host.write_all(&bytes).unwrap());
        let got = read(&mut BufReader::new(guest), &mut Session::early()).unwrap();
        assert!(matches!(got.body,RequestBody::Boot(saved) if saved==b));
        sender.join().unwrap();
    }
    #[test]
    fn real_socket_output_is_accepted_by_host_decoder() {
        let (mut host, mut guest) = UnixStream::pair().unwrap();
        let b = boot();
        write(&mut guest, Frame::Hello(Session::early().hello().clone())).unwrap();
        write(
            &mut guest,
            Frame::Result(WireResult {
                version: VERSION,
                boot_nonce: b.boot_nonce,
                request_id: 1,
                body: ResultBody::Ready {},
            }),
        )
        .unwrap();
        drop(guest);
        let mut bytes = Vec::new();
        host.read_to_end(&mut bytes).unwrap();
        let mut decoder = Decoder::new(Stage::Early, b.boot_nonce);
        assert_eq!(
            bytes
                .into_iter()
                .filter_map(|b| decoder.push(b).unwrap())
                .count(),
            2
        );
    }
}
