use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub const MAX_FILE: usize = 1024 * 1024;
pub const MAX_OPERATIONS: usize = 32;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotId {
    pub agent: String,
    pub tests: String,
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
impl SnapshotId {
    pub fn of(agent: &[u8], tests: &[u8]) -> Self {
        Self {
            agent: digest(agent),
            tests: digest(tests),
        }
    }
    pub fn valid(&self) -> bool {
        [&self.agent, &self.tests].into_iter().all(|s| {
            s.len() == 64
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    }
    pub fn key(&self) -> String {
        format!("{}-{}", self.agent, self.tests)
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FileSlot {
    Agent,
    Tests,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    Pending,
    Running,
    Completed,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineResult {
    pub exit: i32,
    pub stdout: Vec<u8>,
    pub has_diagnostics: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationInfo {
    pub state: OperationState,
    pub exit: Option<i32>,
    pub bytes: usize,
    pub has_diagnostics: bool,
}
