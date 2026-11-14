use super::{failed, Orka, IDENTITY, REQUEST_BUDGET};
use bridge_runtime_protocol::*;
use serde_json::Value;
use std::{io, time::Instant};

const AS_RUN: &str = "--as=system:serviceaccount:orka-system:bridge-native-run";

fn optional_text<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(s)) => Ok(s),
        _ => Err(failed()),
    }
}
fn job_label(name: &str) -> bool {
    let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !name.is_empty()
        && name.len() <= 63
        && edge(name.as_bytes()[0])
        && edge(name.as_bytes()[name.len() - 1])
        && name.bytes().all(|b| edge(b) || b == b'-')
}
impl Orka {
    /// A fixed, UID-fenced observation as Run, never an admin pod read.
    pub fn run_worker(&mut self, name: &str, uid: &str) -> io::Result<EngineResult> {
        if self.identity != IDENTITY || !valid_run_worker(name, uid) {
            return Err(failed());
        }
        let deadline = Instant::now() + REQUEST_BUDGET;
        if !self.rbac {
            self.identity(deadline)?;
        }
        let raw = self.run(
            &["get", "tasks.core.orka.ai", name, "-o", "json", AS_RUN],
            None,
            deadline,
        )?;
        let task: Value = serde_json::from_slice(&raw).map_err(|_| failed())?;
        if task["metadata"]["uid"].as_str() != Some(uid) {
            return Err(failed());
        }
        let status = &task["status"];
        if !status.is_null() && !status.is_object() {
            return Err(failed());
        }
        let finished = match optional_text(status, "phase")? {
            "Succeeded" | "Failed" | "Cancelled" => true,
            "" | "Pending" | "Running" | "Finalizing" | "Scheduled" => false,
            _ => return Err(failed()),
        };
        let job = optional_text(status, "jobName")?;
        let mut pods = 0;
        // A Task cancelled before scheduling has no Job. Pending retries may
        // also have no Job; zero pods alone never establishes completion.
        if !job.is_empty() {
            if !job_label(job) {
                return Err(failed());
            }
            let selector = format!("--selector=job-name={job}");
            let raw = self.run(
                &[
                    "get",
                    "pods",
                    &selector,
                    "-o",
                    "json",
                    "--chunk-size=0",
                    AS_RUN,
                ],
                None,
                deadline,
            )?;
            let list: Value = serde_json::from_slice(&raw).map_err(|_| failed())?;
            let metadata = list["metadata"].as_object().ok_or_else(failed)?;
            if !optional_text(&list["metadata"], "continue")?.is_empty()
                || match metadata.get("remainingItemCount") {
                    None | Some(Value::Null) => false,
                    Some(n) => n.as_u64() != Some(0),
                }
            {
                return Err(failed());
            }
            for pod in list["items"].as_array().ok_or_else(failed)? {
                if !pod.is_object() {
                    return Err(failed());
                }
                // Completed pods are retained for results. All other phases,
                // including absent/unknown phases and terminating pods, count.
                if !matches!(
                    pod["status"]["phase"].as_str(),
                    Some("Succeeded" | "Failed")
                ) {
                    pods += 1;
                }
            }
        }
        Ok(EngineResult {
            exit: 0,
            stdout: serde_json::to_vec(&RunWorkerReply { finished, pods })?,
            has_diagnostics: false,
        })
    }
}
