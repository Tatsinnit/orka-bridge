//! Local test summaries, not execution handles. Agent ownership outlives results.
use crate::{
    agent_folder::{child, invalid, read, replace, sync},
    example_run::RunState,
    overview::LocalRunObservation,
};
use bridge_runtime_protocol::Uuid;
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io, path::PathBuf, sync::Mutex};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const DIRECTORY: &str = "local-results-v1";
const HISTORY: &str = "history.json";
const SAVING: &str = "saving";
const INCOMPLETE: &str = "incomplete";

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunAgent {
    pub installation: Uuid,
    pub namespace: String,
    pub agent_uid: String,
}
impl From<&LocalRunObservation> for RunAgent {
    fn from(row: &LocalRunObservation) -> Self {
        Self {
            installation: row.installation,
            namespace: row.namespace.clone(),
            agent_uid: row.agent_uid.clone(),
        }
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalResultsSnapshot {
    pub results: Vec<LocalRunObservation>,
    pub run_agents: Vec<RunAgent>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    #[serde(flatten)]
    snapshot: LocalResultsSnapshot,
}
impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: 1,
            snapshot: LocalResultsSnapshot::default(),
        }
    }
}

pub struct LocalResults {
    app_data: Option<PathBuf>,
    // Never acquired while ExampleRun's state mutex is held.
    lock: Mutex<bool>,
}
impl LocalResults {
    pub fn new(app_data: Option<PathBuf>) -> Self {
        Self {
            app_data,
            lock: Mutex::new(false),
        }
    }
    pub fn list(&self) -> io::Result<LocalResultsSnapshot> {
        let failed = self.lock.lock().map_err(|_| invalid())?;
        if *failed {
            return Err(invalid());
        }
        Ok(Self::load(self.root(false)?.as_ref())?.snapshot)
    }
    pub fn record(&self, observation: LocalRunObservation) -> io::Result<()> {
        self.save(observation, |dir, bytes| replace(dir, HISTORY, bytes))
    }
    fn save(
        &self,
        observation: LocalRunObservation,
        commit: impl FnOnce(&Dir, &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut failed = self.lock.lock().map_err(|_| invalid())?;
        let result = (|| {
            validate(&observation)?;
            let root = self.root(true)?.ok_or_else(invalid)?;
            let mut ledger = Self::load(Some(&root))?;
            // Commit any prior interrupted result as unknown before replacing its marker.
            if root.symlink_metadata(SAVING).is_ok() {
                replace(&root, HISTORY, &serde_json::to_vec(&ledger)?)?;
            }
            // Durable intent includes ownership even if the first ledger write is lost.
            let mut pending = observation.clone();
            pending.state = RunState::Unknown;
            pending.report = None;
            upsert(&mut ledger.snapshot, observation);
            let bytes = serde_json::to_vec(&ledger)?;
            // Never silently evict an identity: Overview needs every acknowledged UID.
            if bytes.len() > MAX_BYTES {
                // Later saves cannot reconstruct this acknowledged run's lost UID.
                replace(&root, INCOMPLETE, b"capacity\n")?;
                return Err(invalid());
            }
            replace(&root, SAVING, &serde_json::to_vec(&pending)?)?;
            commit(&root, &bytes)?;
            root.remove_file(SAVING)?;
            sync(&root)
        })();
        // Later writes cannot recover an earlier run's unsaved ownership tuple.
        *failed |= result.is_err();
        result
    }
    fn root(&self, create: bool) -> io::Result<Option<Dir>> {
        let path = self.app_data.as_ref().ok_or_else(invalid)?;
        match std::fs::symlink_metadata(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                // Only this native-supplied app-data ancestor is trusted.
                std::fs::create_dir_all(path)?;
            }
            Err(e) => return Err(e),
            Ok(meta) if !meta.file_type().is_dir() => return Err(invalid()),
            _ => (),
        }
        let parent = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        if create {
            return child(&parent, DIRECTORY).map(Some);
        }
        match parent.symlink_metadata(DIRECTORY) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
            Ok(_) => parent.open_dir_nofollow(DIRECTORY).map(Some),
        }
    }
    fn load(root: Option<&Dir>) -> io::Result<Ledger> {
        let Some(root) = root else {
            return Ok(Ledger::default());
        };
        match root.symlink_metadata(INCOMPLETE) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
            Ok(_) => return Err(invalid()),
        }
        let mut ledger: Ledger = match root.symlink_metadata(HISTORY) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ledger::default(),
            Err(e) => return Err(e),
            Ok(_) => serde_json::from_slice(&read(root, HISTORY, MAX_BYTES)?)?,
        };
        let mut documents = HashSet::new();
        let owners: HashSet<_> = ledger.snapshot.run_agents.iter().collect();
        if ledger.version != 1
            || owners.len() != ledger.snapshot.run_agents.len()
            || owners
                .iter()
                .any(|a| !text(&a.namespace) || !text(&a.agent_uid))
        {
            return Err(invalid());
        }
        for row in &mut ledger.snapshot.results {
            validate(row)?;
            if !documents.insert(row.document_id) || !owners.contains(&RunAgent::from(&*row)) {
                return Err(invalid());
            }
            if row.state == RunState::Running {
                row.state = RunState::Unknown;
                row.report = None;
            }
        }
        match root.symlink_metadata(SAVING) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
            Ok(_) => {
                let mut pending: LocalRunObservation =
                    serde_json::from_slice(&read(root, SAVING, 32 * 1024)?)?;
                validate(&pending)?;
                pending.state = RunState::Unknown;
                pending.report = None;
                upsert(&mut ledger.snapshot, pending);
            }
        }
        Ok(ledger)
    }
}
fn upsert(snapshot: &mut LocalResultsSnapshot, observation: LocalRunObservation) {
    let owner = RunAgent::from(&observation);
    if !snapshot.run_agents.contains(&owner) {
        snapshot.run_agents.push(owner);
    }
    snapshot
        .results
        .retain(|row| row.document_id != observation.document_id);
    snapshot.results.push(observation);
}
fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096
}
fn validate(row: &LocalRunObservation) -> io::Result<()> {
    if !row.source.valid()
        || !text(&row.namespace)
        || !text(&row.agent_name)
        || !text(&row.agent_uid)
        || row.observed_at > 253_402_300_799_999
        || row.state == RunState::Idle
        || (row.state == RunState::Complete) != row.report.is_some()
        || row.report.as_ref().is_some_and(|r| r.passed > r.total)
    {
        return Err(invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_folder::DocumentId;
    use serde_json::json;
    #[test]
    fn capacity_rejection_stays_unavailable_after_restart_and_later_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = Some(temp.path().into());
        let store = LocalResults::new(path.clone());
        let installation = Uuid::new_v4();
        let owner = RunAgent {
            installation,
            namespace: "orka-system".into(),
            agent_uid: "x".repeat(4096),
        };
        let mut ledger = Ledger::default();
        // Fill a valid ledger just below the real limit without thousands of writes.
        let count = (MAX_BYTES - serde_json::to_vec(&ledger).unwrap().len())
            / (serde_json::to_vec(&owner).unwrap().len() + 1);
        ledger.snapshot.run_agents = (0..count)
            .map(|index| RunAgent {
                agent_uid: format!("{index:08}{}", "x".repeat(4088)),
                ..owner.clone()
            })
            .collect();
        let original = serde_json::to_vec(&ledger).unwrap();
        assert!(original.len() <= MAX_BYTES);
        let root = store.root(true).unwrap().unwrap();
        replace(&root, HISTORY, &original).unwrap();
        assert_eq!(store.list().unwrap().run_agents.len(), count);
        let mut row: LocalRunObservation = serde_json::from_value(json!({
            "installation": installation, "documentId": DocumentId::SAMPLE,
            "source": bridge_runtime_protocol::SnapshotId::of(b"a", b"b"),
            "namespace": "orka-system", "agentName": "n".repeat(4096), "agentUid": "r".repeat(4096),
            "observedAt": 1, "state": "complete", "report": {"verdict":"passed", "passed":1, "total":1}
        })).unwrap();
        assert_eq!(
            store.record(row.clone()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(
            read(&root, HISTORY, MAX_BYTES).unwrap() == original,
            "capacity rejection must not change the prior ledger"
        );
        drop(store);
        let reopened = LocalResults::new(path.clone());
        assert!(
            reopened.list().is_err(),
            "capacity failure must survive restart"
        );
        // A later small run must not clear the loss or replace the prior ledger.
        row.agent_name = "small".into();
        row.agent_uid = "small".into();
        assert!(reopened.record(row).is_err());
        assert!(
            read(&root, HISTORY, MAX_BYTES).unwrap() == original,
            "a later record must not change the prior ledger"
        );
        assert!(LocalResults::new(path).list().is_err());
    }
    #[test]
    fn pending_first_save_recovers_acknowledged_identity_without_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = Some(temp.path().into());
        let store = LocalResults::new(root.clone());
        let row: LocalRunObservation = serde_json::from_value(json!({
            "installation": Uuid::new_v4(), "documentId": DocumentId::SAMPLE,
            "source": bridge_runtime_protocol::SnapshotId::of(b"a", b"b"),
            "namespace": "orka-system", "agentName": "test", "agentUid": "pending",
            "observedAt": 1, "state": "running", "report": null
        }))
        .unwrap();
        assert!(store
            .save(row, |_, _| Err(io::Error::other("before ledger commit")))
            .is_err());
        let recovered = LocalResults::new(root).list().unwrap();
        assert_eq!(recovered.run_agents.len(), 1);
        assert_eq!(recovered.run_agents[0].agent_uid, "pending");
        assert_eq!(recovered.results[0].state, RunState::Unknown);
        assert!(recovered.results[0].report.is_none());
    }
    #[test]
    fn interrupted_commit_is_unknown_after_reopen_and_next_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = Some(temp.path().into());
        let store = LocalResults::new(root.clone());
        let row: LocalRunObservation = serde_json::from_value(json!({
            "installation": Uuid::new_v4(), "documentId": DocumentId::SAMPLE,
            "source": bridge_runtime_protocol::SnapshotId::of(b"a", b"b"),
            "namespace": "orka-system", "agentName": "test", "agentUid": "first",
            "observedAt": 1, "state": "complete", "report": {"verdict":"passed", "passed":1, "total":1}
        })).unwrap();
        store.record(row.clone()).unwrap();
        assert!(store
            .save(row.clone(), |dir, bytes| {
                replace(dir, HISTORY, bytes)?;
                Err(io::Error::other("injected flush failure"))
            })
            .is_err());
        assert!(store.list().is_err());
        let mut recovery = row.clone();
        recovery.document_id = DocumentId(Uuid::new_v4());
        store.record(recovery).unwrap();
        assert!(
            store.list().is_err(),
            "a later save cannot recover a lost ownership tuple"
        );
        let reopened = LocalResults::new(root);
        assert_eq!(reopened.list().unwrap().results[0].state, RunState::Unknown);
        let mut next = row;
        next.document_id = DocumentId(Uuid::new_v4());
        reopened.record(next).unwrap();
        assert_eq!(reopened.list().unwrap().results[0].state, RunState::Unknown);
    }
}
