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
    use crate::diagnostics::{self, Context};
    const STAGE: &str = "early";
    if let Err(e) = validate(b) {
        diagnostics::note(Context::new(STAGE, "boot-descriptor-validate"), &e);
        return Err(e);
    }
    let mut disk = diagnostics::context(Context::new(STAGE, "probe"), system.probe())?;
    if disk.bytes != STATE_BYTES {
        diagnostics::log(
            Context::new(STAGE, "state-size-mismatch"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    let fresh = disk.blank && b.state_creation == StateCreation::New;
    if fresh {
        diagnostics::context(
            Context::new(STAGE, "format"),
            system.format(b.state_fs_uuid),
        )?;
        disk = diagnostics::context(Context::new(STAGE, "probe-after-format"), system.probe())?;
    } else if b.state_creation == StateCreation::New {
        diagnostics::log(
            Context::new(STAGE, "state-creation-conflict"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    if disk.blank || disk.bytes != STATE_BYTES || disk.fs_uuid != Some(b.state_fs_uuid) {
        diagnostics::log(
            Context::new(STAGE, "state-post-format-mismatch"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    diagnostics::context(Context::new(STAGE, "mount-state"), system.mount_state())?;
    if fresh {
        diagnostics::context(
            Context::new(STAGE, "initialize"),
            system.initialize(&Identity::new(b.installation_id)),
        )?;
    }
    let identity = diagnostics::context(Context::new(STAGE, "identity"), system.identity())?
        .ok_or_else(|| {
            diagnostics::log(
                Context::new(STAGE, "identity-missing"),
                io::ErrorKind::Other,
                None,
            );
            invalid()
        })?;
    if identity.schema_version != "bridge-state/v1" || identity.installation_id != b.installation_id
    {
        diagnostics::log(
            Context::new(STAGE, "identity-mismatch"),
            io::ErrorKind::Other,
            None,
        );
        return Err(invalid());
    }
    diagnostics::context(
        Context::new(STAGE, "bind-persistent"),
        system.bind_persistent(),
    )?;
    diagnostics::context(Context::new(STAGE, "credentials"), system.credentials())?;
    diagnostics::context(Context::new(STAGE, "apply-ready"), system.ready(b))?;
    diagnostics::context(Context::new(STAGE, "save-boot"), system.save_boot(b))
}
