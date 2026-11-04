use bridge_runtime_protocol::*;
use serde::{Deserialize, Serialize};
use std::io;

pub fn invalid() -> io::Error {
    io::Error::other("incompatible guest state or request")
}
pub fn validate(b: &BootDescriptor) -> io::Result<()> {
    Frame::Request(WireRequest {
        version: VERSION,
        boot_nonce: b.boot_nonce,
        request_id: 1,
        body: RequestBody::Boot(b.clone()),
    })
    .validate()
    .map_err(|_| invalid())
}
#[derive(Clone, Debug)]
pub struct Disk {
    pub bytes: u64,
    pub fs_uuid: Option<Uuid>,
    pub blank: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub schema_version: String,
    pub installation_id: Uuid,
}
impl Identity {
    pub fn new(id: Uuid) -> Self {
        Self {
            schema_version: "bridge-state/v1".into(),
            installation_id: id,
        }
    }
}
/// Privileged guest I/O boundary. Test implementations never mount or format.
pub trait BootSystem {
    fn probe(&mut self) -> io::Result<Disk>;
    fn format(&mut self, id: Uuid) -> io::Result<()>;
    fn mount_state(&mut self) -> io::Result<()>;
    fn identity(&mut self) -> io::Result<Option<Identity>>;
    fn initialize(&mut self, id: &Identity) -> io::Result<()>;
    fn bind_persistent(&mut self) -> io::Result<()>;
    fn credentials(&mut self) -> io::Result<()>;
    fn ready(&mut self, b: &BootDescriptor) -> io::Result<()>;
    fn save_boot(&mut self, b: &BootDescriptor) -> io::Result<()>;
    fn shutdown(&mut self) -> io::Result<()>;
}
pub fn apply(system: &mut impl BootSystem, b: &BootDescriptor) -> io::Result<()> {
    validate(b)?;
    let mut disk = system.probe()?;
    if disk.bytes != STATE_BYTES {
        return Err(invalid());
    }
    let fresh = disk.blank && b.state_creation == StateCreation::New;
    if fresh {
        system.format(b.state_fs_uuid)?;
        disk = system.probe()?;
    } else if b.state_creation == StateCreation::New {
        return Err(invalid());
    }
    if disk.blank || disk.bytes != STATE_BYTES || disk.fs_uuid != Some(b.state_fs_uuid) {
        return Err(invalid());
    }
    system.mount_state()?;
    if fresh {
        system.initialize(&Identity::new(b.installation_id))?;
    }
    let identity = system.identity()?.ok_or_else(invalid)?;
    if identity.schema_version != "bridge-state/v1" || identity.installation_id != b.installation_id
    {
        return Err(invalid());
    }
    system.bind_persistent()?;
    system.credentials()?;
    system.ready(b)?;
    system.save_boot(b)
}
