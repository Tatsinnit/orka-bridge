use super::*;
use crate::runtime::{
    client::VmClient, credential_store::CredentialStore, credentials::CapturedGithubToken,
};
use bridge_runtime_protocol::*;
use serde::Deserialize;
use std::io::{Read, Write};

#[derive(Debug, Serialize)]
pub struct ModelSetupState {
    pub associated: bool,
    /// Matching Connect acknowledgment on this lease, not inference or fresh gateway inspection.
    pub ready: bool,
    pub unknown: bool,
    pub connection: Option<ModelConnection>,
    /// GitHub login verified when this Copilot sign-in was saved. Ready connections only.
    pub account: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    Storing,
    Stored,
    Pending,
    Connected,
    Cleared,
}
/// Closed key-company input; provider names and setup models remain native-owned.
pub type KeyVendor = ModelVendor;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CredentialKind {
    #[default]
    Copilot,
    Key,
}
impl CredentialKind {
    fn is_copilot(&self) -> bool {
        *self == Self::Copilot
    }
}
fn default_selection(vendor: Option<KeyVendor>) -> ModelSelection {
    ModelSelection {
        vendor,
        provider: if vendor.is_some() {
            "bridge-model"
        } else {
            "copilot"
        }
        .into(),
        model: match vendor {
            None => "gpt-5.6-terra",
            Some(KeyVendor::Openai) => "gpt-4.1",
            Some(KeyVendor::Anthropic) => "claude-sonnet-5-5",
        }
        .into(),
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Binding {
    #[serde(default, skip_serializing_if = "CredentialKind::is_copilot")]
    kind: CredentialKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor: Option<KeyVendor>,
    slot: Uuid,
    installation: Uuid,
    phase: Phase,
    nonce: Option<Uuid>,
    connection: Option<ModelConnection>,
}
fn unavailable() -> SetupError {
    SetupError {
        kind: "modelUnavailable",
        message: "The stored model connection is unavailable or its completion is unknown.",
        vendor: None,
        diagnostic: None,
        native: None,
        recovery: None,
    }
}
fn valid_saved_connection(b: &Binding) -> bool {
    b.connection
        .as_ref()
        .is_none_or(|c| !c.id.is_nil() && !c.generation.is_nil() && c.selection.valid())
}
fn mismatch(installation: Uuid, b: &Binding) -> SetupError {
    if !valid_saved_connection(b) {
        return unavailable();
    }
    SetupError {
        recovery: Some(Box::new(ModelRecovery {
            installation,
            saved_installation: b.installation,
            credential_slot: b.slot,
            reset_stage: None,
        })),
        ..SetupError::model("modelInstallationMismatch")
    }
}
fn read_binding(root: &Path) -> Result<Option<Binding>, SetupError> {
    let path = root.join("model-v1");
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Ok(m) if m.is_file() && m.len() <= 16384 => {}
        _ => return Err(unavailable()),
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| unavailable())?
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > 16384 {
        return Err(unavailable());
    }
    let b: Binding = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if (b.kind == CredentialKind::Key) != b.vendor.is_some()
        || b.connection
            .as_ref()
            .is_some_and(|c| c.selection.vendor != b.vendor)
        || b.slot.is_nil()
        || b.installation.is_nil()
        || b.nonce.is_some_and(|id| id.is_nil())
        || (matches!(b.phase, Phase::Storing | Phase::Stored)
            && (b.nonce.is_some() || b.connection.is_some()))
        || (matches!(b.phase, Phase::Connected | Phase::Cleared)
            && (b.nonce.is_none() || b.connection.is_none()))
    {
        return Err(unavailable());
    }
    Ok(Some(b))
}
fn write_binding(root: &Path, b: &Binding) -> Result<(), SetupError> {
    write_file(
        root,
        "model-v1",
        &serde_json::to_vec(b).map_err(|_| unavailable())?,
    )
}
fn write_file(root: &Path, name: &str, bytes: &[u8]) -> Result<(), SetupError> {
    let path = root.join(format!("model-next-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|_| unavailable())?;
    let result = (|| {
        file.write_all(bytes).map_err(|_| unavailable())?;
        file.sync_all().map_err(|_| unavailable())?;
        drop(file);
        crate::runtime::journal::replace(&path, &root.join(name)).map_err(|_| unavailable())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&path);
    }
    result
}
/// Exact nonsecret source bytes, held under the existing mutation owner.
#[allow(dead_code)] // Native upgrade backend remains unexposed until credential guards land.
pub(crate) struct UpgradeMetadata {
    pub model: Option<Vec<u8>>,
    pub account: Option<Vec<u8>>,
    _owner: fs::File,
}
impl SetupStore {
    #[allow(dead_code)]
    pub(crate) fn snapshot_for_upgrade(
        &self,
        installation: Uuid,
    ) -> Result<UpgradeMetadata, SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_ref().ok_or_else(unavailable)?;
        fs::create_dir_all(root).map_err(|_| unavailable())?;
        let owner = model_owner(root)?;
        let binding = read_binding(root)?;
        if binding
            .as_ref()
            .is_some_and(|b| b.installation != installation || !valid_saved_connection(b))
        {
            return Err(unavailable());
        }
        let model = binding
            .as_ref()
            .map(|_| fs::read(root.join("model-v1")).map_err(|_| unavailable()))
            .transpose()?;
        let account = match fs::symlink_metadata(root.join(ACCOUNT_FILE)) {
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Ok(m) if m.is_file() && m.len() <= MAX_ACCOUNT_FILE => {
                let bytes = fs::read(root.join(ACCOUNT_FILE)).map_err(|_| unavailable())?;
                let record: AccountRecord =
                    serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
                if bytes.len() as u64 > MAX_ACCOUNT_FILE
                    || !crate::runtime::credentials::valid_login(&record.login)
                    || binding
                        .as_ref()
                        .is_none_or(|b| b.slot != record.slot || b.kind != CredentialKind::Copilot)
                {
                    return Err(unavailable());
                }
                Some(bytes)
            }
            _ => return Err(unavailable()),
        };
        Ok(UpgradeMetadata {
            model,
            account,
            _owner: owner,
        })
    }
}
// Independent descriptors coordinate native mutations across instances and processes.
fn model_owner(root: &Path) -> Result<fs::File, SetupError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("model-owner.lock"))
        .map_err(|_| unavailable())?;
    file.try_lock().map_err(|_| unavailable())?;
    Ok(file)
}
// Called under the model-owner lock for this installation's Storing reservation.
// A failed read is not proof of absence unless CredReadW specifically reports NOT_FOUND.
fn discard_absent_reservation(
    root: &Path,
    b: &Binding,
    store: &impl CredentialStore,
) -> Result<(), SetupError> {
    match store.load(b.slot) {
        Err(StoreError::Native {
            call: NativeCall::CredReadW,
            code: 1168,
        }) => fs::remove_file(root.join("model-v1")).map_err(|_| unavailable()),
        Err(error @ StoreError::Native { .. }) => Err(SetupError::store(error)),
        _ => Ok(()),
    }
}
// Separate from the strict Binding schema so older builds still read their binding.
const ACCOUNT_FILE: &str = "model-account-v1";
const MAX_ACCOUNT_FILE: u64 = 256;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountRecord {
    slot: Uuid,
    login: String,
}
// Best effort: missing or failed metadata means "account unavailable", never a failed sign-in.
fn save_account(root: &Path, slot: Uuid, login: Option<&str>) {
    let saved = login.is_some_and(|login| {
        let record = AccountRecord {
            slot,
            login: login.into(),
        };
        serde_json::to_vec(&record)
            .ok()
            .is_some_and(|bytes| write_file(root, ACCOUNT_FILE, &bytes).is_ok())
    });
    if !saved {
        let _ = fs::remove_file(root.join(ACCOUNT_FILE));
    }
}
fn read_account(root: &Path, b: &Binding) -> Option<String> {
    if b.kind != CredentialKind::Copilot {
        return None;
    }
    let path = root.join(ACCOUNT_FILE);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_ACCOUNT_FILE {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_ACCOUNT_FILE + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let record: AccountRecord = serde_json::from_slice(&bytes).ok()?;
    (bytes.len() as u64 <= MAX_ACCOUNT_FILE
        && record.slot == b.slot
        && crate::runtime::credentials::valid_login(&record.login))
    .then_some(record.login)
}
fn state(root: &Path, b: Option<&Binding>, ready: bool) -> ModelSetupState {
    ModelSetupState {
        associated: b.is_some_and(|b| b.phase != Phase::Storing),
        ready,
        unknown: b.is_some_and(|b| matches!(b.phase, Phase::Storing | Phase::Pending)),
        connection: b.and_then(|b| b.connection.clone()),
        account: b.filter(|_| ready).and_then(|b| read_account(root, b)),
    }
}
fn cleanup_acknowledged(remote: &ModelState, known: &ModelConnection) -> bool {
    remote.phase == ModelPhase::Idle
        && remote.connection.is_none()
        && !remote.credential.installed
        && remote
            .credential
            .generation
            .is_some_and(|g| g != known.generation)
}
fn finish_connect(
    root: &Path,
    b: &mut Binding,
    remote: ModelState,
) -> Result<ModelSetupState, SetupError> {
    b.connection = remote.connection;
    if remote.phase != ModelPhase::Ready {
        write_binding(root, b).map_err(|_| SetupError::connect(remote.diagnostic.clone()))?;
        return Err(SetupError::connect(remote.diagnostic));
    }
    b.phase = Phase::Connected;
    write_binding(root, b)?;
    Ok(state(root, Some(b), true))
}
// Explicit intent only, under both ownership locks. A fresh guest has no token,
// but kmx's owned model resources survive on the state disk and must be stopped.
fn cleanup_previous_boot(
    root: &Path,
    b: &mut Binding,
    client: &mut VmClient,
) -> Result<(), SetupError> {
    let connection = b.connection.as_ref().ok_or_else(unavailable)?;
    let before = client
        .model_status()
        .map_err(|_| SetupError::model("modelReconnect"))?;
    if !before.retained_cleanup
        || before.phase != ModelPhase::Idle
        || before.connection.is_some()
        || before.credential.installed
        || before.credential.generation.is_some()
    {
        return Err(SetupError::model("modelReconnect"));
    }
    client
        .disconnect_retained_model(connection)
        .map_err(|_| SetupError::model("modelReconnect"))?;
    b.phase = Phase::Stored;
    b.nonce = None;
    b.connection = None;
    write_binding(root, b)
}
impl SetupStore {
    // Diagnose the saved installation before association/vendor refusals or any capture.
    // Mutating paths still check again under their own ownership guards.
    fn check_model_installation(&self, installation: Uuid) -> Result<(), SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        let _owner = model_owner(root)?;
        if installation.is_nil() {
            return Err(unavailable());
        }
        if let Some(b) = read_binding(root)? {
            if b.installation != installation {
                return Err(mismatch(installation, &b));
            }
        }
        Ok(())
    }
    /// A new installation owns a new journal/state disk, not the old model route.
    /// Explicit mismatch recovery forgets host custody only; it cannot authorize guest cleanup.
    pub fn reset_mismatched_model(
        &self,
        installation: Uuid,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        selected(root)?;
        let _owner = model_owner(root)?;
        if installation.is_nil() {
            return Err(unavailable());
        }
        let Some(b) = read_binding(root)? else {
            return Ok(state(root, None, false));
        };
        if b.installation == installation || !valid_saved_connection(&b) {
            return Err(unavailable());
        }
        let failure = |mut error: SetupError, stage| {
            let mut recovery = mismatch(installation, &b).recovery.unwrap();
            recovery.reset_stage = Some(stage);
            error.recovery = Some(recovery);
            error
        };
        // Both native stores treat an absent slot as success. Keep the exact record
        // until deletion is confirmed, so interrupted/failed resets can retry this slot.
        store
            .delete(b.slot)
            .map_err(|e| failure(SetupError::store(e), "credentialRemovalUnconfirmed"))?;
        fs::remove_file(root.join("model-v1"))
            .map_err(|_| failure(unavailable(), "recordRemovalUnconfirmed"))?;
        Ok(state(root, None, false))
    }
    fn recover_empty_reservation(
        &self,
        installation: Uuid,
        store: &impl CredentialStore,
    ) -> Result<(), SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        if !selected(root)? {
            return Ok(());
        }
        let _owner = model_owner(root)?;
        if let Some(b) = read_binding(root)? {
            if b.installation == installation && b.phase == Phase::Storing {
                discard_absent_reservation(root, &b, store)?;
            }
        }
        Ok(())
    }
    /// Fixed explicit app intent. Capture is never automatic and never repeats an association.
    pub fn connect_stored_model(
        &self,
        installation: Uuid,
        client: &mut VmClient,
        store: &impl CredentialStore,
        capture: impl FnOnce() -> Result<CapturedGithubToken, crate::runtime::credentials::CaptureError>,
    ) -> Result<ModelSetupState, SetupError> {
        self.check_model_installation(installation)?;
        // Explicit sign-in also recovers an empty reservation left by an older failed save.
        self.recover_empty_reservation(installation, store)?;
        let binding = self.model_binding_status()?;
        if !binding.associated && !binding.unknown {
            // A serving VM can still have an older handler. Probe before host capture/save.
            let before = client
                .model_status()
                .map_err(|_| SetupError::model("modelGuest"))?;
            if before.phase != ModelPhase::Idle || before.credential.installed {
                return Err(SetupError::model("modelGuest"));
            }
            use crate::runtime::credentials::CaptureError;
            let token = capture().map_err(|error| {
                SetupError::model(match error {
                    CaptureError::Unavailable => "modelCaptureUnavailable",
                    CaptureError::Failed => "modelCaptureFailed",
                    CaptureError::InvalidOutput => "modelCaptureInvalid",
                    CaptureError::TimedOut => "modelCaptureTimedOut",
                })
            })?;
            self.associate_captured(installation, &token, store)?;
        }
        let selection = {
            let _access = self.access.lock().map_err(|_| unavailable())?;
            let b = read_binding(self.root.as_deref().ok_or_else(unavailable)?)?
                .ok_or_else(unavailable)?;
            b.connection
                .map(|c| c.selection)
                .unwrap_or_else(|| default_selection(b.vendor))
        };
        let vendor = selection.vendor;
        self.connect_model(installation, selection, client, store)
            .map_err(|mut error| {
                // Recovery needs the saved company even when no successful reply can name it.
                error.vendor = vendor;
                error
            })
    }
    /// Key-only retry cannot capture a GitHub token when an earlier attempt never saved.
    pub fn connect_saved_key_model(
        &self,
        installation: Uuid,
        vendor: KeyVendor,
        client: &mut VmClient,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        self.check_model_installation(installation)?;
        let selection = {
            let _access = self.access.lock().map_err(|_| unavailable())?;
            let root = self.root.as_deref().ok_or_else(unavailable)?;
            let b = read_binding(root)?.ok_or_else(|| SetupError::model("modelKeyInvalid"))?;
            if b.vendor != Some(vendor) {
                return Err(SetupError::model("modelAlreadyAssociated"));
            }
            b.connection
                .map(|c| c.selection)
                .unwrap_or_else(|| default_selection(Some(vendor)))
        };
        self.connect_model(installation, selection, client, store)
            .map_err(|mut error| {
                error.vendor = Some(vendor);
                error
            })
    }
    /// One saved connection. Replacement requires explicit, confirmed removal first.
    pub fn connect_key_model(
        &self,
        installation: Uuid,
        vendor: KeyVendor,
        key: &str,
        client: &mut VmClient,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        self.check_model_installation(installation)?;
        let prefix = match vendor {
            KeyVendor::Openai => "sk-",
            KeyVendor::Anthropic => "sk-ant-",
        };
        if !key.starts_with(prefix)
            || key.len() <= prefix.len()
            || key.len() > MAX_CREDENTIAL
            || !key.bytes().all(|b| (b'!'..=b'~').contains(&b))
            || (vendor == KeyVendor::Openai && key.starts_with("sk-ant-"))
        {
            return Err(SetupError::model("modelKeyInvalid"));
        }
        let binding = self.model_binding_status()?;
        if binding.associated || binding.unknown {
            return Err(SetupError::model("modelAlreadyAssociated"));
        }
        let before = client
            .model_status()
            .map_err(|_| SetupError::model("modelGuest"))?;
        if before.phase != ModelPhase::Idle || before.credential.installed {
            return Err(SetupError::model("modelGuest"));
        }
        self.associate_secret_for_vendor(installation, key.as_bytes(), Some(vendor), None, store)?;
        self.connect_model(installation, default_selection(Some(vendor)), client, store)
            .map_err(|mut error| {
                error.vendor = Some(vendor);
                error
            })
    }
    /// Explicit native intent only. Neither setup status nor reopen captures a sign-in.
    pub fn associate_captured(
        &self,
        installation: Uuid,
        capture: &CapturedGithubToken,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        self.associate_secret_for_vendor(
            installation,
            capture.as_bytes(),
            None,
            capture.account(),
            store,
        )
    }
    #[cfg(test)]
    fn associate_secret(
        &self,
        installation: Uuid,
        secret: &[u8],
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        self.associate_secret_for_vendor(installation, secret, None, None, store)
    }
    fn associate_secret_for_vendor(
        &self,
        installation: Uuid,
        secret: &[u8],
        vendor: Option<KeyVendor>,
        account: Option<&str>,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        if installation.is_nil() || !selected(root)? {
            return Err(unavailable());
        }
        let _owner = model_owner(root)?;
        if read_binding(root)?.is_some() {
            return Err(SetupError::model("modelAlreadyAssociated"));
        }
        CredentialPayload::new(secret).map_err(|_| unavailable())?;
        let mut b = Binding {
            kind: if vendor.is_some() {
                CredentialKind::Key
            } else {
                CredentialKind::Copilot
            },
            vendor,
            slot: Uuid::new_v4(),
            installation,
            phase: Phase::Storing,
            nonce: None,
            connection: None,
        };
        // Reserve the exact slot durably: save can apply and then report an error.
        write_binding(root, &b)?;
        if let Err(error) = store.save(b.slot, secret) {
            // Preserve the save error, including its Windows code, even if cleanup fails.
            discard_absent_reservation(root, &b, store).map_err(|_| SetupError::store(error))?;
            return Err(SetupError::store(error));
        }
        b.phase = Phase::Stored;
        // Publication can be visible even when the final directory flush fails.
        write_binding(root, &b)?;
        save_account(root, b.slot, account);
        Ok(state(root, Some(&b), false))
    }
    /// Reports an association, not live model readiness; never reads the OS store.
    /// Read under the same ownership as Connect/Disconnect, never loading a secret.
    pub fn workspace_catalog(
        &self,
        installation: Uuid,
        client: &mut VmClient,
    ) -> Result<crate::catalog::CatalogReply, &'static str> {
        let failure = "The connected workspace catalog is unavailable.";
        let _access = self.access.lock().map_err(|_| failure)?;
        let root = self.root.as_deref().ok_or(failure)?;
        if !selected(root).map_err(|_| failure)? {
            return Err(failure);
        }
        let _owner = model_owner(root).map_err(|_| failure)?;
        let binding = read_binding(root).map_err(|_| failure)?.ok_or(failure)?;
        let connection = binding.connection.ok_or(failure)?;
        if binding.installation != installation
            || binding.nonce != Some(client.boot_nonce())
            || binding.phase != Phase::Connected
            || !connection.selection.valid()
            || connection.id.is_nil()
            || connection.generation.is_nil()
        {
            return Err(failure);
        }
        let mut catalog = client.workspace_catalog().map_err(|_| failure)?;
        crate::catalog::normalize_models(&mut catalog, connection.selection.vendor);
        Ok(crate::catalog::CatalogReply {
            default_model: connection.selection.model,
            catalog,
        })
    }
    pub fn model_binding_status(&self) -> Result<ModelSetupState, SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        selected(root)?;
        Ok(state(root, read_binding(root)?.as_ref(), false))
    }
    fn finish_model_connect(
        &self,
        root: &Path,
        b: &mut Binding,
        remote: ModelState,
        client: &mut VmClient,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        let result = finish_connect(root, b, remote);
        if b.kind == CredentialKind::Key
            && result.as_ref().is_err_and(|e| {
                matches!(
                    e.kind,
                    "modelCredentialRejected"
                        | "modelProviderUnreachable"
                        | "modelModelUnavailable"
                )
            })
        {
            if b.connection.is_none() {
                // A complete kmx rejection carries its anchored identity. Without one,
                // completion is uncertain: retain custody and allow only same-attempt retry.
                return Err(SetupError::model("modelConnect"));
            }
            // Reuse sign-out's removal lifecycle without releasing either ownership guard.
            self.finish_sign_out(root, b, Some(client), store)
                .map_err(|_| SetupError::model("modelKeyCleanup"))?;
        }
        result
    }
    /// Blocking native worker call. Caller owns the established VM client/lease.
    pub fn connect_model(
        &self,
        installation: Uuid,
        selection: ModelSelection,
        client: &mut VmClient,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        if !selected(root)? || !selection.valid() {
            return Err(unavailable());
        }
        let _owner = model_owner(root)?;
        let mut b = read_binding(root)?.ok_or_else(unavailable)?;
        if b.installation != installation {
            return Err(mismatch(installation, &b));
        }
        if b.vendor != selection.vendor || matches!(b.phase, Phase::Storing | Phase::Cleared) {
            return Err(unavailable());
        }
        if matches!(b.phase, Phase::Pending | Phase::Connected)
            && b.nonce != Some(client.boot_nonce())
        {
            cleanup_previous_boot(root, &mut b, client)?;
        }
        if b.phase == Phase::Pending {
            // Only explicit intent on the original lease can reconcile an uncertain attempt.
            if b.nonce != Some(client.boot_nonce()) {
                return Err(unavailable());
            }
            let remote = client
                .model_status()
                .map_err(|_| SetupError::model("modelConnect"))?;
            if (remote.credential.installed
                && remote.credential.profile != selection.credential_profile())
                || remote.connection.as_ref().is_some_and(|c| {
                    c.selection != selection
                        || b.connection.as_ref().is_some_and(|known| known != c)
                })
            {
                return Err(unavailable());
            }
            if remote.phase == ModelPhase::Ready {
                b.connection = remote.connection;
                b.phase = Phase::Connected;
                write_binding(root, &b)?;
                return Ok(state(root, Some(&b), true));
            }
            if let Some(connection) = &remote.connection {
                // Persist the acknowledged identity before cleanup, never guess one.
                b.connection = Some(connection.clone());
                write_binding(root, &b)?;
                let stopped = client
                    .disconnect_model(connection)
                    .map_err(|_| SetupError::model("modelConnect"))?;
                if stopped.phase != ModelPhase::Idle {
                    let mut error = SetupError::connect(stopped.diagnostic);
                    if b.kind == CredentialKind::Key {
                        error.kind = "modelKeyCleanup";
                    }
                    return Err(error);
                }
                client
                    .clear_credential_file(connection.generation)
                    .map_err(|_| SetupError::model("modelDelivery"))?;
            } else if let Some(known) = &b.connection {
                // Cleanup may have completed before the host's Stored write failed.
                // Same-lease Idle plus a cleared, advanced generation proves completion.
                if !cleanup_acknowledged(&remote, known) || known.selection != selection {
                    return Err(unavailable());
                }
            } else if remote.credential.installed {
                let generation = remote.credential.generation.ok_or_else(unavailable)?;
                // Guest admits the original attempt only; kmx's retained record refuses
                // partial creates. Do not recapture, load or republish credentials.
                let remote = client
                    .connect_model(generation, selection)
                    .map_err(|_| SetupError::model("modelConnect"))?;
                return self.finish_model_connect(root, &mut b, remote, client, store);
            } else if remote.phase != ModelPhase::Idle {
                return Err(unavailable());
            }
            b.phase = Phase::Stored;
            b.nonce = None;
            b.connection = None;
            write_binding(root, &b)?;
        }
        if b.phase == Phase::Connected {
            // Health-only on the same lease: no capture, store load or delivery.
            if b.nonce != Some(client.boot_nonce())
                || b.connection
                    .as_ref()
                    .is_none_or(|c| c.selection != selection)
            {
                return Err(unavailable());
            }
            let healthy = client
                .model_status()
                .is_ok_and(|s| s.phase == ModelPhase::Ready && s.connection == b.connection);
            if !healthy {
                b.phase = Phase::Pending;
                write_binding(root, &b)?;
                return Err(unavailable());
            }
            return Ok(state(root, Some(&b), true));
        }
        // An older image must be rejected before retrieving any secret.
        let before = client
            .model_status()
            .map_err(|_| SetupError::model("modelGuest"))?;
        if before.phase != ModelPhase::Idle || before.credential.installed {
            return Err(SetupError::model("modelGuest"));
        }
        let secret = store.load(b.slot).map_err(SetupError::store)?;
        b.phase = Phase::Pending;
        b.nonce = Some(client.boot_nonce());
        write_binding(root, &b)?; // durable barrier before either guest mutation
        let delivered = client
            .deliver_credential_for_profile(
                selection.credential_profile(),
                &secret,
                before.credential.generation,
            )
            .map_err(|_| SetupError::model("modelDelivery"))?;
        let generation = delivered
            .generation
            .ok_or_else(|| SetupError::model("modelDelivery"))?;
        let remote = client
            .connect_model(generation, selection)
            .map_err(|_| SetupError::model("modelConnect"))?;
        self.finish_model_connect(root, &mut b, remote, client, store)
    }
    /// Explicit sign-out; unavailable/unknown VM cleanup never deletes the host slot.
    pub fn sign_out_model(
        &self,
        installation: Uuid,
        client: Option<&mut VmClient>,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        let _access = self.access.lock().map_err(|_| unavailable())?;
        let root = self.root.as_deref().ok_or_else(unavailable)?;
        let _owner = model_owner(root)?;
        if installation.is_nil() {
            return Err(unavailable());
        }
        let Some(mut b) = read_binding(root)? else {
            return Ok(state(root, None, false));
        };
        if b.installation != installation {
            return Err(mismatch(installation, &b));
        }
        if matches!(b.phase, Phase::Pending | Phase::Connected) {
            let client = client.ok_or_else(unavailable)?;
            if b.nonce != Some(client.boot_nonce()) {
                cleanup_previous_boot(root, &mut b, client)?;
            } else {
                if b.phase == Phase::Pending {
                    if let Some(known) = &b.connection {
                        let remote = client.model_status().map_err(|_| unavailable())?;
                        if cleanup_acknowledged(&remote, known) {
                            // Guest cleanup may precede a failed Cleared checkpoint write.
                            b.phase = Phase::Cleared;
                            write_binding(root, &b)?;
                        }
                    }
                }
                return self.finish_sign_out(root, &mut b, Some(client), store);
            }
        }
        self.finish_sign_out(root, &mut b, None, store)
    }
    fn finish_sign_out(
        &self,
        root: &Path,
        b: &mut Binding,
        client: Option<&mut VmClient>,
        store: &impl CredentialStore,
    ) -> Result<ModelSetupState, SetupError> {
        if !matches!(b.phase, Phase::Storing | Phase::Stored | Phase::Cleared) {
            let client = client.ok_or_else(unavailable)?;
            if b.nonce != Some(client.boot_nonce()) {
                return Err(unavailable());
            }
            let connection = b.connection.clone().ok_or_else(unavailable)?;
            b.phase = Phase::Pending;
            write_binding(root, b)?;
            let stopped = client
                .disconnect_model(&connection)
                .map_err(|_| unavailable())?;
            if stopped.phase != ModelPhase::Idle {
                return Err(unavailable());
            }
            client
                .clear_credential_file(connection.generation)
                .map_err(|_| unavailable())?;
            // Retain acknowledged cleanup, so a locked OS store does not require RPC replay.
            b.phase = Phase::Cleared;
            write_binding(root, b)?;
        }
        store.delete(b.slot).map_err(SetupError::store)?;
        fs::remove_file(root.join("model-v1")).map_err(|_| unavailable())?;
        // Only after acknowledged removal; a stale record cannot match a new slot anyway.
        let _ = fs::remove_file(root.join(ACCOUNT_FILE));
        Ok(state(root, None, false))
    }
}

