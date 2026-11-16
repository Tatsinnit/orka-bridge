//! Explicitly confirmed places. Only display/identity metadata survives a restart.
mod metadata;
use metadata::{Identity, Remembered, Store};
#[cfg(test)]
#[path = "places_tests.rs"]
mod tests;
use crate::{
    clusters::{commands::ClusterSession, ClusterError, RemoteAccess, TargetBinding},
    send_summary::AttemptSummary,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAgent {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub agent_type: Option<String>,
    pub model: Option<String>,
    pub tools: Option<Vec<String>>,
    pub record: Option<AttemptSummary>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub key: String,
    pub context: String,
    pub service: String,
    pub namespace: String,
    pub stale: bool,
    pub observed_at: Option<u64>,
    pub checked_at: Option<u64>,
    pub agents: Vec<RemoteAgent>,
    #[serde(skip)]
    identity: Identity,
}
impl Place {
    pub(crate) fn new(binding: TargetBinding) -> Self {
        Self::from_identity(Identity::from(&binding), None)
    }
    fn from_identity(identity: Identity, checked_at: Option<u64>) -> Self {
        Self {
            key: identity.key(),
            context: identity.context_name.clone(),
            service: format!("{}/{}", identity.service_namespace, identity.service_name),
            namespace: identity.watched_namespace.clone(),
            stale: true,
            observed_at: None,
            checked_at,
            agents: Vec::new(),
            identity,
        }
    }
    pub(crate) fn matches(&self, binding: &TargetBinding) -> bool {
        self.identity == Identity::from(binding)
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    pub places: Vec<Place>,
    pub history_state: &'static str,
    pub unobserved: Vec<AttemptSummary>,
}
fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .map(str::to_owned)
}
pub(crate) fn agent(value: &Value, namespace: &str) -> Result<RemoteAgent, ClusterError> {
    let bad = ClusterError::Unavailable;
    if value["metadata"]["namespace"].as_str() != Some(namespace) {
        return Err(bad);
    }
    Ok(RemoteAgent {
        name: text(&value["metadata"]["name"]).ok_or(bad)?,
        namespace: namespace.into(),
        uid: text(&value["metadata"]["uid"]).ok_or(bad)?,
        agent_type: text(&value["spec"]["type"]),
        model: text(&value["spec"]["model"]["name"]),
        tools: value["spec"]["tools"]
            .as_array()
            .and_then(|items| items.iter().map(|t| text(&t["name"])).collect()),
        record: None,
    })
}
struct Entry {
    view: Place,
    access: Option<Arc<RemoteAccess>>,
    generation: u64,
}
#[derive(Default)]
struct State {
    entries: BTreeMap<String, Entry>,
    // Never reset on removal: a late read must not match a remove/re-add generation.
    generation: u64,
    store: Store,
}
impl State {
    fn next_generation(&mut self) -> u64 {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("inventory generation exhausted");
        self.generation
    }
    fn start_read(&mut self, key: &str) -> Option<(u64, Arc<RemoteAccess>)> {
        let generation = self.next_generation();
        let entry = self.entries.get_mut(key)?;
        entry.view.stale = true;
        let access = entry.access.clone()?;
        // Both Overview and Manage share one latest-request-wins revision per entry.
        entry.generation = generation;
        Some((generation, access))
    }
    fn save(&mut self, excluding: Option<&str>) -> Result<(), &'static str> {
        let places: Vec<_> = self
            .entries
            .iter()
            .filter(|(key, _)| excluding != Some(key.as_str()))
            .map(|(_, e)| Remembered {
                identity: e.view.identity.clone(),
                observed_at: e.view.checked_at.unwrap(),
            })
            .collect();
        // Keep the lock across replacement so older snapshots cannot win a write race.
        self.store.save(&places)
    }
}
#[derive(Default)]
pub(crate) struct Inventory {
    state: Mutex<State>,
    refreshing: tokio::sync::Mutex<()>,
}
impl Inventory {
    pub(crate) fn with_storage(root: Option<std::path::PathBuf>) -> Self {
        let (store, places) = Store::load(root);
        let mut state = State {
            store,
            ..State::default()
        };
        for saved in places {
            let view = Place::from_identity(saved.identity, Some(saved.observed_at));
            let generation = state.next_generation();
            state.entries.insert(
                view.key.clone(),
                Entry {
                    view,
                    generation,
                    access: None,
                },
            );
        }
        Self {
            state: Mutex::new(state),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }
    pub(crate) fn persistence_state(&self) -> &'static str {
        self.state.lock().unwrap().store.state()
    }
    pub(crate) fn get(&self, key: &str) -> Option<Place> {
        self.state
            .lock()
            .unwrap()
            .entries
            .get(key)
            .map(|e| e.view.clone())
    }
    pub(crate) fn forget(&self, key: &str) -> Result<(), &'static str> {
        let mut state = self.state.lock().unwrap();
        if !state.entries.contains_key(key) {
            return Err("Cluster unavailable");
        }
        state.save(Some(key))?;
        state.entries.remove(key);
        Ok(())
    }
    pub(crate) fn remember(&self, access: Arc<RemoteAccess>) {
        let mut view = Place::new(access.binding().clone());
        view.checked_at = Some(crate::overview::now());
        let mut state = self.state.lock().unwrap();
        let generation = state.next_generation();
        if let Some(prior) = state.entries.get(&view.key) {
            view.agents = prior.view.agents.clone();
            view.observed_at = prior.view.observed_at;
        }
        state.entries.insert(
            view.key.clone(),
            Entry {
                view,
                access: Some(access),
                generation,
            },
        );
        // Live access is still usable if remembering fails; the snapshot reports unavailable.
        let _ = state.save(None);
    }
    pub(crate) fn invalidate(&self, binding: Option<&TargetBinding>) {
        let key = binding.map(|b| Place::new(b.clone()).key);
        let mut state = self.state.lock().unwrap();
        let generation = state.next_generation();
        for (id, entry) in state.entries.iter_mut() {
            if key.as_ref().is_none_or(|key| key == id) {
                entry.generation = generation;
                entry.access = None;
                entry.view.stale = true;
            }
        }
    }
    pub(crate) fn snapshot(&self) -> Vec<Place> {
        self.state
            .lock()
            .unwrap()
            .entries
            .values()
            .map(|e| e.view.clone())
            .collect()
    }
    pub(crate) async fn refresh(&self) -> Vec<Place> {
        let Ok(_refresh) = self.refreshing.try_lock() else {
            // Concurrent IPC requests share the in-progress read, not another round.
            let _refresh = self.refreshing.lock().await;
            return self.snapshot();
        };
        let reads: Vec<_> = {
            let mut state = self.state.lock().unwrap();
            let keys: Vec<_> = state.entries.keys().cloned().collect();
            keys.into_iter()
                .filter_map(|key| {
                    state
                        .start_read(&key)
                        .map(|(generation, access)| (key, generation, access))
                })
                .collect()
        };
        for (key, generation, access) in reads {
            self.refresh_entry(&key, generation, access).await;
        }
        self.snapshot()
    }
    pub(crate) async fn refresh_one(&self, key: &str) -> Vec<Place> {
        // A selected cluster must not queue behind unrelated Overview inventory reads.
        let read = self.state.lock().unwrap().start_read(key);
        if let Some((generation, access)) = read {
            self.refresh_entry(key, generation, access).await;
        }
        self.get(key).into_iter().collect()
    }
    async fn refresh_entry(&self, key: &str, generation: u64, access: Arc<RemoteAccess>) {
        let result = access.inventory().await;
        let mut state = self.state.lock().unwrap();
        if let Some(entry) = state.entries.get_mut(key) {
            // Reload/re-Use while a read was in flight revokes that response.
            if entry.generation == generation {
                if let Ok(agents) = result {
                    entry.view.agents = agents;
                    entry.view.observed_at = Some(crate::overview::now());
                    entry.view.checked_at = entry.view.observed_at;
                    entry.view.stale = false;
                    let _ = state.save(None);
                }
            }
        }
    }
}
/// Overview's key for the destination an attempt recorded, from the same identity fields `Place` uses.
pub fn attempt_key(attempt: &AttemptSummary) -> Option<String> {
    metadata::attempt_key(attempt)
}
pub(crate) fn project(mut places: Vec<Place>, history: Option<Vec<AttemptSummary>>) -> Places {
    let Some(history) = history else {
        return Places {
            places,
            history_state: "unavailable",
            unobserved: Vec::new(),
        };
    };
    let mut matched = HashSet::new();
    for place in &mut places {
        let b = &place.identity;
        for agent in &mut place.agents {
            // History is newest first. A name is never an association key.
            if let Some(record) = history.iter().find(|r| {
                b.cluster_identity.is_some()
                    && r.target_uids.cluster_identity == b.cluster_identity
                    && r.agent_uid.as_deref() == Some(&agent.uid)
                    && r.target_display == format!("{} · {}", b.context_name, b.watched_namespace)
                    && r.target_uids.service_namespace == b.service_namespace
                    && r.target_uids.service_uid == b.service_uid
                    && r.target_uids.deployment_namespace == b.deployment_namespace
                    && r.target_uids.deployment_uid == b.deployment_uid
                    && r.target_uids.watched_namespace == b.watched_namespace
                    && agent.namespace == b.watched_namespace
            }) {
                matched.insert(record.id.clone());
                agent.record = Some(record.clone());
            }
        }
    }
    Places {
        places,
        history_state: "complete",
        unobserved: history
            .into_iter()
            .filter(|r| !matched.contains(&r.id))
            .collect(),
    }
}
pub async fn list(session: &ClusterSession, history: Option<Vec<AttemptSummary>>) -> Places {
    project(session.overview().await, history)
}
pub async fn list_one(
    session: &ClusterSession,
    history: Option<Vec<AttemptSummary>>,
    key: &str,
) -> Places {
    session.overview_one(key).await;
    // Refresh only this destination, but keep the other cached places on the board.
    project(session.places().places, history)
}
