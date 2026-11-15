//! Deterministic owner race regression without production scheduling hooks.
use super::*;
use crate::engine::run::{RunFuture, RunLimits, SavedResource};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Barrier;
use tokio::time::Instant;

struct Control {
    posts: Arc<Mutex<Vec<Value>>>,
}
impl RunTransport for Control {
    fn namespace(&self) -> &str {
        "agents"
    }
    fn provider(&self) -> &str {
        ""
    }
    fn limits(&self) -> RunLimits {
        RunLimits::default()
    }
    fn request<'a>(
        &'a self,
        path: &'a str,
        _: Option<u64>,
        body: Option<&'a Value>,
        _: Instant,
    ) -> RunFuture<'a> {
        Box::pin(async move {
            assert!(body.is_none(), "control cannot create");
            if path.ends_with("/result") {
                return Ok(json!({"result":"answer"}));
            }
            if path.ends_with("/events") {
                return Ok(
                    json!({"namespace":"agents","streamType":"task","streamID":"task","afterSeq":0,"latestSeq":0,"events":[]}),
                );
            }
            let agent = path == "agents/agent";
            assert!(agent || path == "tasks/task");
            Ok(
                json!({"apiVersion":"core.orka.ai/v1alpha1","kind":if agent {"Agent"} else {"Task"},"metadata":{"name":if agent {"agent"} else {"task"},"namespace":"agents","uid":if agent {"agent-uid"} else {"task-uid"},"generation":1},"spec":{},"status":{"phase":if agent || self.posts.lock().unwrap().is_empty() {"Running"} else {"Cancelled"}}}),
            )
        })
    }
    fn cancel_task<'a>(&'a self, name: &'a str, uid: &'a str, _: Instant) -> RunFuture<'a> {
        Box::pin(async move {
            assert_eq!((name, uid), ("task", "task-uid"));
            self.posts
                .lock()
                .unwrap()
                .push(json!({"method":"POST","path":"tasks/task/cancel","body":{"uid":uid}}));
            Ok(json!({"name":name,"namespace":"agents","requested":true,"phase":"Cancelled"}))
        })
    }
}

#[tokio::test]
async fn late_intent_after_failed_drive_is_serviced_before_atomic_finalization() {
    let owner = SendOwner::new(Arc::new(AgentFolder::new(None)), ClusterSession::default());
    let id = SendId(Uuid::new_v4());
    let document = owner.folder.document_id();
    let (stop, _) = watch::channel(false);
    let (_, done) = watch::channel(false);
    {
        let mut inner = owner.inner.lock().unwrap();
        inner.current.insert(document, id);
        inner.active = Some(Active {
            document: owner.folder.document(),
            id,
            stop,
            done,
            cancel_requested: false,
        });
        inner.views.insert(
            id,
            SendView {
                document_id: document,
                id,
                target_id: String::new(),
                target_name: None,
                provider_id: String::new(),
                source: None,
                model: None,
                agent_name: Some("agent".into()),
                agent_action: None,
                state: SendState::Running,
                stage: Stage::Observe,
                acknowledged: vec![Receipt {
                    kind: "Task",
                    name: "task".into(),
                    uid: "task-uid".into(),
                }],
                pending: None,
                report: None,
                error: None,
                storage_error: false,
                cancellation: None,
            },
        );
    }
    let barrier = Arc::new(Barrier::new(2));
    let worker_owner = owner.clone();
    let worker_barrier = barrier.clone();
    let worker = std::thread::spawn(move || {
        // This is the old unlocked snapshot. Pause at exactly the vulnerable boundary.
        assert_eq!(
            worker_owner
                .status(Some(&id.0.to_string()))
                .unwrap()
                .cancellation,
            None
        );
        worker_barrier.wait();
        worker_barrier.wait();
        let mut failed_drive = Some(Err(Stage::Observe));
        let finalized = worker_owner
            .inner
            .lock()
            .unwrap()
            .finish(id, &mut failed_drive, false);
        (finalized, failed_drive)
    });
    barrier.wait();
    let accepted = owner.cancel_document(document, &id.0.to_string()).unwrap();
    assert_eq!(
        accepted.view.unwrap().cancellation,
        Some(Cancellation::Cancelling)
    );
    barrier.wait();
    let (finalized, mut result) = worker.join().unwrap();
    assert!(
        !finalized,
        "an accepted late intent must retain admission and run control"
    );
    assert!(owner.inner.lock().unwrap().active.is_some());
    assert!(
        matches!(result, Some(Err(Stage::Observe))),
        "retain the failed drive result until control finishes"
    );

    let posts = Arc::new(Mutex::new(Vec::new()));
    let runner = NativeRunner::with_transport(Control {
        posts: posts.clone(),
    });
    let digest = format!("{:x}", Sha256::digest(b"{}"));
    let mut resources: Vec<SavedResource> = serde_json::from_value(json!([
        {"kind":"Agent","name":"agent","namespace":"agents","uid":"agent-uid","generation":1,"specDigest":digest,"terminal":false},
        {"kind":"Task","name":"task","namespace":"agents","uid":"task-uid","generation":1,"specDigest":digest,"terminal":false}
    ])).unwrap();
    let cases: Vec<crate::engine::run::RunCase> = serde_json::from_value(json!([{"id":"one","input":"question","assertions":[{"id":"answer","type":"contains","value":"answer"}]}])).unwrap();
    let outcome = runner
        .cancel_saved(&mut resources, &cases, &mut |_| Ok(()))
        .await
        .unwrap();
    let report = runner
        .recover(&mut resources, &cases, None)
        .await
        .report
        .unwrap();
    assert_eq!(
        posts.lock().unwrap().as_slice(),
        [json!({"method":"POST","path":"tasks/task/cancel","body":{"uid":"task-uid"}})]
    );
    owner.update(id, |view| view.cancellation = Some(outcome));
    // Another click during cleanup must not relatch after the accepted intent was serviced.
    assert_eq!(
        owner
            .cancel_document(document, &id.0.to_string())
            .unwrap()
            .view
            .unwrap()
            .cancellation,
        Some(Cancellation::Confirmed)
    );
    result = Some(Ok(report));
    assert!(owner.inner.lock().unwrap().finish(id, &mut result, true));
    let view = owner.status(Some(&id.0.to_string())).unwrap();
    assert_eq!(view.cancellation, Some(Cancellation::Confirmed));
    assert_eq!(view.report.unwrap().verdict, Verdict::Cancelled);
    assert!(owner.inner.lock().unwrap().active.is_none());
}