#[cfg(test)]
#[path = "setup_model_tests.rs"]
mod scenarios;

#[cfg(test)]
mod catalog_binding_tests {
    use super::*;
    struct NoIo;
    impl Read for NoIo {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("invalid binding read guest")
        }
    }
    impl Write for NoIo {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            panic!("invalid binding wrote guest")
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn catalog_rejects_wrong_installation_nonce_and_phase_before_wire_io() {
        for case in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let setup = SetupStore::new(Some(dir.path().into()));
            setup.select_local().unwrap();
            let nonce = Uuid::from_u128(1);
            let install = Uuid::from_u128(2);
            let connection = ModelConnection {
                id: Uuid::from_u128(3),
                generation: Uuid::from_u128(4),
                selection: ModelSelection {
                    vendor: None,
                    provider: "copilot".into(),
                    model: "m".into(),
                },
            };
            write_binding(
                setup.root.as_deref().unwrap(),
                &Binding {
                    kind: CredentialKind::Copilot,
                    vendor: None,
                    slot: Uuid::from_u128(5),
                    installation: if case == 0 {
                        Uuid::from_u128(6)
                    } else {
                        install
                    },
                    nonce: Some(if case == 1 { Uuid::from_u128(7) } else { nonce }),
                    phase: if case == 2 {
                        Phase::Pending
                    } else if case == 3 {
                        Phase::Cleared
                    } else {
                        Phase::Connected
                    },
                    connection: Some(connection),
                },
            )
            .unwrap();
            let mut client = VmClient::established(Box::new(NoIo), nonce).unwrap();
            assert!(setup.workspace_catalog(install, &mut client).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::credential_store::StoreError;

    #[test]
    fn failed_save_and_existing_storing_reservation_clear_only_after_confirmed_absence() {
        struct FailedSave(Option<StoreError>);
        impl CredentialStore for FailedSave {
            fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
                Err(StoreError::Native {
                    call: NativeCall::CredWriteW,
                    code: 8,
                })
            }
            fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
                match self.0 {
                    Some(error) => Err(error),
                    None => Ok(CredentialPayload::new(b"synthetic-model-only").unwrap()),
                }
            }
            fn delete(&self, _: Uuid) -> Result<(), StoreError> {
                panic!("a failed save must not delete a credential")
            }
        }
        for (read_error, absent) in [
            (
                Some(StoreError::Native {
                    call: NativeCall::CredReadW,
                    code: 1168,
                }),
                true,
            ),
            (None, false),
            (Some(StoreError::Unavailable), false),
            (Some(StoreError::Invalid), false),
            (
                Some(StoreError::Native {
                    call: NativeCall::CredReadW,
                    code: 5,
                }),
                false,
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let setup = SetupStore::new(Some(tmp.path().to_owned()));
            setup.select_local().unwrap();
            let install = Uuid::new_v4();
            let vault = FailedSave(read_error);
            let error = setup
                .associate_secret(install, b"synthetic-model-only", &vault)
                .unwrap_err();
            assert_eq!(
                serde_json::to_value(error).unwrap()["native"],
                serde_json::json!({"call": "CredWriteW", "code": 8})
            );
            let root = setup.root.as_deref().unwrap();
            assert_eq!(read_binding(root).unwrap().is_none(), absent);
            // Recreate an old failed-save reservation: an explicit sign-in must use the same proof.
            write_binding(
                root,
                &Binding {
                    kind: CredentialKind::Copilot,
                    vendor: None,
                    slot: Uuid::new_v4(),
                    installation: install,
                    phase: Phase::Storing,
                    nonce: None,
                    connection: None,
                },
            )
            .unwrap();
            let mut client =
                VmClient::established(Box::new(std::io::Cursor::new(Vec::new())), Uuid::new_v4())
                    .unwrap();
            let error = setup
                .connect_stored_model(install, &mut client, &vault, || {
                    panic!("the empty test peer must refuse before capture")
                })
                .unwrap_err();
            let expected_kind = match read_error {
                Some(StoreError::Native { code: 5, .. }) => "modelStore",
                _ if absent => "modelGuest",
                _ => "modelUnavailable",
            };
            assert_eq!(error.kind, expected_kind);
            if expected_kind == "modelStore" {
                assert_eq!(
                    serde_json::to_value(error).unwrap()["native"],
                    serde_json::json!({"call": "CredReadW", "code": 5})
                );
            }
            assert_eq!(read_binding(root).unwrap().is_none(), absent);
        }
    }

    #[test]
    fn unsupported_guest_cannot_capture_or_save_first_sign_in() {
        struct Closed;
        impl Read for Closed {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Ok(0)
            }
        }
        impl Write for Closed {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let setup = SetupStore::new(Some(tmp.path().to_owned()));
        setup.select_local().unwrap();
        let mut client = VmClient::established(Box::new(Closed), Uuid::new_v4()).unwrap();
        let mut captures = 0;
        let error = setup
            .connect_stored_model(Uuid::new_v4(), &mut client, &Store, || {
                captures += 1;
                Err(crate::runtime::credentials::CaptureError::Failed)
            })
            .unwrap_err();
        assert_eq!(
            captures, 0,
            "unsupported guest must refuse before first capture"
        );
        assert_eq!(error.kind, "modelGuest");
        assert!(!tmp.path().join("setup-v1/model-v1").exists());
    }
    #[test]
    fn explicit_capture_failure_is_visible_without_reserving_or_saving_a_slot() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = SetupStore::new(Some(tmp.path().to_owned()));
        setup.select_local().unwrap();
        struct Idle(std::io::Cursor<Vec<u8>>);
        impl Read for Idle {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                self.0.read(b)
            }
        }
        impl Write for Idle {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let nonce = Uuid::new_v4();
        let response = encode(&Frame::Result(WireResult {
            version: VERSION,
            boot_nonce: nonce,
            request_id: 3,
            body: ResultBody::ModelState(ModelState {
                retained_cleanup: true,
                diagnostic: None,
                phase: ModelPhase::Idle,
                connection: None,
                credential: CredentialState {
                    profile: CREDENTIAL_PROFILE,
                    generation: None,
                    installed: false,
                },
            }),
        }))
        .unwrap();
        let mut client =
            VmClient::established(Box::new(Idle(std::io::Cursor::new(response))), nonce).unwrap();
        let error = setup
            .connect_stored_model(Uuid::new_v4(), &mut client, &Store, || {
                Err(crate::runtime::credentials::CaptureError::Failed)
            })
            .unwrap_err();
        assert_eq!(error.kind, "modelCaptureFailed");
        assert!(!setup.model_binding_status().unwrap().associated);
        assert!(!tmp.path().join("setup-v1/model-v1").exists());
        let encoded = serde_json::to_string(&error).unwrap();
        assert!(!encoded.contains("synthetic-model-only"));
    }
    struct Store;
    impl CredentialStore for Store {
        fn save(&self, _: Uuid, secret: &[u8]) -> Result<(), StoreError> {
            assert_eq!(secret, b"synthetic-model-only");
            Ok(())
        }
        fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
            panic!("association/status must not load a secret")
        }
        fn delete(&self, _: Uuid) -> Result<(), StoreError> {
            Ok(())
        }
    }
    #[cfg(unix)]
    #[test]
    fn first_sign_in_probes_captures_stores_connects_then_reuses_without_capture() {
        use std::{
            io::{BufRead, BufReader},
            os::unix::net::UnixStream,
        };
        let tmp = tempfile::tempdir().unwrap();
        let setup = SetupStore::new(Some(tmp.path().to_owned()));
        setup.select_local().unwrap();
        let install = Uuid::new_v4();
        let events = Arc::new(Mutex::new(Vec::new()));
        struct Vault(Arc<Mutex<Vec<&'static str>>>);
        impl CredentialStore for Vault {
            fn save(&self, _: Uuid, secret: &[u8]) -> Result<(), StoreError> {
                assert_eq!(secret, b"synthetic-model-only");
                self.0.lock().unwrap().push("save");
                Ok(())
            }
            fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
                self.0.lock().unwrap().push("load");
                Ok(CredentialPayload::new(b"synthetic-model-only").unwrap())
            }
            fn delete(&self, _: Uuid) -> Result<(), StoreError> {
                panic!("Connect must not delete credentials")
            }
        }
        let vault = Vault(events.clone());
        let generation = Uuid::new_v4();
        let connection = ModelConnection {
            id: Uuid::new_v4(),
            selection: ModelSelection {
                vendor: None,
                provider: "copilot".into(),
                model: "gpt-5.6-terra".into(),
            },
            generation,
        };
        let expected = connection.clone();
        let seen = events.clone();
        let root = tmp.path().join("setup-v1");
        let (a, b) = UnixStream::pair().unwrap();
        let peer = std::thread::spawn(move || {
            let mut io = BufReader::new(b);
            for step in 0..6 {
                let mut raw = Vec::new();
                io.read_until(b'\n', &mut raw).unwrap();
                let Frame::Request(r) = serde_json::from_slice::<Frame>(&raw).unwrap() else {
                    panic!("request")
                };
                let credential = CredentialState {
                    profile: CREDENTIAL_PROFILE,
                    generation: if step < 3 { None } else { Some(generation) },
                    installed: step >= 3,
                };
                let body = match (step, &r.body) {
                    (0 | 1, RequestBody::ModelStatus {}) => {
                        seen.lock().unwrap().push("probe");
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            diagnostic: None,
                            phase: ModelPhase::Idle,
                            connection: None,
                            credential,
                        })
                    }
                    (2, RequestBody::CredentialStatus {}) => {
                        seen.lock().unwrap().push("credentialStatus");
                        ResultBody::CredentialState(credential)
                    }
                    (3, RequestBody::CredentialPublish { token, .. }) => {
                        assert_eq!(token.as_bytes(), b"synthetic-model-only");
                        seen.lock().unwrap().push("deliver");
                        ResultBody::CredentialState(credential)
                    }
                    (
                        4,
                        RequestBody::ModelConnect {
                            generation: g,
                            selection,
                        },
                    ) => {
                        assert_eq!(*g, generation);
                        assert_eq!(*selection, expected.selection);
                        assert_eq!(read_binding(&root).unwrap().unwrap().phase, Phase::Pending);
                        seen.lock().unwrap().push("connect");
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            diagnostic: None,
                            phase: ModelPhase::Ready,
                            connection: Some(expected.clone()),
                            credential,
                        })
                    }
                    (5, RequestBody::ModelStatus {}) => {
                        seen.lock().unwrap().push("health");
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            diagnostic: None,
                            phase: ModelPhase::Ready,
                            connection: Some(expected.clone()),
                            credential,
                        })
                    }
                    _ => panic!("unexpected first-use or repeat request"),
                };
                io.get_mut()
                    .write_all(
                        &encode(&Frame::Result(WireResult {
                            version: VERSION,
                            boot_nonce: r.boot_nonce,
                            request_id: r.request_id,
                            body,
                        }))
                        .unwrap(),
                    )
                    .unwrap();
            }
        });
        let mut client = VmClient::established(Box::new(a), Uuid::new_v4()).unwrap();
        assert!(
            setup
                .connect_stored_model(install, &mut client, &vault, || {
                    events.lock().unwrap().push("capture");
                    CapturedGithubToken::try_from(b"synthetic-model-only".as_slice())
                })
                .unwrap()
                .ready
        );
        assert!(
            setup
                .connect_stored_model(install, &mut client, &vault, || {
                    panic!("repeat must never capture")
                })
                .unwrap()
                .ready
        );
        peer.join().unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            [
                "probe",
                "capture",
                "save",
                "probe",
                "load",
                "credentialStatus",
                "deliver",
                "connect",
                "health"
            ]
        );
        let binding = read_binding(&tmp.path().join("setup-v1")).unwrap().unwrap();
        assert_eq!(binding.installation, install);
        assert_eq!(binding.connection, Some(connection));
        assert!(!fs::read_to_string(tmp.path().join("setup-v1/model-v1"))
            .unwrap()
            .contains("synthetic-model-only"));
    }
    #[cfg(unix)]
    #[test]
    fn connect_orders_delivery_and_kmx_and_reopen_is_health_only() {
        use std::{
            io::{BufRead, BufReader, Write},
            os::unix::net::UnixStream,
        };
        let tmp = tempfile::tempdir().unwrap();
        let setup = SetupStore::new(Some(tmp.path().to_owned()));
        setup.select_local().unwrap();
        let install = Uuid::new_v4();
        let events = Arc::new(Mutex::new(Vec::new()));
        struct Vault(Arc<Mutex<Vec<&'static str>>>);
        impl CredentialStore for Vault {
            fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
                self.0.lock().unwrap().push("save");
                Ok(())
            }
            fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
                self.0.lock().unwrap().push("load");
                Ok(CredentialPayload::new(b"synthetic-model-only").unwrap())
            }
            fn delete(&self, _: Uuid) -> Result<(), StoreError> {
                self.0.lock().unwrap().push("delete");
                Ok(())
            }
        }
        let vault = Vault(events.clone());
        setup
            .associate_secret(install, b"synthetic-model-only", &vault)
            .unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        let generation = Uuid::new_v4();
        let connection = ModelConnection {
            id: Uuid::new_v4(),
            selection: ModelSelection {
                vendor: None,
                provider: "copilot".into(),
                model: "gpt-5.6-terra".into(),
            },
            generation,
        };
        let expected = connection.clone();
        let seen = events.clone();
        let root = tmp.path().join("setup-v1");
        let peer = std::thread::spawn(move || {
            let mut io = BufReader::new(b);
            for step in 0..8 {
                let mut raw = Vec::new();
                io.read_until(b'\n', &mut raw).unwrap();
                let Frame::Request(r) = serde_json::from_slice::<Frame>(&raw).unwrap() else {
                    panic!("request")
                };
                let credential = |installed| CredentialState {
                    profile: CREDENTIAL_PROFILE,
                    generation: if step < 2 { None } else { Some(generation) },
                    installed,
                };
                let body = match (step, &r.body) {
                    (0, RequestBody::ModelStatus {}) => {
                        seen.lock().unwrap().push("status");
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            diagnostic: None,
                            phase: ModelPhase::Idle,
                            connection: None,
                            credential: credential(false),
                        })
                    }
                    (1, RequestBody::CredentialStatus {}) => {
                        ResultBody::CredentialState(credential(false))
                    }
                    (2, RequestBody::CredentialPublish { token, .. }) => {
                        assert_eq!(token.as_bytes(), b"synthetic-model-only");
                        seen.lock().unwrap().push("deliver");
                        ResultBody::CredentialState(credential(true))
                    }
                    (
                        3,
                        RequestBody::ModelConnect {
                            generation: g,
                            selection,
                        },
                    ) => {
                        assert_eq!(*g, generation);
                        assert_eq!(*selection, expected.selection);
                        assert_eq!(read_binding(&root).unwrap().unwrap().phase, Phase::Pending);
                        seen.lock().unwrap().push("connect");
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            diagnostic: None,
                            phase: ModelPhase::Ready,
                            connection: Some(expected.clone()),
                            credential: credential(true),
                        })
                    }
                    (4, RequestBody::ModelStatus {}) => ResultBody::ModelState(ModelState {
                        retained_cleanup: true,
                        diagnostic: None,
                        phase: ModelPhase::Ready,
                        connection: Some(expected.clone()),
                        credential: credential(true),
                    }),
                    (5, RequestBody::ModelDisconnect { connection: id }) => {
                        assert_eq!(*id, expected.id);
                        seen.lock().unwrap().push("disconnect");
                        ResultBody::ModelState(ModelState {
                            retained_cleanup: true,
                            diagnostic: None,
                            phase: ModelPhase::Idle,
                            connection: None,
                            credential: credential(true),
                        })
                    }
                    (6, RequestBody::CredentialStatus {}) => {
                        ResultBody::CredentialState(credential(true))
                    }
                    (7, RequestBody::CredentialClear { .. }) => {
                        seen.lock().unwrap().push("clear");
                        ResultBody::CredentialState(CredentialState {
                            profile: CREDENTIAL_PROFILE,
                            generation: Some(Uuid::new_v4()),
                            installed: false,
                        })
                    }
                    _ => panic!("unexpected model request sequence"),
                };
                io.get_mut()
                    .write_all(
                        &encode(&Frame::Result(WireResult {
                            version: VERSION,
                            boot_nonce: r.boot_nonce,
                            request_id: r.request_id,
                            body,
                        }))
                        .unwrap(),
                    )
                    .unwrap();
            }
        });
        let mut client = VmClient::established(Box::new(a), Uuid::new_v4()).unwrap();
        assert!(
            setup
                .connect_stored_model(install, &mut client, &vault, || panic!(
                    "stored binding must not recapture"
                ))
                .unwrap()
                .ready
        );
        assert!(!setup.model_binding_status().unwrap().ready);
        assert!(
            setup
                .connect_model(install, connection.selection.clone(), &mut client, &vault)
                .unwrap()
                .ready
        );
        setup
            .sign_out_model(install, Some(&mut client), &vault)
            .unwrap();
        peer.join().unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "save",
                "status",
                "load",
                "deliver",
                "connect",
                "disconnect",
                "clear",
                "delete"
            ]
        );
        assert!(!setup.model_binding_status().unwrap().associated);
    }
    #[test]
    fn association_persists_only_opaque_metadata_after_store_success() {
        let tmp = tempfile::tempdir().unwrap();
        let setup = SetupStore::new(Some(tmp.path().to_owned()));
        setup.select_local().unwrap();
        let state = setup
            .associate_secret(Uuid::new_v4(), b"synthetic-model-only", &Store)
            .unwrap();
        assert!(state.associated);
        assert!(!state.ready);
        assert!(setup.model_binding_status().unwrap().associated);
        let mut persisted = Vec::new();
        for entry in std::fs::read_dir(tmp.path().join("setup-v1")).unwrap() {
            persisted.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        assert!(!persisted
            .windows(b"synthetic-model-only".len())
            .any(|s| s == b"synthetic-model-only"));
    }
}
