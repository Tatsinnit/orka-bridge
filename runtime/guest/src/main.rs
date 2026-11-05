use bridge_guest::{
    diagnostics,
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

fn wait(ctx: diagnostics::Context, fd: i32, event: i16, deadline: Instant) -> io::Result<()> {
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            diagnostics::log_reason(ctx, "poll-timeout", io::ErrorKind::TimedOut, None);
            return Err(early::invalid());
        };
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
        if n < 0 {
            let saved = io::Error::last_os_error();
            if saved.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            diagnostics::note_reason(ctx, "poll-system-error", &saved);
            return Err(early::invalid());
        }
        if n == 0 {
            diagnostics::log_reason(ctx, "poll-timeout", io::ErrorKind::TimedOut, None);
            return Err(early::invalid());
        }
        if poll.revents & event != 0 {
            return Ok(());
        }
        let reason = if poll.revents & libc::POLLNVAL != 0 {
            "poll-invalid-fd"
        } else if poll.revents & libc::POLLERR != 0 {
            "poll-error"
        } else if poll.revents & libc::POLLHUP != 0 {
            "poll-hup"
        } else {
            "poll-unexpected"
        };
        diagnostics::log_reason(ctx, reason, io::ErrorKind::Other, None);
        return Err(early::invalid());
    }
}
fn read(
    ctx: diagnostics::Context,
    stream: &mut BufReader<impl Read + AsRawFd>,
    session: &mut Session,
) -> io::Result<WireRequest> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if stream.buffer().is_empty() {
            wait(ctx, stream.get_ref().as_raw_fd(), libc::POLLIN, deadline)?
        }
        let bytes = match stream.fill_buf() {
            Ok(bytes) if !bytes.is_empty() => bytes,
            Ok(_) => {
                diagnostics::log_reason(ctx, "eof", io::ErrorKind::UnexpectedEof, None);
                return Err(early::invalid());
            }
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                continue
            }
            Err(e) => {
                diagnostics::note_reason(ctx, "read-error", &e);
                return Err(early::invalid());
            }
        };
        let mut used = 0;
        let mut result = None;
        for byte in bytes {
            used += 1;
            if let Some(request) = diagnostics::context(ctx, session.push(*byte))? {
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
fn write(
    ctx: diagnostics::Context,
    stream: &mut (impl Write + AsRawFd),
    frame: Frame,
) -> io::Result<()> {
    let bytes = encode(&frame).map_err(|_| {
        diagnostics::log_reason(ctx, "encode", io::ErrorKind::InvalidData, None);
        early::invalid()
    })?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut offset = 0;
    while offset < bytes.len() {
        wait(ctx, stream.as_raw_fd(), libc::POLLOUT, deadline)?;
        match stream.write(&bytes[offset..]) {
            Ok(0) => {
                diagnostics::log_reason(ctx, "write-zero", io::ErrorKind::WriteZero, None);
                return Err(early::invalid());
            }
            Ok(n) => offset += n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                diagnostics::note_reason(ctx, "write-error", &e);
                return Err(e);
            }
        }
    }
    Ok(())
}
fn reject_boot(
    stage: &'static str,
    stream: &mut (impl Write + AsRawFd),
    request: &WireRequest,
    cause: &io::Error,
) -> io::Result<()> {
    // Preserve the primary diagnosis even if sending the fixed error reply fails.
    diagnostics::note(
        diagnostics::Context::new(stage, "boot-apply-or-ready"),
        cause,
    );
    write(
        diagnostics::Context::new(stage, "incompatible-reply-write"),
        stream,
        service::result(
            request,
            ResultBody::Error {
                code: ErrorCode::IncompatibleState,
            },
        ),
    )?;
    Err(early::invalid())
}
fn port(stage: &'static str) -> io::Result<File> {
    let path = diagnostics::context(
        diagnostics::Context::new(stage, "port-canonicalize"),
        fs::canonicalize(linux::PORT),
    )?;
    let mut f = diagnostics::context(
        diagnostics::Context::new(stage, "port-open"),
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_NOFOLLOW)
            .open(path),
    )?;
    let meta = diagnostics::context(
        diagnostics::Context::new(stage, "port-metadata"),
        f.metadata(),
    )?;
    let invalid_phase = if !meta.file_type().is_char_device() {
        Some("port-type")
    } else if meta.uid() != 0 {
        Some("port-owner")
    } else if meta.mode() & 0o077 != 0 {
        Some("port-mode")
    } else {
        None
    };
    if let Some(phase) = invalid_phase {
        diagnostics::log(
            diagnostics::Context::new(stage, phase),
            io::ErrorKind::Other,
            None,
        );
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
                    diagnostics::log(
                        diagnostics::Context::new(stage, "port-drain-overflow"),
                        io::ErrorKind::Other,
                        None,
                    );
                    return Err(early::invalid());
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                diagnostics::note(diagnostics::Context::new(stage, "port-drain-read"), &e);
                return Err(e);
            }
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
        const CTX: diagnostics::Context = diagnostics::Context::new("serve", "socket-bind");
        let listener = diagnostics::context(CTX, UnixListener::bind(linux::SOCKET))?;
        diagnostics::context(
            CTX,
            fs::set_permissions(linux::SOCKET, fs::Permissions::from_mode(0o600)),
        )?;
        diagnostics::context(CTX, listener.set_nonblocking(true))?;
        let m = diagnostics::context(CTX, fs::symlink_metadata(linux::SOCKET))?;
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
            let _ = diagnostics::context(
                diagnostics::Context::new("serve", "socket-cleanup"),
                fs::remove_file(linux::SOCKET),
            );
        }
    }
}
fn control(
    peer: &'static str,
    io: &mut BufReader<impl Read + Write + AsRawFd>,
    session: &mut Session,
    system: &mut Linux,
    b: &BootDescriptor,
    operations: &mut bridge_guest::engine::Operations<bridge_guest::engine::Kmx>,
    staging: &mut bridge_guest::staging::Staging,
) -> io::Result<bool> {
    let read_phase = if peer == "channel" {
        "channel-read"
    } else {
        "local-read"
    };
    let write_phase = if peer == "channel" {
        "channel-reply-write"
    } else {
        "local-reply-write"
    };
    let r = read(diagnostics::Context::new("serve", read_phase), io, session)?;
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
    write(
        diagnostics::Context::new("serve", write_phase),
        io.get_mut(),
        service::result(&r, body),
    )?;
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
    let operations = diagnostics::context(
        diagnostics::Context::new("serve", "operations-store-init"),
        bridge_guest::engine::Operations::new(root.join("operations"), bridge_guest::engine::Kmx),
    )?;
    let staging = diagnostics::context(
        diagnostics::Context::new("serve", "staging-store-init"),
        bridge_guest::staging::Staging::new(root.join("snapshots")),
    )?;
    ready()?;
    Ok((operations, staging))
}
fn run(mode: &str) -> io::Result<()> {
    let stage: &'static str = if mode == "--early" { "early" } else { "serve" };
    // SAFETY: geteuid has no pointers or preconditions.
    if unsafe { libc::geteuid() } != 0 {
        diagnostics::log(
            diagnostics::Context::new(stage, "euid-precheck"),
            io::ErrorKind::PermissionDenied,
            None,
        );
        return Err(early::invalid());
    }
    let early_mode = mode == "--early";
    let saved = if early_mode {
        None
    } else {
        Some(diagnostics::context(
            diagnostics::Context::new(stage, "accepted-boot-read"),
            Linux::accepted(),
        )?)
    };
    let mut system = if early_mode {
        Linux::early()?
    } else {
        Linux::serve()?
    };
    if let Some(b) = &saved {
        diagnostics::context(
            diagnostics::Context::new(stage, "saved-ready-validate"),
            system.ready(b),
        )?;
    }
    let mut session = if early_mode {
        Session::early()
    } else {
        diagnostics::context(
            diagnostics::Context::new(stage, "session-init"),
            Session::serve(saved.clone()),
        )?
    };
    let mut channel = BufReader::new(port(stage)?);
    write(
        diagnostics::Context::new(stage, "hello-write"),
        channel.get_mut(),
        Frame::Hello(session.hello().clone()),
    )?;
    let r = read(
        diagnostics::Context::new(stage, "boot-read"),
        &mut channel,
        &mut session,
    )?;
    let RequestBody::Boot(b) = &r.body else {
        diagnostics::log(
            diagnostics::Context::new(stage, "boot-request-kind"),
            io::ErrorKind::InvalidData,
            None,
        );
        return Err(early::invalid());
    };
    let result = if early_mode {
        Linux::set_clock(b).and_then(|_| early::apply(&mut system, b))
    } else {
        system.ready(b)
    };
    if let Err(error) = result {
        return reject_boot(stage, channel.get_mut(), &r, &error);
    }
    let socket = if early_mode {
        None
    } else {
        Some(Socket::bind()?)
    };
    if early_mode {
        write(
            diagnostics::Context::new(stage, "ready-reply-write"),
            channel.get_mut(),
            service::result(&r, ResultBody::Ready {}),
        )?;
        return Ok(());
    }
    let socket = socket.ok_or_else(early::invalid)?;
    let (mut operations, mut staging) =
        serve_ready(std::path::Path::new("/var/lib/bridge"), || {
            write(
                diagnostics::Context::new(stage, "ready-reply-write"),
                channel.get_mut(),
                service::result(&r, ResultBody::Ready {}),
            )
        })?;
    loop {
        if !channel.buffer().is_empty() {
            if control(
                "channel",
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
            // Save errno once, immediately, before any logging or further syscalls.
            let saved = io::Error::last_os_error();
            if saved.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            diagnostics::note(diagnostics::Context::new("serve", "accept-poll"), &saved);
            return Err(early::invalid());
        }
        if fds[0].revents != 0
            && control(
                "channel",
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
            let (client, _) = diagnostics::context(
                diagnostics::Context::new("serve", "accept"),
                socket.listener.accept(),
            )?;
            if diagnostics::context(
                diagnostics::Context::new("serve", "local-peer-check"),
                peer::check(&client),
            )
            .is_err()
            {
                continue;
            }
            diagnostics::context(
                diagnostics::Context::new("serve", "local-set-nonblocking"),
                client.set_nonblocking(true),
            )?;
            let mut client = BufReader::new(client);
            let mut local = diagnostics::context(
                diagnostics::Context::new("serve", "local-session-init"),
                Session::local(b.clone()),
            )?;
            // A bad local peer cannot poison the serial session or its request IDs.
            if control(
                "local",
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
        diagnostics::log(
            diagnostics::Context::new("cli", "arguments"),
            io::ErrorKind::InvalidInput,
            None,
        );
        diagnostics::fixed_line("usage: bridge-guest --early | --serve");
        std::process::exit(2)
    }
    let mode = args[0].to_str().unwrap_or("");
    if let Err(error) = run(mode) {
        diagnostics::note(
            diagnostics::Context::new(if mode == "--early" { "early" } else { "serve" }, "run"),
            &error,
        );
        diagnostics::fixed_line("bridge-guest: incompatible state or unavailable guest operation");
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    const CTX: diagnostics::Context = diagnostics::Context::new("serve", "test");
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
    fn diagnostic_io_child() {
        if std::env::var_os("BRIDGE_TEST_DIAGNOSTIC_IO").is_none() {
            return;
        }
        let (host, guest) = UnixStream::pair().unwrap();
        drop(host);
        assert!(read(
            diagnostics::Context::new("early", "boot-read"),
            &mut BufReader::new(guest),
            &mut Session::early(),
        )
        .is_err());
        let (host, mut guest) = UnixStream::pair().unwrap();
        drop(host);
        assert!(write(
            diagnostics::Context::new("serve", "hello-write"),
            &mut guest,
            Frame::Hello(Session::early().hello().clone()),
        )
        .is_err());
        let mut fds = [-1; 2];
        // SAFETY: fds holds two writable entries; both returned handles are owned below.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        use std::os::fd::FromRawFd;
        // SAFETY: the successful pipe call returned two distinct owned file descriptors.
        let reader = unsafe { File::from_raw_fd(fds[0]) };
        let writer = unsafe { File::from_raw_fd(fds[1]) };
        drop(writer);
        assert!(wait(
            diagnostics::Context::new("early", "hello-write"),
            reader.as_raw_fd(),
            libc::POLLOUT,
            Instant::now() + Duration::from_secs(1)
        )
        .is_err());
        let (host, mut guest) = UnixStream::pair().unwrap();
        drop(host);
        let b = boot();
        let request = WireRequest {
            version: VERSION,
            boot_nonce: b.boot_nonce,
            request_id: 1,
            body: RequestBody::Boot(b),
        };
        assert_eq!(
            reject_boot(
                "serve",
                &mut guest,
                &request,
                &io::Error::from_raw_os_error(13)
            )
            .unwrap_err()
            .raw_os_error(),
            Some(32)
        );
    }
    #[test]
    fn incompatible_reply_is_still_delivered_after_diagnosis() {
        let (mut host, mut guest) = UnixStream::pair().unwrap();
        let b = boot();
        let request = WireRequest {
            version: VERSION,
            boot_nonce: b.boot_nonce,
            request_id: 1,
            body: RequestBody::Boot(b.clone()),
        };
        assert!(reject_boot(
            "early",
            &mut guest,
            &request,
            &io::Error::from_raw_os_error(13)
        )
        .is_err());
        drop(guest);
        let mut bytes = Vec::new();
        host.read_to_end(&mut bytes).unwrap();
        let Frame::Result(reply) = serde_json::from_slice(&bytes).unwrap() else {
            panic!("expected result");
        };
        assert_eq!(reply.boot_nonce, b.boot_nonce);
        assert_eq!(reply.request_id, 1);
        assert_eq!(
            reply.body,
            ResultBody::Error {
                code: ErrorCode::IncompatibleState
            }
        );
    }
    #[test]
    fn real_io_failures_have_bounded_specific_safe_context() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::diagnostic_io_child", "--nocapture"])
            .env("BRIDGE_TEST_DIAGNOSTIC_IO", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(stderr.lines().collect::<Vec<_>>(), [
            "bridge-guest: stage=early phase=boot-read.eof kind=UnexpectedEof errno=none",
            "bridge-guest: stage=serve phase=hello-write.write-error kind=BrokenPipe errno=32",
            "bridge-guest: stage=early phase=hello-write.poll-hup kind=Other errno=none",
            "bridge-guest: stage=serve phase=boot-apply-or-ready kind=PermissionDenied errno=13",
            "bridge-guest: stage=serve phase=incompatible-reply-write.write-error kind=BrokenPipe errno=32",
        ]);
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
        assert_eq!(read(CTX, &mut guest, &mut session).unwrap().request_id, 3);
        assert_eq!(read(CTX, &mut guest, &mut session).unwrap().request_id, 4);
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
        let got = read(CTX, &mut BufReader::new(guest), &mut Session::early()).unwrap();
        assert!(matches!(got.body,RequestBody::Boot(saved) if saved==b));
        sender.join().unwrap();
    }
    #[test]
    fn real_socket_output_is_accepted_by_host_decoder() {
        let (mut host, mut guest) = UnixStream::pair().unwrap();
        let b = boot();
        write(
            CTX,
            &mut guest,
            Frame::Hello(Session::early().hello().clone()),
        )
        .unwrap();
        write(
            CTX,
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
