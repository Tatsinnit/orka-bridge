use bridge_guest::{
    early::{apply, BootSystem, Disk, Identity},
    peer::authorize,
    service::Session,
};
use bridge_runtime_protocol::*;
use std::io;

fn descriptor(mode: StateCreation) -> BootDescriptor {
    BootDescriptor {
        installation_id: Uuid::from_u128(42),
        generation: 1,
        boot_nonce: Uuid::from_u128(99),
        state_creation: mode,
        state_fs_uuid: Uuid::from_u128(42),
        state_bytes: STATE_BYTES,
        host_utc: 1_800_000_000,
    }
}
#[derive(Default)]
struct System {
    events: Vec<&'static str>,
    disk: Option<Disk>,
    identity: Option<Identity>,
    creds: bool,
    fail_tmpfs: bool,
}
impl BootSystem for System {
    fn probe(&mut self) -> io::Result<Disk> {
        self.events.push("probe");
        self.disk.clone().ok_or(io::Error::other("missing"))
    }
    fn format(&mut self, id: Uuid) -> io::Result<()> {
        self.events.push("mkfs-uuid");
        self.disk = Some(Disk {
            bytes: STATE_BYTES,
            fs_uuid: Some(id),
            blank: false,
        });
        Ok(())
    }
    fn mount_state(&mut self) -> io::Result<()> {
        self.events.push("mount");
        Ok(())
    }
    fn identity(&mut self) -> io::Result<Option<Identity>> {
        Ok(self.identity.clone())
    }
    fn initialize(&mut self, id: &Identity) -> io::Result<()> {
        self.events.push("identity");
        self.identity = Some(id.clone());
        Ok(())
    }
    fn bind_persistent(&mut self) -> io::Result<()> {
        self.events.push("bind-machine-daemons");
        Ok(())
    }
    fn credentials(&mut self) -> io::Result<()> {
        self.events.push("tmpfs");
        if self.fail_tmpfs {
            return Err(io::Error::other("tmpfs"));
        }
        self.creds = true;
        Ok(())
    }
    fn ready(&mut self, b: &BootDescriptor) -> io::Result<()> {
        self.events.push("guard");
        if self.creds
            && self
                .identity
                .as_ref()
                .is_some_and(|i| i.installation_id == b.installation_id)
        {
            Ok(())
        } else {
            Err(io::Error::other("guard"))
        }
    }
    fn shutdown(&mut self) -> io::Result<()> {
        self.events.push("shutdown");
        Ok(())
    }
    fn save_boot(&mut self, _: &BootDescriptor) -> io::Result<()> {
        self.events.push("accepted");
        Ok(())
    }
}
fn fresh() -> System {
    System {
        disk: Some(Disk {
            bytes: STATE_BYTES,
            fs_uuid: None,
            blank: true,
        }),
        ..System::default()
    }
}
fn existing() -> System {
    System {
        disk: Some(Disk {
            bytes: STATE_BYTES,
            fs_uuid: Some(Uuid::from_u128(42)),
            blank: false,
        }),
        identity: Some(Identity::new(Uuid::from_u128(42))),
        ..System::default()
    }
}
#[test]
fn new_disk_uuid_before_identity_journal() {
    let mut s = fresh();
    apply(&mut s, &descriptor(StateCreation::New)).unwrap();
    assert_eq!(
        s.events,
        [
            "probe",
            "mkfs-uuid",
            "probe",
            "mount",
            "identity",
            "bind-machine-daemons",
            "tmpfs",
            "guard",
            "accepted"
        ]
    );
}
#[test]
fn formatted_without_journal_no_reformat() {
    for mode in [StateCreation::New, StateCreation::Existing] {
        let mut s = existing();
        s.identity = None;
        assert!(apply(&mut s, &descriptor(mode)).is_err());
        assert!(!s.events.contains(&"mkfs-uuid"));
        assert!(!s.events.contains(&"accepted"));
    }
}
#[test]
fn missing_state_no_docker() {
    let mut s = System::default();
    assert!(apply(&mut s, &descriptor(StateCreation::New)).is_err());
    assert_eq!(s.events, ["probe"]);
}
#[test]
fn wrong_uuid_retained() {
    let mut s = existing();
    s.disk.as_mut().unwrap().fs_uuid = Some(Uuid::from_u128(43));
    assert!(apply(&mut s, &descriptor(StateCreation::Existing)).is_err());
    assert_eq!(s.events, ["probe"]);
}
#[test]
fn credentials_tmpfs_before_docker_restart() {
    let mut s = existing();
    apply(&mut s, &descriptor(StateCreation::Existing)).unwrap();
    assert!(
        s.events.iter().position(|x| *x == "tmpfs")
            < s.events.iter().position(|x| *x == "accepted")
    );
    s.fail_tmpfs = true;
    s.events.clear();
    assert!(apply(&mut s, &descriptor(StateCreation::Existing)).is_err());
    assert!(!s.events.contains(&"accepted"));
    assert!(!s.events.contains(&"mkfs-uuid"));
}
#[test]
fn foreign_signature_capacity_and_identity_refuse_without_format() {
    for mut s in [fresh(), existing(), fresh()] {
        s.disk.as_mut().unwrap().bytes = 1;
        assert!(apply(&mut s, &descriptor(StateCreation::New)).is_err());
        assert_eq!(s.events, ["probe"]);
    }
    let mut s = fresh();
    s.disk.as_mut().unwrap().blank = false;
    assert!(apply(&mut s, &descriptor(StateCreation::New)).is_err());
    assert_eq!(s.events, ["probe"]);
    let mut s = existing();
    s.identity = Some(Identity::new(Uuid::from_u128(44)));
    assert!(apply(&mut s, &descriptor(StateCreation::Existing)).is_err());
    assert!(!s.events.contains(&"accepted"));
}
fn request(b: BootDescriptor, id: u64) -> Frame {
    Frame::Request(WireRequest {
        version: VERSION,
        boot_nonce: b.boot_nonce,
        request_id: id,
        body: RequestBody::Boot(b),
    })
}
#[test]
fn serve_without_accepted_nonce() {
    assert!(Session::serve(None).is_err());
    let mut b = descriptor(StateCreation::New);
    b.boot_nonce = Uuid::nil();
    assert!(Session::serve(Some(b)).is_err());
}
#[test]
fn early_serve_partial_frame() {
    let b = descriptor(StateCreation::New);
    let bytes = encode(&request(b.clone(), 1)).unwrap();
    let mut early = Session::early();
    assert_eq!(early.hello().accepted_boot_nonce, None);
    for byte in &bytes[..10] {
        assert!(early.push(*byte).unwrap().is_none());
    }
    let mut serve = Session::serve(Some(b.clone())).unwrap();
    assert_eq!(serve.hello().accepted_boot_nonce, Some(b.boot_nonce));
    assert_ne!(serve.hello().guest_nonce, early.hello().guest_nonce);
    let mut existing = b;
    existing.state_creation = StateCreation::Existing;
    let bytes = encode(&request(existing, 2)).unwrap();
    let mut result = None;
    for byte in bytes {
        result = serve.push(byte).unwrap().or(result);
    }
    assert!(result.is_some());
}
#[test]
fn wrong_nonce_stage_duplicate_and_guest_results_refused() {
    let b = descriptor(StateCreation::New);
    let mut s = Session::serve(Some(b.clone())).unwrap();
    let mut wrong = b.clone();
    wrong.boot_nonce = Uuid::from_u128(101);
    assert!(s.accept(request(wrong, 2)).is_err());
    let mut s = Session::serve(Some(b.clone())).unwrap();
    assert!(s.accept(request(b.clone(), 2)).is_err());
    let mut s = Session::early();
    assert!(s.accept(request(b.clone(), 2)).is_err());
    let mut s = Session::early();
    s.accept(request(b.clone(), 1)).unwrap();
    assert!(s.accept(request(b, 1)).is_err());
}
#[test]
fn readiness_shutdown_and_no_execution_operations() {
    let b = descriptor(StateCreation::Existing);
    let mut s = existing();
    s.creds = true;
    let mut r = WireRequest {
        version: VERSION,
        boot_nonce: b.boot_nonce,
        request_id: 3,
        body: RequestBody::Readiness {},
    };
    let (reply, stop) = bridge_guest::service::handle(&mut s, &b, &r);
    assert!(matches!(reply, ResultBody::Ready {}));
    assert!(!stop);
    s.events.clear();
    r.body = RequestBody::Engine(EngineActionWire::Version {});
    let (reply, stop) = bridge_guest::service::handle(&mut s, &b, &r);
    assert!(matches!(
        reply,
        ResultBody::Error {
            code: ErrorCode::Unavailable
        }
    ));
    assert!(!stop);
    assert!(s.events.is_empty());
    r.body = RequestBody::Shutdown {};
    let (reply, stop) = bridge_guest::service::handle(&mut s, &b, &r);
    assert!(matches!(reply, ResultBody::Ready {}));
    assert!(stop);
    assert_eq!(s.events, ["shutdown"]);
    s.creds = false;
    r.body = RequestBody::Readiness {};
    let (reply, stop) = bridge_guest::service::handle(&mut s, &b, &r);
    assert!(matches!(
        reply,
        ResultBody::Error {
            code: ErrorCode::IncompatibleState
        }
    ));
    assert!(!stop);
}
#[test]
fn guest_receiver_rejects_bad_utf8_unknown_fields_oversize_and_results() {
    for bytes in [vec![255, b'\n'], b"{\"extra\":true}\n".to_vec()] {
        let mut s = Session::early();
        assert!(bytes.into_iter().any(|b| s.push(b).is_err()));
    }
    let mut s = Session::early();
    for _ in 0..MAX_FRAME - 1 {
        s.push(b' ').unwrap();
    }
    assert!(s.push(b' ').is_err());
    assert!(s.push(b'\n').is_err());
    let b = descriptor(StateCreation::New);
    let mut s = Session::early();
    assert!(s
        .accept(Frame::Result(WireResult {
            version: VERSION,
            boot_nonce: b.boot_nonce,
            request_id: 1,
            body: ResultBody::Ready {}
        }))
        .is_err());
}
#[test]
fn unauthorized_socket_peer() {
    assert!(authorize(0).is_ok());
    assert!(authorize(1000).is_err());
}
