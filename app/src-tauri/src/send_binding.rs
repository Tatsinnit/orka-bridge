//! Ownership is independent of evictable answers. Never infer it from history.
use super::*;
use crate::{
    agent_folder::{invalid, read, replace},
    engine::run::ResourceId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const FILE: &str = "bindings.json";
const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    pub document: DocumentId,
    pub target: TargetUids,
    pub current: bool,
    pub name: String,
    /// Exact last desired spec, not the server's default-expanded acknowledgement.
    pub desired: Value,
    pub acknowledged: Option<Value>,
    /// Persisted before attempt details and before create/update; never replayed.
    pub intent: Option<Value>,
    pub attempt: String,
    pub unfinished: bool,
}
impl Binding {
    pub fn action(&self, desired: &Value) -> SendAction {
        if self.desired == *desired {
            SendAction::Unchanged
        } else {
            SendAction::Update
        }
    }
    fn identity(&self, value: &Value, desired: &Value) -> Result<ResourceId, Stage> {
        ResourceId::acknowledge(
            value,
            desired,
            &self.name,
            &self.target.watched_namespace,
            "Agent",
        )
        .map_err(|_| Stage::UpdateUnconfirmed)
    }
    pub fn receipt(&self) -> Result<ResourceId, Stage> {
        let value = self.acknowledged.as_ref().ok_or(Stage::UpdateUnconfirmed)?;
        self.identity(value, &value["spec"])
    }
    pub fn acknowledge(&mut self, value: Value) -> Result<(), Stage> {
        let desired = self.intent.as_ref().unwrap_or(&self.desired);
        let receipt = self.identity(&value, desired)?;
        if let Some(old) = &self.acknowledged {
            if value["metadata"]["uid"] != old["metadata"]["uid"]
                || receipt.generation < old["metadata"]["generation"].as_u64().unwrap_or(u64::MAX)
            {
                return Err(Stage::UpdateUnconfirmed);
            }
        }
        if let Some(desired) = self.intent.take() {
            self.desired = desired;
        }
        self.acknowledged = Some(receipt.value());
        // Agent acknowledgement (including intent reconciliation) precedes every
        // Task barrier. Persist this with the cleared intent, not in a later save:
        // missing/removed Agent-only history cannot strand an unfinished binding.
        self.unfinished = false;
        Ok(())
    }
    pub fn reconciled(&self, value: &Value) -> Result<ResourceId, Stage> {
        let desired = self.intent.as_ref().ok_or(Stage::UpdateUnconfirmed)?;
        let mut next = self.clone();
        next.acknowledge(value.clone())?;
        if let Some(old) = &self.acknowledged {
            // Keep known server defaults, but require every authored field change
            // (including removal) and reject any unrelated outside spec change.
            fn expected(current: &Value, before: &Value, after: &Value) -> Value {
                let (Some(current), Some(before), Some(after)) =
                    (current.as_object(), before.as_object(), after.as_object())
                else {
                    return after.clone();
                };
                let mut next = current.clone();
                for key in before.keys().filter(|key| !after.contains_key(*key)) {
                    next.remove(key);
                }
                for (key, value) in after {
                    next.insert(
                        key.clone(),
                        match (current.get(key), before.get(key)) {
                            (Some(current), Some(before)) => expected(current, before, value),
                            _ => value.clone(),
                        },
                    );
                }
                Value::Object(next)
            }
            if value["spec"] != expected(&old["spec"], &self.desired, desired) {
                return Err(Stage::UpdateUnconfirmed);
            }
            // Inclusion alone cannot confirm deletion of an optional field, or
            // distinguish a still-in-flight PUT from the old revision.
            if value["spec"] == old["spec"]
                && value["metadata"]["generation"] == old["metadata"]["generation"]
            {
                return Err(Stage::UpdateUnconfirmed);
            }
        }
        self.identity(value, desired)
    }
    pub fn repair_agent(&self, details: &mut AttemptDetails) -> Result<bool, Stage> {
        let old = details.resources.iter_mut().find(|r| r.kind == "Agent");
        if old.as_ref().is_some_and(|r| r.uid.is_some()) {
            return Ok(false); // An acknowledged archived revision is never replaced.
        }
        let saved = crate::engine::run::SavedResource::acknowledged(&self.receipt()?);
        if let Some(old) = old {
            *old = saved;
        } else {
            details.resources.insert(0, saved);
        }
        Ok(true)
    }
    fn validate(&self) -> Result<(), &'static str> {
        let text = |s: &str| !s.is_empty() && s.len() <= 4096 && !s.contains('\0');
        let dns = |s: &str| crate::engine::run::valid_cancel_identity(s, "uid");
        if !self.target.valid_identity()
            || !dns(&self.name)
            || !dns(&self.target.watched_namespace)
            || !dns(&self.target.service_namespace)
            || !dns(&self.target.deployment_namespace)
            || !text(&self.target.service_uid)
            || !text(&self.target.deployment_uid)
            || parse(&self.attempt).is_none()
            || !self.desired.is_object()
            || self.intent.as_ref().is_some_and(|s| !s.is_object())
            || self.acknowledged.is_none() && self.intent.is_none()
            || self
                .acknowledged
                .as_ref()
                .is_some_and(|v| self.identity(v, &self.desired).is_err())
        {
            return Err("storage");
        }
        Ok(())
    }
}
impl SummaryStore {
    fn bindings(&self) -> Result<Vec<Binding>, &'static str> {
        let Some(root) = self.root(false).map_err(|_| "storage")? else {
            return Ok(Vec::new());
        };
        Self::read_bindings(&root)
    }
    fn read_bindings(root: &cap_std::fs::Dir) -> Result<Vec<Binding>, &'static str> {
        match root.symlink_metadata(FILE) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err("storage"),
            Ok(_) => {
                let bindings: Vec<Binding> =
                    serde_json::from_slice(&read(root, FILE, MAX_BYTES).map_err(|_| "storage")?)
                        .map_err(|_| "storage")?;
                if bindings.len() > 256 {
                    return Err("storage");
                }
                for (i, b) in bindings.iter().enumerate() {
                    b.validate()?;
                    if bindings[..i].iter().any(|a| {
                        a.document == b.document
                            && a.target == b.target
                            && (a.name == b.name || a.current && b.current)
                    }) {
                        return Err("storage");
                    }
                }
                Ok(bindings)
            }
        }
    }
    // Publication already holds the store lock; read through its existing root.
    pub(crate) fn unfinished_attempts(
        root: &cap_std::fs::Dir,
    ) -> Result<std::collections::HashSet<String>, &'static str> {
        Ok(Self::read_bindings(root)?
            .into_iter()
            .filter(|b| b.unfinished)
            .map(|b| b.attempt)
            .collect())
    }
    pub(super) fn binding(
        &self,
        document: DocumentId,
        target: &TargetUids,
    ) -> Result<Option<Binding>, &'static str> {
        let _lock = self.lock.lock().map_err(|_| "storage")?;
        let bindings = self.bindings()?;
        // Missing destination evidence is never permission to adopt or duplicate ownership.
        if target.cluster_identity.is_none()
            || bindings
                .iter()
                .any(|b| b.current && b.document == document && b.target.legacy_matches(target))
        {
            return Err("legacyDestination");
        }
        Ok(bindings
            .into_iter()
            .find(|b| b.current && b.document == document && b.target.pinned_matches(target)))
    }
    pub(super) fn attempt_binding(&self, attempt: &str) -> Result<Option<Binding>, &'static str> {
        let _lock = self.lock.lock().map_err(|_| "storage")?;
        Ok(self.bindings()?.into_iter().find(|b| b.attempt == attempt))
    }
    /// A definitive create rejection cannot leave an owned or uncertain agent.
    pub(super) fn reject_creation(&self, rejected: &Binding) -> Result<(), &'static str> {
        let _lock = self.lock.lock().map_err(|_| "storage")?;
        let mut bindings = self.bindings()?;
        bindings.retain(|binding| {
            binding.document != rejected.document
                || binding.target != rejected.target
                || binding.name != rejected.name
                || binding.acknowledged.is_some()
        });
        if !bindings.iter().any(|binding| {
            binding.current
                && binding.document == rejected.document
                && binding.target == rejected.target
        }) {
            if let Some(previous) = bindings.iter_mut().rev().find(|binding| {
                binding.document == rejected.document && binding.target == rejected.target
            }) {
                previous.current = true;
            }
        }
        let root = self.root(true).map_err(|_| "storage")?.ok_or("storage")?;
        replace(
            &root,
            FILE,
            &serde_json::to_vec(&bindings).map_err(|_| "storage")?,
        )
        .map_err(|_| "storage")
    }
    /// Release only after terminal evidence is durable, or before removing it.
    pub(crate) fn complete_binding(
        &self,
        attempt: &str,
        details: &AttemptDetails,
    ) -> Result<(), &'static str> {
        if details.unresolved()
            || details.resources.is_empty()
            || details.resources.iter().any(|r| r.uid.is_none())
        {
            return Ok(());
        }
        if let Some(mut binding) = self.attempt_binding(attempt)? {
            if binding.unfinished {
                binding.unfinished = false;
                self.save_binding(binding)?;
            }
        }
        Ok(())
    }
    pub(super) fn save_binding(&self, binding: Binding) -> Result<(), &'static str> {
        let _lock = self.lock.lock().map_err(|_| "storage")?;
        binding.validate()?;
        let mut bindings = self.bindings()?;
        for old in &mut bindings {
            if binding.current
                && old.document == binding.document
                && old.target == binding.target
                && old.name != binding.name
            {
                old.current = false;
            }
        }
        if let Some(old) = bindings.iter_mut().find(|b| {
            b.document == binding.document && b.target == binding.target && b.name == binding.name
        }) {
            *old = binding;
        } else {
            bindings.push(binding);
        }
        let bytes = serde_json::to_vec(&bindings).map_err(|_| "storage")?;
        if bindings.len() > 256 || bytes.len() > MAX_BYTES {
            return Err("storage");
        }
        let root = self
            .root(true)
            .map_err(|_| "storage")?
            .ok_or_else(invalid)
            .map_err(|_| "storage")?;
        replace(&root, FILE, &bytes).map_err(|_| "storage")
    }
}
pub(super) fn target(binding: &crate::clusters::TargetBinding) -> TargetUids {
    TargetUids {
        cluster_identity: Some(binding.cluster_identity.clone()),
        service_namespace: binding.service_namespace.clone(),
        service_uid: binding.service_uid.clone(),
        deployment_namespace: binding.deployment_namespace.clone(),
        deployment_uid: binding.deployment_uid.clone(),
        watched_namespace: binding.watched_namespace.clone(),
    }
}
pub(super) fn lookup_stage(error: &str) -> Stage {
    if error == "legacyDestination" {
        Stage::LegacyDestination
    } else {
        Stage::Storage
    }
}
impl SendOwner {
    /// No mutation replay: only GET can resolve persisted Agent intent.
    pub(super) async fn deployment(
        &self,
        document: DocumentId,
        target: &TargetUids,
        transport: &dyn RunTransport,
    ) -> Result<Option<Binding>, Stage> {
        let Some(mut binding) = self
            .summary
            .binding(document, target)
            .map_err(lookup_stage)?
        else {
            return Ok(None);
        };
        let path = format!("agents/{}", binding.name);
        let current = transport
            .request(&path, None, None, transport.deadline())
            .await
            .map_err(|e| {
                if binding.intent.is_some() {
                    Stage::UpdateUnconfirmed
                } else {
                    match e {
                        ErrorKind::NotFound => Stage::UpdateMissing,
                        ErrorKind::Authentication | ErrorKind::Permission => Stage::UpdateDenied,
                        _ => Stage::UpdateUnconfirmed,
                    }
                }
            })?;
        let pending = binding.intent.is_some();
        if pending {
            let receipt = binding.reconciled(&current)?;
            binding.acknowledge(receipt.value())?;
            // Agent acknowledgement is durable before repairing attempt details.
            self.summary
                .save_binding(binding.clone())
                .map_err(|_| Stage::Storage)?;
        }
        binding
            .receipt()?
            .unchanged(&current)
            .map_err(|_| Stage::UpdateConflict)?;
        if let Ok((mut summary, Some(mut details))) = self.summary.record(&binding.attempt) {
            if binding.repair_agent(&mut details)? {
                summary.agent_uid = Some(binding.receipt()?.uid);
                self.summary
                    .checkpoint(summary, details.clone())
                    .map_err(|_| Stage::Storage)?;
                self.summary
                    .complete_binding(&binding.attempt, &details)
                    .map_err(|_| Stage::Storage)?;
            }
        }
        if binding.unfinished {
            if let Ok((_, Some(details))) = self.summary.record(&binding.attempt) {
                if !details.unresolved() && !details.resources.is_empty() {
                    binding.unfinished = false;
                    self.summary
                        .save_binding(binding.clone())
                        .map_err(|_| Stage::Storage)?;
                }
            }
            if binding.unfinished {
                return Err(Stage::EarlierWork);
            }
        }
        Ok(Some(binding))
    }
    pub(super) async fn update_deployment(
        &self,
        binding: &mut Binding,
        transport: &dyn RunTransport,
        desired: Value,
    ) -> Result<ResourceId, Stage> {
        let old = binding.receipt()?;
        binding.intent = Some(desired.clone());
        self.summary
            .save_binding(binding.clone())
            .map_err(|_| Stage::Storage)?;
        let body =
            json!({"expectedUID":old.uid,"expectedGeneration":old.generation,"spec":desired});
        let result = transport
            .update_agent(&binding.name, &body, transport.deadline())
            .await;
        let stage = match result {
            Ok(value) => {
                binding.acknowledge(value)?;
                self.summary
                    .save_binding(binding.clone())
                    .map_err(|_| Stage::Storage)?;
                return binding.receipt();
            }
            Err(ErrorKind::NotFound) => match transport
                .request(
                    &format!("agents/{}", binding.name),
                    None,
                    None,
                    transport.deadline(),
                )
                .await
            {
                Ok(_) => Stage::UpdateUnsupported,
                Err(ErrorKind::NotFound) => Stage::UpdateMissing,
                _ => Stage::UpdateUnconfirmed,
            },
            Err(ErrorKind::Unsupported) => Stage::UpdateUnsupported,
            Err(ErrorKind::Conflict) => Stage::UpdateConflict,
            Err(ErrorKind::Authentication | ErrorKind::Permission) => Stage::UpdateDenied,
            Err(ErrorKind::Invalid) => Stage::UpdateInvalid,
            Err(_) => Stage::UpdateUnconfirmed,
        };
        if stage != Stage::UpdateUnconfirmed {
            binding.intent = None;
            binding.unfinished = false;
            self.summary
                .save_binding(binding.clone())
                .map_err(|_| Stage::Storage)?;
        }
        Err(stage)
    }
}
#[cfg(test)]
#[path = "send_binding_tests.rs"]
mod tests;
