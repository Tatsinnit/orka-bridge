use bridge_runtime_protocol::WorkspaceCatalog;
use serde::Serialize;

// Both pinned images carry the model/catalog guest and Role.
pub const IMAGE_AVAILABLE: bool = cfg!(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64")
));
pub const IMAGE_BYTES: Option<u64> = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some(17179869184)
} else if cfg!(all(windows, target_arch = "x86_64")) {
    Some(3765960704)
} else {
    None
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReply {
    pub default_model: String,
    #[serde(flatten)]
    pub catalog: WorkspaceCatalog,
}
