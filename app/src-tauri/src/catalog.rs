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

/// Only connection contention is transient; guest and binding failures need explicit refresh.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CatalogError {
    CatalogBusy,
    CatalogUnavailable,
}
impl From<crate::runtime::types::RuntimeError> for CatalogError {
    fn from(error: crate::runtime::types::RuntimeError) -> Self {
        match error {
            crate::runtime::types::RuntimeError::Busy => Self::CatalogBusy,
            _ => Self::CatalogUnavailable,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReply {
    pub default_model: String,
    #[serde(flatten)]
    pub catalog: WorkspaceCatalog,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::types::RuntimeError;

    #[test]
    fn only_native_connection_contention_is_observable_as_loading() {
        assert_eq!(
            serde_json::to_value(CatalogError::from(RuntimeError::Busy)).unwrap(),
            "catalogBusy"
        );
        for error in [RuntimeError::Closed, RuntimeError::Unavailable] {
            assert_eq!(
                serde_json::to_value(CatalogError::from(error)).unwrap(),
                "catalogUnavailable"
            );
        }
    }
}
