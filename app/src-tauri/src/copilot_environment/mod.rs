//! Native-only ownership boundary. No desktop command or GitHub credential flow is exposed here.
use bridge_runtime_protocol::Uuid;
use kube::Config;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid,
    Storage,
    Busy,
    Conflict,
    TargetChanged,
    Unavailable,
    Denied,
    Pending,
}

/// Stable Kubernetes destination, not a Send/Orka connection or a bearer credential.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    context: String,
    server: String,
    trust: String,
}
impl Target {
    pub fn from_config(context: &str, config: &Config) -> Result<Self, Error> {
        let server = config.cluster_url.to_string();
        if context.is_empty()
            || context.len() > 1024
            || server.len() > 2048
            || config
                .cluster_url
                .authority()
                .is_none_or(|a| a.as_str().contains('@'))
            || config.cluster_url.query().is_some()
        {
            return Err(Error::Invalid);
        }
        let file = config
            .root_cert_file
            .as_ref()
            .map(|path| {
                let mut bytes = Vec::new();
                std::fs::File::open(path)
                    .map_err(|_| Error::Unavailable)?
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Unavailable)?;
                if bytes.len() > 8 * 1024 * 1024 {
                    return Err(Error::Invalid);
                }
                Ok(bytes)
            })
            .transpose()?;
        let trust = serde_json::to_vec(&(
            &config.root_cert,
            file,
            &config.tls_server_name,
            config.accept_invalid_certs,
        ))
        .map_err(|_| Error::Invalid)?;
        Ok(Self {
            context: context.into(),
            server,
            trust: format!("{:x}", Sha256::digest(trust)),
        })
    }
}

mod owner;
mod resources;
mod store;
pub use owner::{Connection, EnvironmentView, Owner};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    version: u8,
    pub environment_id: Uuid,
    pub workspace_id: Uuid,
    pub target: Target,
    pub namespace: String,
    pub image: String,
    pub deployment_uid: Option<String>,
    pub pvc_uid: Option<String>,
    pub policy_uid: Option<String>,
}
impl Receipt {
    pub fn resource_name(&self) -> String {
        format!("bridge-copilot-{}", self.environment_id.simple())
    }
    fn validate(&self) -> Result<(), Error> {
        let image = self.image.rsplit_once("@sha256:");
        if self.version != 1
            || self.target.context.is_empty()
            || self.target.context.len() > 1024
            || self.target.server.len() > 2048
            || self
                .target
                .server
                .parse::<http::Uri>()
                .ok()
                .is_none_or(|u| {
                    u.authority().is_none_or(|a| a.as_str().contains('@')) || u.query().is_some()
                })
            || self.target.trust.len() != 64
            || !self.target.trust.bytes().all(|c| c.is_ascii_hexdigit())
            || !valid_namespace(&self.namespace)
            || self.image.len() > 512
            || !image.is_some_and(|(name, hash)| {
                !name.is_empty()
                    && !name.contains('@')
                    && !name.chars().any(char::is_whitespace)
                    && hash.len() == 64
                    && hash.bytes().all(|c| c.is_ascii_hexdigit())
            })
            || [&self.deployment_uid, &self.pvc_uid, &self.policy_uid]
                .iter()
                .any(|uid| uid.as_ref().is_some_and(|s| s.is_empty() || s.len() > 128))
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod fixture;
fn valid_namespace(namespace: &str) -> bool {
    let bytes = namespace.as_bytes();
    bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[cfg(test)]
mod tests;
