use super::*;
use crate::runtime::{
    apple_input::tests::installation,
    journal::{Journal, StatePhase},
};

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    sync::Arc,
};
pub(crate) fn resource(script: &str) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir(d.path().join("native")).unwrap();
    let file = d.path().join("native/BridgeVM");
    fs::write(&file, format!("#!/usr/bin/python3\n{script}")).unwrap();
    fs::set_permissions(file, fs::Permissions::from_mode(0o700)).unwrap();
    d
}
pub(crate) const PEER: &str = r#"import os,sys,socket,json
a=sys.argv[1:]
assert a[::2]==['--config-fd','--supervisor-fd','--guest-fd','--ownership-fd']
assert len(set(a[1::2]))==4
assert 'GH_TOKEN' not in os.environ
c=json.load(os.fdopen(int(a[1])))
if c['createEFI']:
    with open(c['installationPath']+'/efi.vars','xb') as f: f.write(b'fake')
s=socket.socket(fileno=int(a[3]))
g=socket.socket(fileno=int(a[5]))
s.sendall(b'BRVM\x01')
while True:
    b=b''
    while len(b)<9:
        p=s.recv(9-len(b))
        if not p: sys.exit(0)
        b+=p
    s.sendall(b'K'+b[1:])
    if b[0]==83: sys.exit(0)
