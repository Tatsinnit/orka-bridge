use super::{
    failed,
    run_worker::{job_label, optional_text},
    Orka, CLEANUP_IDENTITY, IDENTITY,
};
use bridge_runtime_protocol::*;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    io,
    time::{Duration, Instant},
};

const AS_CLEANUP: &str = "--as=system:serviceaccount:orka-system:bridge-native-run-cleanup";
const BUDGET: Duration = Duration::from_secs(5);

/// Refuse incomplete observations, including duplicate names or UIDs. kubectl's
/// unchunked list must not turn pagination or malformed objects into absence.
fn items<'a>(list: &'a Value, kind: &str, api: &str) -> io::Result<&'a [Value]> {
    if !((list["kind"] == "List" && list["apiVersion"] == "v1")
        || (list["kind"] == format!("{kind}List") && list["apiVersion"] == api))
    {
        return Err(failed());
    }
    let metadata = list["metadata"].as_object().ok_or_else(failed)?;
    if !optional_text(&list["metadata"], "continue")?.is_empty()
        || match metadata.get("remainingItemCount") {
            None | Some(Value::Null) => false,
            Some(n) => n.as_u64() != Some(0),
        }
    {
        return Err(failed());
    }
    let items = list["items"].as_array().ok_or_else(failed)?;
    let (mut names, mut uids) = (BTreeSet::new(), BTreeSet::new());
    for item in items {
        let name = item["metadata"]["name"].as_str().ok_or_else(failed)?;
        let uid = item["metadata"]["uid"].as_str().ok_or_else(failed)?;
        if item["kind"] != kind
            || item["apiVersion"] != api
            || item["metadata"]["namespace"] != ORKA_NAMESPACE
            || name.len() > 253
            // Resource names are DNS subdomains (253 bytes total), not label
            // values: Kubernetes does not impose a 63-byte component limit.
            || name.split('.').any(|part| {
                part.is_empty()
                    || part.starts_with('-')
                    || part.ends_with('-')
                    || !part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || uid.is_empty()
            || uid.len() > 64
            || !names.insert(name)
            || !uids.insert(uid)
        {
            return Err(failed());
        }
    }
    Ok(items)
}
fn generated_run(name: &str) -> bool {
    name.strip_prefix("bridge-run-").is_some_and(|id| {
        id.len() == 32
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
impl Orka {
    /// Best effort, never a generic Run DELETE. No bearer or port-forward is
    /// needed; a separate least-privilege identity leaves retained Run roles intact.
    pub fn cleanup_run_agent(&self, uid: &str) -> io::Result<EngineResult> {
        let deadline = Instant::now() + BUDGET;
        if self.identity != IDENTITY
            || validate_action(&EngineActionWire::CleanupRunAgent { uid: uid.into() }).is_err()
        {
            return Err(failed());
        }
        let mut cleanup = Self {
            kubectl: self.kubectl.clone(),
            context: self.context.clone(),
            identity: CLEANUP_IDENTITY,
            rbac: false,
            token: None,
            previous_token: None,
            forward: None,
            client: None,
        };
        cleanup.identity(deadline)?;
        let removed = cleanup.cleanup_uid(uid, deadline)?;
        if Instant::now() >= deadline {
            return Err(failed());
        }
        Ok(EngineResult {
            exit: 0,
            stdout: serde_json::to_vec(&CleanupRunReply { removed })?,
            has_diagnostics: false,
        })
    }
    fn cleanup_list(&self, resource: &str, deadline: Instant) -> io::Result<Value> {
        let raw = self.run(
            &["get", resource, "-o", "json", "--chunk-size=0", AS_CLEANUP],
            None,
            deadline,
        )?;
        serde_json::from_slice(&raw).map_err(|_| failed())
    }
    fn cleanup_agent_name(&self, uid: &str, deadline: Instant) -> io::Result<Option<String>> {
        let list = self.cleanup_list("agents.core.orka.ai", deadline)?;
        let agents = items(&list, "Agent", "core.orka.ai/v1alpha1")?;
        let Some(agent) = agents.iter().find(|a| a["metadata"]["uid"] == uid) else {
            return Ok(None);
        };
        let name = agent["metadata"]["name"].as_str().ok_or_else(failed)?;
        if !generated_run(name) {
            return Err(failed());
        }
        Ok(Some(name.into()))
    }
    fn cleanup_uid(&self, uid: &str, deadline: Instant) -> io::Result<bool> {
        let Some(name) = self.cleanup_agent_name(uid, deadline)? else {
            return Ok(true);
        };
        let list = self.cleanup_list("tasks.core.orka.ai", deadline)?;
        let mut jobs = BTreeSet::new();
        for task in items(&list, "Task", "core.orka.ai/v1alpha1")? {
            let spec = task["spec"].as_object().ok_or_else(failed)?;
            let Some(reference) = spec.get("agentRef").filter(|r| !r.is_null()) else {
                // Non-AI Tasks need no Agent. An invalid AI reference cannot
                // establish non-use of the candidate.
                if spec.get("type").and_then(Value::as_str) == Some("ai") {
                    return Err(failed());
                }
                continue;
            };
            let target = reference["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(failed)?;
            let namespace = optional_text(reference, "namespace")?;
            let namespace = if namespace.is_empty() {
                ORKA_NAMESPACE
            } else {
                namespace
            };
            if target != name || namespace != ORKA_NAMESPACE {
                continue;
            }
            let status = &task["status"];
            if !status.is_null() && !status.is_object() {
                return Err(failed());
            }
            if !matches!(
                optional_text(status, "phase")?,
                "Succeeded" | "Failed" | "Cancelled"
            ) {
                return Ok(false);
            }
            let job = optional_text(status, "jobName")?;
            // Even a terminal Task may still have a worker. Missing association
            // is not proof of inactivity (unlike the observation-only RunWorker).
            if job.is_empty() {
                return Ok(false);
            }
            if !job_label(job) {
                return Err(failed());
            }
            jobs.insert(job);
        }
        for job in jobs {
            let selector = format!("--selector=job-name={job}");
            let raw = self.run(
                &[
                    "get",
                    "pods",
                    &selector,
                    "-o",
                    "json",
                    "--chunk-size=0",
                    AS_CLEANUP,
                ],
                None,
                deadline,
            )?;
            let list: Value = serde_json::from_slice(&raw).map_err(|_| failed())?;
            for pod in items(&list, "Pod", "v1")? {
                if !matches!(
                    pod["status"]["phase"].as_str(),
                    Some("Succeeded" | "Failed")
                ) {
                    return Ok(false);
                }
            }
        }
        let path = format!("/apis/core.orka.ai/v1alpha1/namespaces/{ORKA_NAMESPACE}/agents/{name}");
        let options = serde_json::to_vec(&json!({"apiVersion":"v1","kind":"DeleteOptions",
            "preconditions":{"uid":uid},"propagationPolicy":"Orphan"}))?;
        // -f - is raw DELETE's request body, not a resource manifest. A name
        // reused after the observation is protected by the server's UID fence.
        self.run(
            &["delete", "--raw", &path, "-f", "-", AS_CLEANUP],
            Some(&options),
            deadline,
        )?;
        // A DELETE acknowledgement is not absence (e.g. finalizers). Observe
        // once, bounded by the same budget; never replay an uncertain mutation.
        Ok(self.cleanup_agent_name(uid, deadline)?.is_none())
    }
}
