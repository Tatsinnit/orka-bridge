use crate::{ModelSelection, MAX_FILE};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_CATALOG_ITEMS: usize = 4096;
pub const MAX_CATALOG_TEXT: usize = 4096;
pub const MAX_CATALOG_DESCRIPTION: usize = 8192;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum CatalogSection<T> {
    Ready { items: Vec<T> },
    Unavailable {},
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picker_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_supported: Option<bool>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CatalogToolKind {
    Builtin,
    Tool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogTool {
    pub kind: CatalogToolKind,
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceCatalog {
    pub models: CatalogSection<CatalogModel>,
    pub tools: CatalogSection<CatalogTool>,
}
impl WorkspaceCatalog {
    pub fn valid(&self) -> bool {
        let models = match &self.models {
            CatalogSection::Unavailable {} => true,
            CatalogSection::Ready { items } => {
                let mut ids = HashSet::new();
                items.len() <= MAX_CATALOG_ITEMS
                    && items.iter().all(|m| {
                        ModelSelection {
                            vendor: None,
                            provider: "copilot".into(),
                            model: m.id.clone(),
                        }
                        .valid()
                            && ids.insert(&m.id)
                    })
            }
        };
        let tools = match &self.tools {
            CatalogSection::Unavailable {} => true,
            CatalogSection::Ready { items } => {
                let mut ids = HashSet::new();
                items.len() <= MAX_CATALOG_ITEMS
                    && items.iter().all(|t| {
                        !t.name.is_empty()
                            && t.name.len() <= MAX_CATALOG_TEXT
                            && !t.name.chars().any(char::is_control)
                            && t.description.len() <= MAX_CATALOG_DESCRIPTION
                            && !t.description.contains('\0')
                            && ids.insert((t.kind, &t.name))
                    })
            }
        };
        models && tools && serde_json::to_vec(self).is_ok_and(|b| b.len() <= MAX_FILE)
    }
}
