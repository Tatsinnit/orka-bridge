use bridge_runtime_protocol::{CatalogSection, ModelVendor, WorkspaceCatalog};
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

/// Image payload only: compressed OS plus the direct-boot files, not installed disk capacity.
#[cfg(any(test, not(debug_assertions)))]
pub(crate) fn image_download_bytes(
    target: crate::runtime::image_types::ImageTarget,
) -> Option<u64> {
    use crate::runtime::image_types::ImageTarget;
    let manifest = match target {
        ImageTarget::MacosArm64 => {
            include_bytes!("../../../images/release/macos-arm64.json").as_slice()
        }
        ImageTarget::WindowsAmd64 => {
            include_bytes!("../../../images/release/windows-amd64.json").as_slice()
        }
    };
    let value: serde_json::Value = serde_json::from_slice(manifest).ok()?;
    value["artifact"]["compressedBytes"]
        .as_u64()?
        .checked_add(value["directBoot"]["kernel"]["bytes"].as_u64()?)?
        .checked_add(value["directBoot"]["initrd"]["bytes"].as_u64()?)
}

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

/// Apply Bridge's offered-model policy using the acknowledged connection's company.
/// Keep IDs and upstream capability evidence so saved files remain explainable.
pub fn normalize_models(catalog: &mut WorkspaceCatalog, vendor: Option<ModelVendor>) {
    let CatalogSection::Ready { items } = &mut catalog.models else {
        return;
    };
    for model in items {
        let offered = match vendor {
            None => true, // Copilot owns its advertised picker/capability flags.
            Some(ModelVendor::Openai) => matches!(
                model.id.as_str(),
                "gpt-4.1" | "gpt-4.1-mini" | "gpt-4.1-nano" | "gpt-4o" | "gpt-4o-mini"
            ),
            Some(ModelVendor::Anthropic) => model.id.starts_with("claude-"),
        };
        if !offered {
            model.picker_enabled = Some(false);
        }
    }
    // Eligibility flags grow the projection; preserve the guest's section fallback.
    let fits = |catalog: &WorkspaceCatalog| {
        serde_json::to_vec(catalog)
            .is_ok_and(|bytes| bytes.len() <= bridge_runtime_protocol::MAX_FILE)
    };
    if !fits(catalog) {
        catalog.tools = CatalogSection::Unavailable {};
    }
    if !fits(catalog) {
        catalog.models = CatalogSection::Unavailable {};
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::types::RuntimeError;

    fn listed(rows: serde_json::Value) -> WorkspaceCatalog {
        serde_json::from_value(serde_json::json!({
            "models": {"state":"ready", "items":rows},
            "tools": {"state":"unavailable"}
        }))
        .unwrap()
    }

    #[test]
    fn catalog_openai_policy_intersects_exact_ids_without_rewriting_capabilities() {
        let mut catalog = listed(serde_json::json!([
            {"id":"gpt-4.1"}, {"id":"gpt-4.1-mini"}, {"id":"gpt-4.1-nano"},
            {"id":"gpt-4o"}, {"id":"gpt-4o-mini"},
            {"id":"gpt-future", "chatSupported":true},
            {"id":"gpt-4.1-2025-04-14", "chatSupported":true},
            {"id":"text-embedding-3-small"},
            {"id":"gpt-4.1-mini-audio-preview"}
        ]));
        normalize_models(
            &mut catalog,
            Some(bridge_runtime_protocol::ModelVendor::Openai),
        );
        let rows = serde_json::to_value(&catalog).unwrap();
        let rows = rows["models"]["items"].as_array().unwrap();
        assert_eq!(rows.len(), 9);
        assert!(rows[..5].iter().all(|m| m.get("pickerEnabled").is_none()));
        assert!(rows[5..].iter().all(|m| m["pickerEnabled"] == false));
        assert_eq!(rows[5]["chatSupported"], true);
        assert_eq!(rows[5]["id"], "gpt-future");
    }

    #[test]
    fn catalog_policy_does_not_promote_explicit_exclusions_or_invent_entries() {
        let mut catalog = listed(serde_json::json!([
            {"id":"gpt-4.1", "pickerEnabled":false, "chatSupported":true},
            {"id":"gpt-4o", "chatSupported":false}
        ]));
        let original = serde_json::to_value(&catalog).unwrap();
        normalize_models(
            &mut catalog,
            Some(bridge_runtime_protocol::ModelVendor::Openai),
        );
        assert_eq!(serde_json::to_value(&catalog).unwrap(), original);
        let mut empty = listed(serde_json::json!([]));
        normalize_models(
            &mut empty,
            Some(bridge_runtime_protocol::ModelVendor::Openai),
        );
        assert_eq!(empty, listed(serde_json::json!([])));
    }

    #[test]
    fn catalog_anthropic_policy_uses_owned_vendor_not_guessed_id_vendor() {
        let rows = serde_json::json!([
            {"id":"claude-sonnet-5-5", "chatSupported":true},
            {"id":"claude-next"}, {"id":"unclassified", "chatSupported":true},
            {"id":"claude-disabled", "pickerEnabled":false},
            {"id":"claude-embedding", "chatSupported":false}
        ]);
        let mut catalog = listed(rows.clone());
        normalize_models(
            &mut catalog,
            Some(bridge_runtime_protocol::ModelVendor::Anthropic),
        );
        assert_eq!(
            serde_json::to_value(&catalog).unwrap()["models"]["items"],
            serde_json::json!([
                {"id":"claude-sonnet-5-5", "chatSupported":true},
                {"id":"claude-next"}, {"id":"unclassified", "chatSupported":true, "pickerEnabled":false},
                {"id":"claude-disabled", "pickerEnabled":false},
                {"id":"claude-embedding", "chatSupported":false}
            ])
        );
        let mut copilot = listed(rows.clone());
        normalize_models(&mut copilot, None);
        assert_eq!(copilot, listed(rows));
    }

    #[test]
    fn catalog_policy_growth_keeps_models_when_the_combined_reply_was_at_the_byte_limit() {
        use bridge_runtime_protocol::{CatalogTool, CatalogToolKind, MAX_FILE};
        let mut catalog = listed(serde_json::json!([{"id":"gpt-future"}]));
        let mut tools: Vec<_> = (0..128)
            .map(|i| CatalogTool {
                kind: CatalogToolKind::Tool,
                name: format!("tool-{i}"),
                description: if i < 127 {
                    "d".repeat(8192)
                } else {
                    String::new()
                },
                available: None,
            })
            .collect();
        catalog.tools = CatalogSection::Ready {
            items: tools.clone(),
        };
        let remaining = MAX_FILE - serde_json::to_vec(&catalog).unwrap().len();
        assert!(remaining <= 8192);
        tools.last_mut().unwrap().description = "d".repeat(remaining);
        catalog.tools = CatalogSection::Ready { items: tools };
        assert_eq!(serde_json::to_vec(&catalog).unwrap().len(), MAX_FILE);
        assert!(catalog.valid());
        normalize_models(&mut catalog, Some(ModelVendor::Openai));
        assert!(catalog.valid());
        assert_eq!(catalog.tools, CatalogSection::Unavailable {});
        assert_eq!(
            serde_json::to_value(&catalog).unwrap()["models"]["items"],
            serde_json::json!([{"id":"gpt-future", "pickerEnabled":false}])
        );
    }

    #[test]
    fn catalog_policy_growth_marks_models_unavailable_only_if_they_alone_exceed_the_limit() {
        let mut catalog = listed(serde_json::json!((0..4096)
            .map(|i| serde_json::json!({"id":format!("{i:04}-{}", "x".repeat(225))}))
            .collect::<Vec<_>>()));
        assert!(catalog.valid());
        normalize_models(&mut catalog, Some(ModelVendor::Openai));
        assert!(catalog.valid());
        assert_eq!(catalog.models, CatalogSection::Unavailable {});
    }

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
