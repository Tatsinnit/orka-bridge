use crate::runtime::credential_store::{NativeCall, StoreError};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupChoice {
    LocalVm,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspacePhase {
    Verifying,
    Materializing,
    Starting,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceProgress {
    pub phase: WorkspacePhase,
    pub bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    pub elapsed_ms: u64,
    pub remaining_ms: Option<u64>,
}

fn is_false(value: &bool) -> bool {
    !*value
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupSnapshot {
    pub selected: Option<SetupChoice>,
    local_vm: &'static str,
    local_vm_error: Option<String>,
    model: &'static str,
    /// Affirmed by the native VM facet, never inferred from saved selection or readiness.
    model_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_progress: Option<WorkspaceProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preflight_failure: Option<preflight::Failure>,
    /// Present only while the local workspace is failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_failure: Option<crate::runtime::types::WorkspaceFailure>,
    /// Native-set only. True once this process has observed any upgrade-backed
    /// destination or an interrupted/restored transaction; never cleared within
    /// a process. Omitted (not just false) in the ordinary, never-replaced case.
    #[serde(skip_serializing_if = "is_false")]
    pub workspace_replaced: bool,
}

impl SetupSnapshot {
    pub fn with_model_support(mut self, supported: bool) -> Self {
        self.model_supported = supported;
        self
    }
    pub fn with_model(mut self, state: &'static str) -> Self {
        self.model = state;
        self
    }
    pub fn with_local_vm(mut self, state: &'static str, error: Option<String>) -> Self {
        self.local_vm = state;
        self.local_vm_error = error;
        self
    }
}

/// Native build facts for the welcome screen; no renderer input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRequirements {
    memory_bytes: u64,
    image_bytes: u64,
    state_disk_bytes: u64,
    download_bytes: Option<u64>,
}
fn requirements_from(
    image_bytes: Option<u64>,
    download_bytes: Option<u64>,
) -> Option<SetupRequirements> {
    Some(SetupRequirements {
        memory_bytes: bridge_runtime_protocol::MEMORY_BYTES,
        image_bytes: image_bytes?,
        state_disk_bytes: bridge_runtime_protocol::STATE_BYTES,
        download_bytes,
    })
}
/// Debug overrides retain engineering sizes; release uses the selected platform image.
#[cfg(debug_assertions)]
pub fn setup_requirements() -> Option<SetupRequirements> {
    requirements_from(crate::dev_image::pinned_image_bytes(), None)
}
#[cfg(not(debug_assertions))]
pub fn setup_requirements() -> Option<SetupRequirements> {
    use crate::{catalog::image_download_bytes, runtime::image_types::ImageTarget};
    let download = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        image_download_bytes(ImageTarget::MacosArm64)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        image_download_bytes(ImageTarget::WindowsAmd64)
    } else {
        None
    };
    requirements_from(crate::catalog::IMAGE_BYTES, download)
}

#[cfg(test)]
mod requirements_tests {
    use super::*;
    #[test]
    fn requirements_use_protocol_sizes_and_exact_fields() {
        let r = requirements_from(Some(7), Some(5)).unwrap();
        assert_eq!(
            serde_json::to_value(r).unwrap(),
            serde_json::json!({
                "memoryBytes": bridge_runtime_protocol::MEMORY_BYTES,
                "imageBytes": 7,
                "stateDiskBytes": bridge_runtime_protocol::STATE_BYTES,
                "downloadBytes": 5
            })
        );
        assert_eq!(requirements_from(None, None), None);
        assert_eq!(
            serde_json::to_value(requirements_from(Some(7), None).unwrap()).unwrap()
                ["downloadBytes"],
            serde_json::Value::Null
        );
    }
    #[test]
    fn download_sizes_include_compressed_image_and_direct_boot_files() {
        use crate::{
            catalog::image_download_bytes as download_bytes, runtime::image_types::ImageTarget,
        };
        assert_eq!(download_bytes(ImageTarget::MacosArm64), Some(2_380_070_756));
        assert_eq!(
            download_bytes(ImageTarget::WindowsAmd64),
            Some(1_967_572_914)
        );
    }
    #[cfg(not(debug_assertions))]
    #[test]
    fn release_reports_selected_platform_requirements() {
        assert_eq!(
            setup_requirements().map(|r| r.image_bytes),
            crate::catalog::IMAGE_BYTES
        );
        assert_eq!(
            setup_requirements()
                .and_then(|r| r.download_bytes)
                .is_some(),
            crate::catalog::IMAGE_AVAILABLE
        );
    }
    #[cfg(all(
        debug_assertions,
        any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "windows", target_arch = "x86_64")
        )
    ))]
    #[test]
    fn debug_supported_target_reports_pinned_bytes() {
        assert!(setup_requirements().unwrap().image_bytes > 0);
        assert_eq!(setup_requirements().unwrap().download_bytes, None);
    }
}