"#;
pub(crate) fn failing_peer(mode: &str, opcode: u8) -> String {
    PEER.replace("s.sendall(b'BRVM\\x01')", "import signal\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\nopen(c['installationPath']+'/pid','w').write(str(os.getpid()))\ns.sendall(b'BRVM\\x01')").replace(
        "s.sendall(b'K'+b[1:])",
        &format!(
            r#"
    if b[0]=={opcode}:
        if '{mode}'=='ack': s.sendall(b'X'+b[1:])
        if '{mode}'=='eof': s.close()
        while True: pass
    s.sendall(b'K'+b[1:])"#
        ),
    )
}
// Only the operation under test gets short budgets; native startup stays unchanged.
struct OperationBudgets(AppleBackend);
impl VmBackend for OperationBudgets {
    fn create_state(&mut self, spec: &LaunchSpec) -> Result<(), RuntimeError> {
        self.0.create_state(spec)
    }
    fn start(&mut self, spec: &LaunchSpec) -> Result<BackendHandle, RuntimeError> {
        let handle = self.0.start(spec)?;
        super::super::apple::failure_fixture::shorten(self.0.owner.as_mut().unwrap());
        Ok(handle)
    }
    fn stop(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        self.0.stop(handle)
    }
    fn pause(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        self.0.pause(handle)
    }
    fn resume(&mut self, handle: &BackendHandle) -> Result<(), RuntimeError> {
        self.0.resume(handle)
    }
}
#[test]
fn failed_operations_revoke_guest_and_retain_original_worker_lock() {
    use std::io::Write;
    for &opcode in b"PR" {
        for mode in ["ack", "eof", "hang"] {
            let (t, j, image) = installation();
            let resources = resource(&failing_peer(mode, opcode));
            let backend = OperationBudgets(AppleBackend::new(resources.path().to_path_buf()));
            let mut worker =
                crate::runtime::RuntimeWorker::new(Box::new(backend), j, image).unwrap();
            worker.start().unwrap();
            let slot = worker
                .lease()
                .unwrap()
                .handle
                .get::<AppleBackendHandle>()
                .unwrap()
                .guest
                .clone();
            assert!(if opcode == b'P' {
                worker.pause()
            } else {
                worker.resume()
            }
            .is_err());
            assert_eq!(
                slot.channel().write(b"forbidden").unwrap_err().kind(),
                std::io::ErrorKind::NotConnected
            );
            assert!(
                worker.stop().is_err(),
                "unobserved exit cannot become Stop success"
            );
            drop(worker);
            let pid = fs::read_to_string(t.path().join("install/pid"))
                .unwrap()
                .parse::<i32>()
                .unwrap();
            let mut status = 0;
            assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
            assert!(matches!(
                Journal::open(&t.path().join("install")),
                Err(RuntimeError::Busy)
            ));
        }
    }
}
#[test]
fn state_resize_preserves_journal_inode_and_rejects_symlink_or_wrong_size() {
    let (t, mut j, image) = installation();
    let spec = j.spec(Uuid::new_v4(), image);
    let before = fs::metadata(&spec.state_path).unwrap().ino();
    let mut backend = AppleBackend::new(t.path().to_path_buf());
    backend.create_state(&spec).unwrap();
    assert_eq!(fs::metadata(&spec.state_path).unwrap().ino(), before);
    assert_eq!(fs::metadata(&spec.state_path).unwrap().len(), STATE_BYTES);
    let mut wrong = spec.clone();
    wrong.state_bytes = 1;
    assert!(backend.create_state(&wrong).is_err());
    j.set_phase(StatePhase::Preparing).unwrap();
    j.finish_preparation().unwrap();
    let image = spec.image.clone();
    // Match RuntimeWorker cleanup: release derived ownership capabilities before
    // the Journal, whose final-owner drop explicitly unlocks across fork/exec.
    drop(backend);
    drop(spec);
    drop(wrong);
    drop(j);
    let mut reopened = Journal::open(&t.path().join("install")).unwrap();
    assert!(
        reopened.begin_boot(image).is_err(),
        "Prepared reopen cannot restore New"
    );
}
#[test]
fn nofollow_state_and_all_preparing_reopens_refuse_without_replacement() {
    let (t, j, image) = installation();
    let spec = j.spec(Uuid::new_v4(), image);
    fs::rename(&spec.state_path, spec.root.join("retained-state")).unwrap();
    std::os::unix::fs::symlink(spec.root.join("retained-state"), &spec.state_path).unwrap();
    let mut backend = AppleBackend::new(t.path().to_path_buf());
    assert!(backend.create_state(&spec).is_err());
    assert_eq!(
        fs::metadata(spec.root.join("retained-state"))
            .unwrap()
            .len(),
        0
    );
    drop(spec);
    drop(j);
    let (_t, mut j, image) = installation();
    let root = j.spec(Uuid::new_v4(), image).root;
    j.set_phase(StatePhase::Preparing).unwrap();
    drop(j);
    assert!(matches!(
        Journal::open(&root),
        Err(RuntimeError::RetainedState)
    ));
}
#[test]
fn first_start_consumes_efi_permission_same_owner_and_mismatched_handle_cannot_stop() {
    let (_t, j, image) = installation();
    let resources = resource(PEER);
    let spec = j.spec(Uuid::new_v4(), image);
    let mut backend = AppleBackend::new(resources.path().to_path_buf());
    backend.create_state(&spec).unwrap();
    assert!(backend.owner.is_none());
    let mut foreign = spec.clone();
    foreign.ownership = Arc::new(spec.ownership.try_clone().unwrap());
    assert!(backend.start(&foreign).is_err());
    let handle = backend.start(&spec).unwrap();
    assert!(backend.stop(&BackendHandle::new(())).is_err());
    backend.pause(&handle).unwrap();
    backend.resume(&handle).unwrap();
    backend.stop(&handle).unwrap();
    fs::remove_file(spec.root.join("efi.vars")).unwrap();
    assert!(
        backend.start(&spec).is_err(),
        "same instance must not recreate missing EFI"
    );
    assert!(backend.start(&foreign).is_err());
}
#[test]
fn failure_never_rebuilds_owner_or_overwrites_partial_efi() {
    let (_t, j, image) = installation();
    let spec = j.spec(Uuid::new_v4(), image);
    let resources=resource("import os,sys,socket\na=sys.argv[1:]\ns=socket.socket(fileno=int(a[3])); s.sendall(b'BAD!!')\n");
    let mut backend = AppleBackend::new(resources.path().to_path_buf());
    backend.create_state(&spec).unwrap();
    assert!(backend.start(&spec).is_err());
    assert!(backend.owner.is_some());
    assert!(backend.start(&spec).is_err());
    fs::write(spec.root.join("efi.vars"), b"partial").unwrap();
    assert!(backend.start(&spec).is_err());
    assert_eq!(fs::read(spec.root.join("efi.vars")).unwrap(), b"partial");
}
#[test]
fn mutable_private_os_passes_worker_but_immutable_corruption_prevents_backend_effects() {
    for corrupt in [false, true] {
        let (_t, j, image) = installation();
        let spec = j.spec(Uuid::new_v4(), image.clone());
        fs::write(spec.root.join(image.disk_path()), b"guest mutated OS").unwrap();
        let resources = resource(PEER);
        let mut worker = crate::runtime::RuntimeWorker::new(
            Box::new(AppleBackend::new(resources.path().to_path_buf())),
            j,
            image.clone(),
        )
        .unwrap();
        if corrupt {
            fs::write(spec.root.join(image.kernel_path()), b"corruption").unwrap();
        }
        assert_eq!(worker.start().is_ok(), !corrupt);
        if !corrupt {
            worker.stop().unwrap();
        }
    }
}
