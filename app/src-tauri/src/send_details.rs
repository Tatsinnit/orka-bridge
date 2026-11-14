//! Bounded private sidecars; the renderer receives only a small evidence view.
use super::*;
use crate::engine::run::{RunCase, RunReport, SavedResource};

const ATTEMPT_BYTES: usize = 4 * 1024 * 1024;
const TOTAL_BYTES: u64 = 64 * 1024 * 1024;
// A UID may need six JSON bytes per UTF-8 byte; generation needs at most 20 digits.
const UID_RESERVE: u64 = MAX_TEXT as u64 * 6 + 2;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptDetails {
    pub document_id: Option<String>,
    pub model: Option<String>,
    pub captured_at: u64,
    pub cases: Vec<RunCase>,
    pub resources: Vec<SavedResource>,
    pub report: Option<RunReport>,
    pub detail_state: String,
    pub availability: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailView {
    pub id: String,
    pub document_id: Option<String>,
    pub model: Option<String>,
    pub captured_at: Option<u64>,
    pub report: Option<RunReport>,
    pub interrupted: bool,
    pub can_check: bool,
    pub can_remove: bool,
    pub detail_state: String,
    pub availability: Option<String>,
}
impl AttemptDetails {
    pub fn unresolved(&self) -> bool {
        self.resources
            .iter()
            .any(|r| !r.terminal && (r.kind == "Task" || r.uid.is_none()))
    }
    fn charge(&self, bytes: u64) -> u64 {
        let pending = self.resources.iter().filter(|r| r.uid.is_none()).count() as u64;
        let future = (self.cases.len() + 1).saturating_sub(self.resources.len()) as u64;
        // Future generated names, the validated namespace, keys and digest fit in 1 KiB.
        bytes
            + if self.unresolved() {
                pending * (UID_RESERVE + 20) + future * (UID_RESERVE + 1024)
            } else {
                0
            }
    }
    fn omit_answers(&mut self) {
        self.detail_state = "omitted".into();
        if let Some(report) = &mut self.report {
            report.verdict = crate::engine::run::Verdict::Unknown;
            for case in &mut report.cases {
                case.answer = None;
                case.verdict = crate::engine::run::Verdict::Unknown;
                for assertion in &mut case.assertions {
                    assertion.verdict = crate::engine::run::Verdict::Unknown;
                }
            }
        }
    }
    fn validate(&self) -> io::Result<()> {
        let text = |s: &str| !s.is_empty() && s.len() <= MAX_TEXT && !s.contains('\0');
        let dns = |s: &str| {
            text(s)
                && s.len() <= 253
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        };
        if self
            .document_id
            .as_ref()
            .is_some_and(|id| Uuid::parse_str(id).is_err())
            || self.model.as_ref().is_some_and(|s| !text(s))
            || self.captured_at > MAX_TIME
            || self.cases.len() > 32
            || self.resources.len() > 33
            || !matches!(self.detail_state.as_str(), "saved" | "omitted" | "removed")
            || self
                .availability
                .as_deref()
                .is_some_and(|s| !matches!(s, "available" | "missing" | "unavailable"))
            || self.resources.iter().any(|r| {
                !matches!(r.kind.as_str(), "Agent" | "Task")
                    || !dns(&r.name)
                    || !dns(&r.namespace)
                    || r.uid.as_ref().is_some_and(|s| !text(s))
                    || r.spec_digest.len() != 64
                    || !r
                        .spec_digest
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
        {
            return Err(invalid());
        }
        let mut ids = HashSet::new();
        for case in &self.cases {
            if !text(&case.id)
                || !ids.insert(&case.id)
                || case.input().is_empty()
                || case.assertions().is_empty()
                || case.assertions().len() > 64
            {
                return Err(invalid());
            }
            let mut ids = HashSet::new();
            if case
                .assertions()
                .iter()
                .any(|a| !text(a.id()) || !ids.insert(a.id()))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn view(&self, id: &str) -> DetailView {
        DetailView {
            id: id.into(),
            document_id: self.document_id.clone(),
            model: self.model.clone(),
            captured_at: Some(self.captured_at),
            report: self.report.clone(),
            interrupted: self.unresolved(),
            can_check: !self.cases.is_empty() && self.resources.iter().any(|r| r.kind == "Task"),
            can_remove: !self.unresolved() && self.detail_state != "removed",
            detail_state: self.detail_state.clone(),
            availability: self.availability.clone(),
        }
    }
}
fn sidecar(id: &str) -> Result<String, &'static str> {
    let id = Uuid::parse_str(id).map_err(|_| "invalidId")?;
    Ok(format!("{id}.json"))
}
fn load(root: &Dir, id: &str) -> io::Result<Option<AttemptDetails>> {
    let name = sidecar(id).map_err(|_| invalid())?;
    match root.symlink_metadata(&name) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
        Ok(_) => {
            let value: AttemptDetails = serde_json::from_slice(&read(root, &name, ATTEMPT_BYTES)?)
                .map_err(|_| invalid())?;
            value.validate()?;
            Ok(Some(value))
        }
    }
}
impl SummaryStore {
    pub fn details(&self, id: &str) -> Result<DetailView, &'static str> {
        let (summary, details) = self.record(id)?;
        Ok(details.map(|d| d.view(&summary.id)).unwrap_or(DetailView {
            id: summary.id,
            document_id: None,
            model: None,
            captured_at: None,
            report: None,
            interrupted: false,
            can_check: false,
            can_remove: false,
            detail_state: "legacy".into(),
            availability: None,
        }))
    }
    pub(crate) fn record(
        &self,
        id: &str,
    ) -> Result<(AttemptSummary, Option<AttemptDetails>), &'static str> {
        sidecar(id)?;
        let _lock = self.lock.lock().map_err(|_| "storage")?;
        let root = self.root(false).map_err(|_| "storage")?.ok_or("notFound")?;
        let ledger = Self::read_ledger(Some(&root)).map_err(|_| "storage")?;
        let summary = ledger
            .attempts
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("notFound")?;
        let mut details = load(&root, id).map_err(|_| "storage")?;
        if read(&root, PENDING, 36).ok().as_deref() == Some(id.as_bytes()) {
            if let Some(d) = &mut details {
                d.report = None;
                d.availability = Some("unavailable".into());
            }
        }
        Ok((summary, details))
    }
    pub fn remove_details(&self, id: &str) -> Result<DetailView, &'static str> {
        let (summary, details) = self.record(id)?;
        let mut details = details.ok_or("notRecoverable")?;
        if details.unresolved() {
            return Err("unresolved");
        }
        details.cases.clear();
        details.resources.clear();
        details.report = None;
        details.document_id = None;
        details.model = None;
        details.availability = None;
        details.detail_state = "removed".into();
        self.checkpoint(summary, details)?;
        self.details(id)
    }
    pub fn checkpoint(
        &self,
        mut summary: AttemptSummary,
        mut details: AttemptDetails,
    ) -> Result<bool, &'static str> {
        let _lock = self.lock.lock().map_err(|_| "storage")?;
        details.validate().map_err(|_| "storage")?;
        let mut bytes = serde_json::to_vec(&details).map_err(|_| "storage")?;
        if details.charge(bytes.len() as u64) > ATTEMPT_BYTES as u64 {
            details.omit_answers();
            summary.verdict = AttemptVerdict::Unknown;
            bytes = serde_json::to_vec(&details).map_err(|_| "storage")?;
            if details.charge(bytes.len() as u64) > ATTEMPT_BYTES as u64 {
                details.report = None;
                bytes = serde_json::to_vec(&details).map_err(|_| "storage")?;
            }
        }
        if details.charge(bytes.len() as u64) > ATTEMPT_BYTES as u64 {
            return Err("storage");
        }
        let mut saved = self.publish(summary.clone(), Some((&details, &bytes)), |root, bytes| {
            replace(root, FILE_NAME, bytes)
        });
        if saved
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::QuotaExceeded)
            && details.report.is_some()
        {
            // Retry only local cache publication, never a remote mutation. Receipts stay intact.
            details.omit_answers();
            summary.verdict = AttemptVerdict::Unknown;
            bytes = serde_json::to_vec(&details).map_err(|_| "storage")?;
            saved = self.publish(summary.clone(), Some((&details, &bytes)), |root, bytes| {
                replace(root, FILE_NAME, bytes)
            });
            if saved
                .as_ref()
                .is_err_and(|e| e.kind() == io::ErrorKind::QuotaExceeded)
            {
                details.report = None;
                bytes = serde_json::to_vec(&details).map_err(|_| "storage")?;
                saved = self.publish(summary, Some((&details, &bytes)), |root, bytes| {
                    replace(root, FILE_NAME, bytes)
                });
            }
        }
        saved.map_err(|_| "storage")?;
        Ok(details.detail_state == "saved")
    }
    pub(super) fn publish(
        &self,
        summary: AttemptSummary,
        details: Option<(&AttemptDetails, &[u8])>,
        commit: impl FnOnce(&Dir, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        summary.validate().map_err(Self::as_io)?;
        let root = self.root(true)?.ok_or_else(invalid)?;
        let mut ledger = Self::read_ledger(Some(&root)).map_err(Self::as_io)?;
        // Quarantine any detail bytes from an interrupted publication before clearing its marker.
        if let Ok(pending) = read(&root, PENDING, 36) {
            let id = std::str::from_utf8(&pending).map_err(|_| invalid())?;
            if let Some(mut old) = load(&root, id)? {
                old.report = None;
                old.detail_state = "omitted".into();
                old.availability = Some("unavailable".into());
                replace(
                    &root,
                    &sidecar(id).map_err(|_| invalid())?,
                    &serde_json::to_vec(&old).map_err(|_| invalid())?,
                )?;
            }
            replace(
                &root,
                FILE_NAME,
                &serde_json::to_vec(&ledger).map_err(|_| invalid())?,
            )?;
        }
        let id = summary.id.clone();
        if let Some(old) = ledger.attempts.iter_mut().find(|a| a.id == id) {
            *old = summary;
        } else {
            ledger.attempts.push(summary);
        }
        let mut sidecars = std::collections::HashMap::new();
        for entry in root.entries()? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(invalid());
            };
            if name
                .strip_prefix(".bridge-")
                .is_some_and(|id| Uuid::parse_str(id).is_ok())
            {
                root.remove_file(name)?;
                continue;
            }
            let Some(key) = name
                .strip_suffix(".json")
                .filter(|s| Uuid::parse_str(s).is_ok())
            else {
                continue;
            };
            let d = load(&root, key)?.ok_or_else(invalid)?;
            if !d.unresolved() && !ledger.attempts.iter().any(|a| a.id == key) {
                root.remove_file(name)?;
                continue;
            }
            sidecars.insert(
                key.to_string(),
                (d.charge(entry.metadata()?.len()), d.unresolved()),
            );
        }
        if let Some((d, b)) = details {
            sidecars.insert(id.clone(), (d.charge(b.len() as u64), d.unresolved()));
        }
        let mut evicted = Vec::new();
        let bytes = loop {
            let bytes = serde_json::to_vec(&ledger).map_err(|_| invalid())?;
            let pending_agents = ledger
                .attempts
                .iter()
                .filter(|a| {
                    a.agent_uid.is_none()
                        && sidecars.get(&a.id).is_some_and(|(_, protected)| *protected)
                })
                .count() as u64;
            let ledger_charge = bytes.len() as u64 + pending_agents * UID_RESERVE;
            let total = sidecars.values().map(|(n, _)| n).sum::<u64>() + ledger_charge;
            if ledger.attempts.len() <= MAX_ATTEMPTS
                && ledger_charge <= MAX_BYTES
                && total <= TOTAL_BYTES
            {
                break bytes;
            }
            let index = ledger
                .attempts
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    a.id != id && !sidecars.get(&a.id).is_some_and(|(_, protected)| *protected)
                })
                .min_by_key(|(_, a)| a.time)
                .map(|(i, _)| i)
                .ok_or_else(|| io::Error::from(io::ErrorKind::QuotaExceeded))?;
            let old = ledger.attempts.remove(index).id;
            sidecars.remove(&old);
            evicted.push(old);
        };
        replace(&root, PENDING, id.as_bytes())?;
        if let Some((_, bytes)) = details {
            replace(&root, &sidecar(&id).map_err(|_| invalid())?, bytes)?;
        }
        commit(&root, &bytes)?;
        // Index is already committed: a leftover terminal orphan is accounted on next save.
        for id in evicted {
            let _ = root.remove_file(sidecar(&id).map_err(|_| invalid())?);
        }
        root.remove_file(PENDING)?;
        sync(&root)
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;
    #[test]
    fn receipt_reservation_is_consumed_by_acknowledged_uid_bytes() {
        let mut detail: AttemptDetails = serde_json::from_value(serde_json::json!({
            "documentId":null,"model":null,"capturedAt":0,"cases":[],"report":null,
            "detailState":"saved","availability":null,"resources":[{
                "kind":"Task","name":"bridge-task-pending","namespace":"agents","uid":null,
                "generation":0,"specDigest":"a".repeat(64),"terminal":false
            }]
        }))
        .unwrap();
        detail.cases = (0..32).map(|i| serde_json::from_value(serde_json::json!({"id":format!("case-{i}"),"input":"question","assertions":[{"id":"a","type":"contains","value":"answer"}]})).unwrap()).collect();
        detail.resources = (0..33).map(|i| serde_json::from_value(serde_json::json!({"kind":if i==0 {"Agent"} else {"Task"},"name":format!("bridge-resource-{i}"),"namespace":"agents","uid":null,"generation":0,"specDigest":"a".repeat(64),"terminal":false})).unwrap()).collect();
        detail.validate().unwrap();
        let before = detail.charge(serde_json::to_vec(&detail).unwrap().len() as u64);
        for uid in ["u".repeat(4096), "\u{1}".repeat(4096)] {
            for resource in &mut detail.resources {
                resource.uid = Some(uid.clone());
                resource.generation = u64::MAX;
            }
            detail.validate().unwrap();
            let after = detail.charge(serde_json::to_vec(&detail).unwrap().len() as u64);
            assert!(after <= before, "all 33 maximum escaped UIDs must fit their existing reservation: {before} -> {after}");
        }
    }
}
