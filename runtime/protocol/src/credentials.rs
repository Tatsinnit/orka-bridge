//! Private transport payload, never a renderer or persistent operation value.
use crate::{ProtocolError, Uuid};
use serde::{Deserialize, Deserializer, Serialize};
pub const MAX_CREDENTIAL: usize = 16 * 1024;
pub const CREDENTIAL_PROFILE: u16 = 1;
pub const MODEL_API_KEY_PROFILE: u16 = 2;
pub fn default_credential_profile() -> u16 {
    CREDENTIAL_PROFILE
}
pub fn is_copilot_profile(profile: &u16) -> bool {
    *profile == CREDENTIAL_PROFILE
}
pub fn valid_credential_profile(profile: u16) -> bool {
    matches!(profile, CREDENTIAL_PROFILE | MODEL_API_KEY_PROFILE)
}

#[derive(Clone, PartialEq, Serialize)]
pub struct CredentialPayload(Vec<u8>);
impl CredentialPayload {
    pub fn new(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty()
            || bytes.len() > MAX_CREDENTIAL
            || !bytes.iter().all(|b| (33..=126).contains(b))
        {
            return Err(ProtocolError::Invalid);
        }
        Ok(Self(bytes.to_vec()))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}
impl std::fmt::Debug for CredentialPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialPayload([REDACTED])")
    }
}
impl<'de> Deserialize<'de> for CredentialPayload {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes = Vec::<u8>::deserialize(d)?;
        Self::new(&bytes).map_err(|_| serde::de::Error::custom("invalid credential payload"))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialState {
    pub profile: u16,
    pub generation: Option<Uuid>,
    pub installed: bool,
}
impl CredentialState {
    pub(crate) fn valid(&self) -> bool {
        valid_credential_profile(self.profile)
            && !self.generation.is_some_and(|id| id.is_nil())
            && (!self.installed || self.generation.is_some())
    }
}
