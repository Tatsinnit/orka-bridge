//! Display/identity metadata only. Never deserialize a runtime binding or capability.
use super::TargetBinding;
use crate::agent_folder::{invalid, read, replace};
use cap_std::fs::Dir;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io, path::PathBuf};

const FILE: &str = "clusters-v1.json";
const LIMIT: usize = 1024 * 1024;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Identity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_identity: Option<String>,
    pub context_name: String,
    pub service_name: String,
    pub service_namespace: String,
    pub service_uid: String,
    pub deployment_namespace: String,
    pub deployment_uid: String,
    pub watched_namespace: String,
}
impl From<&TargetBinding> for Identity {
    fn from(b: &TargetBinding) -> Self {
        Self {
            cluster_identity: Some(b.cluster_identity.clone()),
            context_name: b.context_name.clone(),
            service_name: b.service_name.clone(),
            service_namespace: b.service_namespace.clone(),
            service_uid: b.service_uid.clone(),
            deployment_namespace: b.deployment_namespace.clone(),
            deployment_uid: b.deployment_uid.clone(),
            watched_namespace: b.watched_namespace.clone(),
        }
    }
}
impl Identity {
    pub fn key(&self) -> String {
        serde_json::to_string(&(
            &self.context_name,
            &self.cluster_identity,
            &self.service_namespace,
            &self.service_uid,
            &self.deployment_namespace,
            &self.deployment_uid,
            &self.watched_namespace,
        ))
        .unwrap()
    }
    fn valid(&self) -> bool {
        // TargetView's renderer boundary limits display text to 2048 UTF-16 units;
        // native Send identity fields allow at most 4096 bytes. No C0/DEL display text.
        let text = |s: &str, limit: usize| {
            !s.is_empty()
                && s.encode_utf16().count() <= limit
                && s.chars().all(|c| c >= ' ' && c != '\u{7f}')
        };
        // Discovery additionally requires a DNS-label watched namespace.
        let n = self.watched_namespace.as_bytes();
        self.cluster_identity
            .as_deref()
            .is_none_or(crate::clusters::valid_identity)
            && !n.is_empty()
            && n.len() <= 63
            && n[0].is_ascii_alphanumeric()
            && n[n.len() - 1].is_ascii_alphanumeric()
            && n.iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            && [
                &self.context_name,
                &self.service_name,
                &self.service_namespace,
                &self.deployment_namespace,
            ]
            .iter()
            .all(|s| text(s, 2048))
            && [&self.service_uid, &self.deployment_uid]
                .iter()
                .all(|s| s.len() <= 4096 && text(s, 4096))
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Remembered {
    pub identity: Identity,
    // Version 1's wire name is retained; this is last check time, not saved inventory.
    pub observed_at: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    version: u8,
    places: Vec<Remembered>,
}
impl Metadata {
    fn valid(&self) -> bool {
        let mut keys = HashSet::new();
        self.version == 1
            && self.places.len() <= 1024
            && self.places.iter().all(|p| {
                p.identity.valid()
                    && p.observed_at <= 8_640_000_000_000_000
                    && keys.insert(p.identity.key())
            })
    }
}
#[derive(Default)]
pub(super) enum Store {
    #[default]
    Memory,
    Available(Dir, Option<Vec<u8>>),
    Unavailable,
}
fn current(dir: &Dir) -> io::Result<Option<Vec<u8>>> {
    match read(dir, FILE, LIMIT) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
impl Store {
    pub fn load(root: Option<PathBuf>) -> (Self, Vec<Remembered>) {
        let loaded = (|| {
            let root = root.ok_or_else(invalid)?;
            std::fs::create_dir_all(&root)?;
            let dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
            let bytes = current(&dir)?;
            let places = if let Some(bytes) = &bytes {
                let metadata: Metadata = serde_json::from_slice(bytes).map_err(|_| invalid())?;
                if !metadata.valid() {
                    return Err(invalid());
                }
                metadata.places
            } else {
                Vec::new()
            };
            Ok((Self::Available(dir, bytes), places))
        })();
        loaded.unwrap_or((Self::Unavailable, Vec::new()))
    }
    pub fn state(&self) -> &'static str {
        if matches!(self, Self::Unavailable) {
            "unavailable"
        } else {
            "complete"
        }
    }
    pub fn save(&mut self, places: &[Remembered]) -> Result<(), &'static str> {
        let result = (|| {
            match self {
                Self::Memory => return Ok(()),
                Self::Unavailable => return Err(invalid()),
                Self::Available(dir, prior) => {
                    // Never clobber a changed/unreadable file, even after a successful load.
                    if &current(dir)? != prior {
                        return Err(invalid());
                    }
                    let metadata = Metadata {
                        version: 1,
                        places: places.to_vec(),
                    };
                    if !metadata.valid() {
                        return Err(invalid());
                    }
                    let bytes = serde_json::to_vec(&metadata).map_err(|_| invalid())?;
                    if bytes.len() > LIMIT {
                        return Err(invalid());
                    }
                    replace(dir, FILE, &bytes)?;
                    *prior = Some(bytes);
                }
            }
            Ok(())
        })();
        result.map_err(|_: io::Error| {
            *self = Self::Unavailable;
            "Cluster metadata unavailable"
        })
    }
}
