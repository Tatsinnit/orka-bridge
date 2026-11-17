use super::*;
use crate::engine::run::Verdict;
use serde_json::json;
use std::{collections::HashSet, time::Duration};

#[derive(Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cancellation {
    #[default]
    None,
    Cancelling,
    Cancelled,
    Unconfirmed,
    BeforeStart,
}
#[derive(Clone)]
pub(super) struct Recovery {
    pub document: DocumentRef,
    pub provider: String,
    pub installation: Uuid,
    pub boot: Uuid,
    pub handle: RunHandle,
    pub pending: Option<PendingRef>,
}
struct Proof {
    confirmed: bool,
    retry: bool,
    report: Option<RunReport>,
}
fn retain_answers(
    handle: &RunHandle,
    current: &mut Option<RunReport>,
    previous: Option<RunReport>,
) {
    if let Some(previous) = previous {
        if let Some(current) = current {
            handle.retain_evidence(current, previous);
        } else {
            *current = Some(previous);
        }
    }
}
fn terminal(task: &Value) -> bool {
    matches!(
        task["status"]["phase"].as_str(),
        Some("Succeeded" | "Failed" | "Cancelled")
    )
}
/// A POST is attempted once per explicit request, never once per polling round.
async fn prove(
    runner: &NativeRunner<'_>,
    recovery: &Recovery,
    retry: bool,
    deadline: Instant,
) -> Proof {
    let transport = runner.transport();
    let mut attempted = HashSet::new();
    let mut report: Option<RunReport> = None;
    loop {
        let mut failed = recovery.pending.is_some();
        let mut complete = true;
        let mut can_retry = false;
        for task in recovery.handle.tasks() {
            let path = task.path();
            let mut task_running = false;
            let checked = async {
                let mut value = transport.request(&path, None, None, deadline).await?;
                task.unchanged(&value)?;
                task_running = !terminal(&value);
                if task_running && retry && attempted.insert(task.name.clone()) {
                    let cancel = format!("{path}/cancel");
                    let body = json!({"uid":task.uid});
                    let result = transport
                        .request(&cancel, None, Some(&body), deadline)
                        .await;
                    // A conflict means finished-first only when this checked read
                    // proves it, never merely because the HTTP code was 409.
                    task_running = false;
                    value = transport.request(&path, None, None, deadline).await?;
                    task.unchanged(&value)?;
                    task_running = !terminal(&value);
                    match result {
                        Ok(_) => {}
                        Err(ErrorKind::Conflict) if terminal(&value) => {}
                        Err(kind) => return Err(kind),
                    }
                }
                if !terminal(&value) {
                    return Ok(false);
                }
                let worker = transport
                    .run_worker(&task.name, &task.uid, deadline)
                    .await?;
                Ok(worker.finished && worker.pods == 0)
            }
            .await;
            can_retry |= task_running;
            match checked {
                Ok(true) => {}
                Ok(false) => complete = false,
                Err(_) => {
                    failed = true;
                    complete = false;
                }
            }
        }
        // Save answers before waiting for a worker's grace period. A later
        // deadline or read failure must not erase already-read evidence.
        let (current, result) = runner.observe_finished(&recovery.handle, deadline).await;
        let mut current = Some(current);
        retain_answers(&recovery.handle, &mut current, report.take());
        report = current;
        failed |= result.is_err();
        if failed || complete || Instant::now() >= deadline {
            return Proof {
                confirmed: complete && !failed && Instant::now() < deadline,
                retry: can_retry,
                report,
            };
        }
        if transport.pause(deadline).await.is_err() {
            return Proof {
                confirmed: false,
                retry: can_retry,
                report,
            };
        }
    }
}
impl ExampleRun {
    pub(super) async fn finish_cancellation(
        &self,
        id: DocumentId,
        runner: &NativeRunner<'_>,
        recovery: Recovery,
        retry: bool,
    ) {
        {
            let mut state = self.0.lock().unwrap();
            let snapshot = state.snapshots.get_mut(&id).unwrap();
            snapshot.cancellation = Cancellation::Cancelling;
            snapshot.stage = Some("cancel");
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        let proof = prove(runner, &recovery, retry, deadline).await;
        let confirmed = proof.confirmed;
        let mut report = proof.report;
        retain_answers(
            &recovery.handle,
            &mut report,
            self.0.lock().unwrap().snapshots[&id].report.clone(),
        );
        if let Some(report) = &mut report {
            report.verdict = if confirmed {
                Verdict::Cancelled
            } else {
                Verdict::Unknown
            };
        }
        // History accepts a report only with Complete, including a partial report.
        let phase = if report.is_some() {
            RunState::Complete
        } else {
            RunState::Unknown
        };
        self.record(
            id,
            recovery.installation,
            &recovery.handle,
            recovery.pending.clone(),
            report,
            phase,
        );
        let mut state = self.0.lock().unwrap();
        let snapshot = state.snapshots.get_mut(&id).unwrap();
        snapshot.state = phase;
        snapshot.error = None;
        snapshot.can_retry_cancellation = !confirmed && proof.retry;
        snapshot.cancellation = if confirmed {
            if recovery.handle.tasks().is_empty() {
                Cancellation::BeforeStart
            } else {
                Cancellation::Cancelled
            }
        } else {
            Cancellation::Unconfirmed
        };
        if confirmed {
            state.recovery.remove(&id);
        } else {
            state.recovery.insert(id, recovery);
        }
    }
}