#[derive(Debug, Serialize)]
struct StoreDiagnostic {
    call: NativeCall,
    code: u32,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelRecovery {
    installation: bridge_runtime_protocol::Uuid,
    saved_installation: bridge_runtime_protocol::Uuid,
    credential_slot: bridge_runtime_protocol::Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_stage: Option<&'static str>,
}
#[derive(Debug, Serialize)]
pub struct SetupError {
    kind: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    vendor: Option<bridge_runtime_protocol::ModelVendor>,
    // Only the finite call and numeric code may cross to the sign-in error UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    native: Option<StoreDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<Box<ModelRecovery>>,
    #[serde(skip)]
    diagnostic: Option<bridge_runtime_protocol::ModelDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup: Option<(&'static str, &'static str)>,
}
impl SetupError {
    pub(crate) fn connect(diagnostic: Option<bridge_runtime_protocol::ModelDiagnostic>) -> Self {
        use bridge_runtime_protocol::ModelErrorCode;
        let kind = match diagnostic.as_ref().and_then(|d| d.error_code) {
            Some(ModelErrorCode::CredentialRejected) => "modelCredentialRejected",
            Some(ModelErrorCode::ProviderUnreachable) => "modelProviderUnreachable",
            Some(ModelErrorCode::ModelUnavailable) => "modelModelUnavailable",
            _ => "modelConnect",
        };
        Self {
            diagnostic,
            ..Self::model(kind)
        }
    }
    pub(crate) fn model_detail(&self) -> Option<String> {
        if let Some((operation, class)) = self.cleanup {
            return Some(format!("{operation}:{class}"));
        }
        self.diagnostic
            .as_ref()
            .and_then(|d| serde_json::to_string(d).ok())
            .map(|detail| format!("{} Guest Connect: {detail}", self.message))
    }
    pub(crate) fn store(error: StoreError) -> Self {
        Self {
            native: match error {
                StoreError::Native { call, code } => Some(StoreDiagnostic { call, code }),
                _ => None,
            },
            ..Self::model("modelStore")
        }
    }
    pub(crate) fn model(kind: &'static str) -> Self {
        let message = match kind {
            "modelInstallationMismatch" => "Bridge's saved AI connection is from an earlier setup.",
            "modelKeyInvalid" => "Paste the whole key with the selected company's key prefix.",
            "modelAlreadyAssociated" => "Remove the saved AI connection before connecting another.",
            "modelCredentialRejected" => "The model company did not accept this key.",
            "modelProviderUnreachable" => "The workspace could not reach the model company.",
            "modelModelUnavailable" => "The selected model is not listed for this key.",
            "modelKeyCleanup" => "Key removal was not confirmed. The saved connection was kept for another removal attempt.",
            "modelCaptureUnavailable" => {
                "Install GitHub CLI and sign in to github.com before connecting."
            }
            "modelCaptureFailed" => {
                "The stored GitHub sign-in could not be read. Check your GitHub CLI sign-in."
            }
            "modelCaptureInvalid" => "The stored GitHub sign-in returned invalid credential data.",
            "modelCaptureTimedOut" => "Reading the stored GitHub sign-in timed out.",
            "modelStore" => "The native credential store could not save or load the sign-in.",
            "modelGuest" => "The guest model handler is unavailable or is not idle.",
            "modelReconnect" => "Bridge could not clean up the previous model connection. The local cluster may need a newer image. The saved sign-in was kept.",
            "modelDelivery" => {
                "Private credential delivery was not acknowledged. Completion is unknown."
            }
            "modelConnect" => {
                "Guest Connect did not acknowledge a ready model. Completion is unknown."
            }
            "modelBusy" => "Another operation is using the local VM. Wait for it to finish.",
            "modelSession" => "The local VM is not available for model connection.",
            _ => "The stored model connection is unavailable or its completion is unknown.",
        };
        Self {
            kind,
            message,
            vendor: None,
            cleanup: None,
            diagnostic: None,
            native: None,
            recovery: None,
        }
    }
}
fn storage_error() -> SetupError {
    SetupError {
        kind: "storage",
        message: "The setup choice could not be read or saved.",
        vendor: None,
        cleanup: None,
        diagnostic: None,
        native: None,
        recovery: None,
    }
}

#[path = "setup_preflight.rs"]
pub mod preflight;

#[path = "setup_help.rs"]
pub mod help;
#[path = "setup_model.rs"]
mod model;
pub use model::{KeyVendor, ModelSetupState};
pub(crate) fn publish_upgrade_metadata(
    root: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<(), SetupError> {
    model::write_upgrade_file(root, name, bytes)
}

/// Owns preferences and an opaque sign-in association, never stored secret bytes.
pub struct SetupStore {
    root: Option<PathBuf>,
    upgrade_root: Option<PathBuf>,
    access: Mutex<()>,
}
impl SetupStore {
    pub fn new(app_data: Option<PathBuf>) -> Self {
        Self::with_upgrade_root(app_data.clone(), app_data)
    }
    /// Both native-owned roots are required: Windows roaming/local are distinct.
    pub fn with_upgrade_root(app_data: Option<PathBuf>, local: Option<PathBuf>) -> Self {
        Self {
            root: app_data.map(|path| path.join("setup-v1")),
            upgrade_root: local,
            access: Mutex::new(()),
        }
    }
    pub async fn status_async(self: Arc<Self>) -> Result<SetupSnapshot, SetupError> {
        tokio::task::spawn_blocking(move || self.status())
            .await
            .map_err(|_| storage_error())?
    }
    pub async fn select_local_async(self: Arc<Self>) -> Result<SetupSnapshot, SetupError> {
        tokio::task::spawn_blocking(move || self.select_local())
            .await
            .map_err(|_| storage_error())?
    }
    pub async fn clear_local_async(self: Arc<Self>) -> Result<SetupSnapshot, SetupError> {
        tokio::task::spawn_blocking(move || {
            let _access = self.access.lock().map_err(|_| storage_error())?;
            let root = self.root.as_deref().ok_or_else(storage_error)?;
            if selected(root)? {
                fs::remove_file(root.join("local-vm")).map_err(|_| storage_error())?;
            }
            snapshot(root)
        })
        .await
        .map_err(|_| storage_error())?
    }
    pub fn status(&self) -> Result<SetupSnapshot, SetupError> {
        let _access = self.access.lock().map_err(|_| storage_error())?;
        snapshot(self.root.as_deref().ok_or_else(storage_error)?)
    }
    pub fn select_local(&self) -> Result<SetupSnapshot, SetupError> {
        let _access = self.access.lock().map_err(|_| storage_error())?;
        let root = self.root.as_deref().ok_or_else(storage_error)?;
        if !selected(root)? {
            fs::create_dir_all(root.parent().ok_or_else(storage_error)?)
                .map_err(|_| storage_error())?;
            match fs::create_dir(root) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => return Err(storage_error()),
            }
            // Revalidate after directory creation, including the concurrent creator case.
            selected(root)?;
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(root.join("local-vm"))
            {
                Ok(file) => file.sync_all().map_err(|_| storage_error())?,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => return Err(storage_error()),
            }
        }
        snapshot(root)
    }
}

fn selected(root: &Path) -> Result<bool, SetupError> {
    // Windows can report NotFound for a child of a regular file. Validate the
    // native app-data parent separately instead of mistaking that for no choice.
    match fs::metadata(root.parent().ok_or_else(storage_error)?) {
        Ok(metadata) if metadata.is_dir() => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        _ => return Err(storage_error()),
    }
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        _ => return Err(storage_error()),
    }
    match fs::symlink_metadata(root.join("local-vm")) {
        Ok(metadata) if metadata.file_type().is_file() && metadata.len() == 0 => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        _ => Err(storage_error()),
    }
}
fn snapshot(root: &Path) -> Result<SetupSnapshot, SetupError> {
    Ok(SetupSnapshot {
        selected: selected(root)?.then_some(SetupChoice::LocalVm),
        // Selection is not runtime readiness. These gates have no renderer override.
        local_vm: "notIntegrated",
        local_vm_error: None,
        model: "locked",
        model_supported: false,
        workspace_progress: None,
        preflight_failure: None,
        workspace_failure: None,
        workspace_replaced: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Arc, Barrier},
        time::Duration,
    };

    #[test]
    fn only_native_enrichment_affirms_model_support() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SetupStore::new(Some(tmp.path().to_owned()));
        let selected = store
            .select_local()
            .unwrap()
            .with_local_vm("ready", None)
            .with_model("available");
        assert!(!selected.model_supported);
        for supported in [false, true] {
            let enriched = store
                .status()
                .unwrap()
                .with_model_support(supported)
                .with_local_vm("installing", None)
                .with_model("locked");
            assert_eq!(
                serde_json::to_value(enriched).unwrap()["modelSupported"],
                supported
            );
        }
    }

    #[test]
    fn workspace_replaced_is_omitted_false_and_present_true() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SetupStore::new(Some(tmp.path().to_owned()));
        let unset = store
            .status()
            .unwrap()
            .with_model_support(false)
            .with_local_vm("notIntegrated", None)
            .with_model("locked");
        let value = serde_json::to_value(unset).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "selected": null,
                "localVm": "notIntegrated",
                "localVmError": null,
                "model": "locked",
                "modelSupported": false
            })
        );
        let mut replaced = store
            .status()
            .unwrap()
            .with_model_support(false)
            .with_local_vm("notIntegrated", None)
            .with_model("locked");
        replaced.workspace_replaced = true;
        assert_eq!(
            serde_json::to_value(replaced).unwrap(),
            serde_json::json!({
                "selected": null,
                "localVm": "notIntegrated",
                "localVmError": null,
                "model": "locked",
                "modelSupported": false,
                "workspaceReplaced": true
            })
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn filesystem_waits_do_not_stall_the_async_caller() {
        for select in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let store = Arc::new(SetupStore::new(Some(tmp.path().to_owned())));
            let barrier = Arc::new(Barrier::new(2));
            let (send, receive) = mpsc::channel();
            let blocked = Arc::clone(&store);
            let waiting = Arc::clone(&barrier);
            let holder = std::thread::spawn(move || {
                let _guard = blocked.access.lock().unwrap();
                waiting.wait();
                receive.recv_timeout(Duration::from_secs(2)).is_ok()
            });
            barrier.wait();
            tokio::join!(biased;
                async {
                    if select { store.select_local_async().await.unwrap(); }
                    else { store.status_async().await.unwrap(); }
                },
                async { tokio::task::yield_now().await; let _ = send.send(()); }
            );
            assert!(
                holder.join().unwrap(),
                "Preference I/O blocked the async caller"
            );
        }
    }
}
