use super::*;
use crate::runtime::apple_channel::GuestSlot;
use crate::runtime::{
    apple_backend::{
        tests::{resource, PEER},
        AppleBackend,
    },
    apple_input::tests::installation,
    journal::Journal,
    RuntimeWorker,
};
use std::{
    io::Write,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};
#[path = "apple_session_extra_tests.rs"]
mod extra;
fn serving_peer(mode: &str) -> String {
    let extra = format!(
        r#"
import threading,time
def guest():
    f=g.makefile('rb',buffering=0)
    def send(frame): g.sendall(json.dumps(frame,separators=(',',':')).encode()+b'\n')
    send({{'type':'hello','body':{{'version':1,'stage':'early','guestNonce':'11111111-1111-4111-8111-111111111111','acceptedBootNonce':None}}}})
    b=json.loads(f.readline())
    assert b['body']['body']['op']=='boot' and b['body']['body']['body']['stateCreation']=='new'
    # Reading Boot proves the host consumed creation before this EOF.
    if '{mode}'=='peer-eof':
        f.close()
        g.close()
        return
    nonce=b['body']['bootNonce']
    ready={{'type':'result','body':{{'version':1,'bootNonce':nonce,'requestId':1,'body':{{'op':'ready','body':{{}}}}}}}}
    hello={{'type':'hello','body':{{'version':1,'stage':'serve','guestNonce':'22222222-2222-4222-8222-222222222222','acceptedBootNonce':nonce}}}}
    if '{mode}'=='cleanup': hello['body']['runCleanup']=True
    if '{mode}'=='bad': ready['body']['bootNonce']='33333333-3333-4333-8333-333333333333'
    # One write pins the no-read-ahead boundary between Ready and next Hello.
    g.sendall(json.dumps(ready).encode()+b'\n'+json.dumps(hello).encode()+b'\n')
    if '{mode}'=='bad': return
    b=json.loads(f.readline())
    assert b['body']['requestId']==2
    assert b['body']['body']['body']['stateCreation']=='existing'
    send({{'type':'result','body':{{'version':1,'bootNonce':nonce,'requestId':2,'body':{{'op':'ready','body':{{}}}}}}}})
    if '{mode}'=='engine':
        f.readline()
        open(c['installationPath']+'/request-seen','x').close()
    if '{mode}' in ['good','cleanup']:
        while True:
            line=f.readline()
            if not line: return
            request=json.loads(line)['body']
            body=request['body']
            if body['op']=='execute':
                assert '{mode}'=='cleanup'
                assert body['body']['action']['type']=='cleanupRunAgent'
                result={{'op':'error','body':{{'code':'invalidRequest'}}}}
            else:
                assert body['op']=='readiness'
                result={{'op':'infrastructure','body':{{'phase':'cluster-ready','service':'starting'}}}}
            send({{'type':'result','body':{{'version':1,'bootNonce':nonce,'requestId':request['requestId'],'body':result}}}})
threading.Thread(target=guest,daemon=True).start()
"#
    );
    PEER.replace("while True:", &format!("{extra}\nwhile True:"))
}
#[test]
fn coalesced_early_ready_and_serve_hello_publish_only_after_correlated_ready() {
    let (_t, journal, image) = installation();
    let resources = resource(&serving_peer("good"));
    let mut session =
        AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
    assert!(session.transport().is_some());
    session.pause().unwrap();
    session.resume().unwrap();
    session.stop().unwrap();
    assert!(session.transport().is_none());
}
#[test]
fn apple_serving_hello_gates_cleanup_without_probing_legacy_guests() {
    for mode in ["good", "cleanup"] {
        let (_t, journal, image) = installation();
        let resources = resource(&serving_peer(mode));
        let mut session =
            AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
        session
            .transport()
            .unwrap()
            .borrow_client(|client| {
                let error = client.cleanup_run_agent("old-uid").unwrap_err();
                assert_eq!(
                    error.kind() == std::io::ErrorKind::Unsupported,
                    mode == "good"
                );
                assert!(client.readiness().is_ok());
            })
            .unwrap();
        session.stop().unwrap();
    }
}
#[test]
fn failed_public_pause_or_resume_never_republishes_transport() {
    for &opcode in b"PR" {
        for mode in ["ack", "eof"] {
            let (t, journal, image) = installation();
            let peer = serving_peer("good").replace(
                "s.sendall(b'K'+b[1:])",
                &format!("if b[0]=={opcode}:\n        {}\n        sys.exit(7)\n    s.sendall(b'K'+b[1:])",
                    if mode == "ack" { "s.sendall(b'X'+b[1:])" } else { "s.close()" }),
            );
            let resources = resource(&peer);
            let mut session =
                AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).unwrap();
            assert!(if opcode == b'P' {
                session.pause()
            } else {
                session.resume()
            }
            .is_err());
            assert!(session.transport().is_none());
            assert!(session.pause().is_err());
            assert!(session.resume().is_err());
            assert!(session.transport().is_none());
            assert!(session.stop().is_err());
            drop(session);
            assert!(matches!(
                Journal::open(&t.path().join("install")),
                Err(RuntimeError::Busy)
            ));
        }
    }
}
#[test]
fn wrong_ready_nonce_cleans_up_without_replay_and_keeps_consumed_record() {
    let (t, journal, image) = installation();
    let resources = resource(&serving_peer("bad"));
    assert!(AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).is_err());
    let reopened = Journal::open(&t.path().join("install")).unwrap();
    assert_eq!(
        reopened.record().phase,
        crate::runtime::journal::StatePhase::Consumed
    );
}
#[test]
fn peer_eof_after_early_boot_observes_stop_and_retains_consumption() {
    let (t, journal, image) = installation();
    let resources = resource(&serving_peer("peer-eof"));
    assert!(AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).is_err());
    assert_eq!(
        Journal::open(&t.path().join("install"))
            .unwrap()
            .record()
            .phase,
        crate::runtime::journal::StatePhase::Consumed
    );
}
#[test]
fn failed_cleanup_quarantines_original_journal_lock() {
    let (t, journal, image) = installation();
    let resources = resource(
        &serving_peer("bad").replace("if b[0]==83: sys.exit(0)", "if b[0]==83: sys.exit(7)"),
    );
    assert!(AppleVmSession::launch(resources.path().to_path_buf(), journal, image, 1).is_err());
    assert!(matches!(
        Journal::open(&t.path().join("install")),
        Err(crate::runtime::types::RuntimeError::Busy)
    ));
}
#[test]
fn early_send_failure_after_boot_request_is_not_restored_to_new() {
    let (t, journal, image) = installation();
    let resources = resource(PEER);
    let mut worker = RuntimeWorker::new(
        Box::new(AppleBackend::new(resources.path().to_path_buf())),
        journal,
        image,
    )
    .unwrap();
    worker.start().unwrap();
    let hello = StageHello {
        version: VERSION,
        stage: Stage::Early,
        guest_nonce: Uuid::new_v4(),
        run_cleanup: false,
        accepted_boot_nonce: None,
    };
    let request = worker.boot_request(&hello, 1).unwrap();
    assert!(matches!(
        request.body,
        RequestBody::Boot(BootDescriptor {
            state_creation: StateCreation::New,
            ..
        })
    ));
    let slot = &worker
        .lease()
        .unwrap()
        .handle
        .get::<crate::runtime::apple_backend::AppleBackendHandle>()
        .unwrap()
        .guest;
    slot.seal();
    slot.revoke(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(slot
        .channel()
        .write_all(&encode(&Frame::Request(request)).unwrap())
        .is_err());
    worker.stop().unwrap();
    drop(worker);
    assert_eq!(
        Journal::open(&t.path().join("install"))
            .unwrap()
            .record()
            .phase,
        crate::runtime::journal::StatePhase::Consumed
    );
}
#[test]
fn bounded_one_byte_frame_reader_refuses_oversize_eof_and_drip_without_renewal() {
    for data in [vec![b'x'; MAX_FRAME + 1], b"{\n".to_vec(), vec![]] {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let slot = GuestSlot::new(socket);
        let mut channel = slot.channel();
        let writer = std::thread::spawn(move || {
            let _ = peer.write_all(&data);
        });
        assert!(read_frame(&mut channel).is_err());
        slot.revoke(Instant::now() + Duration::from_secs(1))
            .unwrap();
        writer.join().unwrap();
    }
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let slot = GuestSlot::new(socket);
    let mut channel = slot.channel();
    channel.deadline = Some(Instant::now() + Duration::from_millis(60));
    let writer = std::thread::spawn(move || {
        for _ in 0..20 {
            if peer.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    });
    assert!(
        matches!(read_frame(&mut channel),Err(crate::runtime::types::RuntimeError::Io(e)) if e.kind()==std::io::ErrorKind::TimedOut)
    );
    slot.revoke(Instant::now() + Duration::from_secs(1))
        .unwrap();
    writer.join().unwrap();
}
