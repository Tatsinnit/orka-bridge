//! Read-only Designer choices from the already-confirmed cluster and AI connection.
use super::{
    client::{continuation, items},
    *,
};
use bridge_runtime_protocol::{
    CatalogModel, CatalogSection, CatalogTool, CatalogToolKind, WorkspaceCatalog, MAX_CATALOG_ITEMS,
};
use std::collections::HashSet;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogReply {
    pub default_model: Option<String>,
    #[serde(flatten)]
    pub catalog: WorkspaceCatalog,
}
fn provider_model(
    items: &[Value],
    namespace: &str,
    selected: &str,
) -> Result<Option<String>, AccessOutcome> {
    let mut found = None;
    for item in items {
        if item["metadata"]["namespace"] != namespace {
            return Err(AccessOutcome::Unavailable);
        }
        let name = item["metadata"]["name"]
            .as_str()
            .ok_or(AccessOutcome::Unavailable)?;
        if name == selected {
            if found.is_some() {
                return Err(AccessOutcome::Unavailable);
            }
            let model = match item["spec"].get("defaultModel") {
                None => None,
                Some(Value::String(value)) if value.is_empty() => None,
                Some(Value::String(value)) => Some(value.clone()),
                _ => return Err(AccessOutcome::Unavailable),
            };
            found = Some(model);
        }
    }
    found.ok_or(AccessOutcome::Unavailable)
}
fn catalog_tools(items: &[Value], namespace: &str) -> Result<Vec<CatalogTool>, AccessOutcome> {
    items
        .iter()
        .map(|item| {
            let kind = match item["builtin"].as_bool() {
                Some(true) => CatalogToolKind::Builtin,
                Some(false) if item["namespace"] == namespace => CatalogToolKind::Tool,
                _ => return Err(AccessOutcome::Unavailable),
            };
            let description = match item.get("description") {
                None => String::new(),
                Some(Value::String(value)) => value.clone(),
                _ => return Err(AccessOutcome::Unavailable),
            };
            let available = match item.get("available") {
                None => None,
                Some(Value::Bool(value)) => Some(*value),
                _ => return Err(AccessOutcome::Unavailable),
            };
            Ok(CatalogTool {
                kind,
                name: item["name"]
                    .as_str()
                    .ok_or(AccessOutcome::Unavailable)?
                    .into(),
                description,
                available,
            })
        })
        .collect()
}
fn reply(
    model: Result<Option<String>, AccessOutcome>,
    tools: Result<Vec<CatalogTool>, AccessOutcome>,
) -> CatalogReply {
    let mut catalog = WorkspaceCatalog {
        models: CatalogSection::Unavailable {},
        tools: CatalogSection::Unavailable {},
    };
    let mut default_model = None;
    if let Ok(model) = model {
        catalog.models = CatalogSection::Ready {
            items: model
                .iter()
                .map(|id| CatalogModel {
                    id: id.clone(),
                    picker_enabled: None,
                    chat_supported: None,
                })
                .collect(),
        };
        if catalog.valid() {
            default_model = model;
        } else {
            catalog.models = CatalogSection::Unavailable {};
        }
    }
    if let Ok(items) = tools {
        catalog.tools = CatalogSection::Ready { items };
        if !catalog.valid() {
            catalog.tools = CatalogSection::Unavailable {};
        }
    }
    CatalogReply {
        default_model,
        catalog,
    }
}
impl RemoteAccess {
    async fn catalog_pages(
        &self,
        stream: &dyn OrkaRead,
        resource: OrkaResource,
    ) -> Result<Vec<Value>, AccessOutcome> {
        let mut all = Vec::new();
        let mut after = None;
        let mut cursors = HashSet::new();
        let mut bytes = 0;
        // Orka returns at most 100 items per ordinary page; cap even empty-page chains.
        for _ in 0..64 {
            let page = self.read_via(stream, resource, after.as_deref()).await?;
            bytes += serde_json::to_vec(&page)
                .map_err(|_| AccessOutcome::Unavailable)?
                .len();
            if bytes > 1024 * 1024 {
                return Err(AccessOutcome::Unavailable);
            }
            all.extend(items(&page)?.iter().cloned());
            if all.len() > MAX_CATALOG_ITEMS {
                return Err(AccessOutcome::Unavailable);
            }
            after = continuation(&page)?;
            if after.is_none() {
                return Ok(all);
            }
            if !cursors.insert(after.clone()) {
                return Err(AccessOutcome::Unavailable);
            }
        }
        Err(AccessOutcome::Unavailable)
    }
    pub(super) async fn designer_catalog(
        &self,
        stream: &dyn OrkaRead,
        provider: ProviderId,
    ) -> Result<CatalogReply, AccessOutcome> {
        let selected = self
            .providers()
            .iter()
            .find(|p| p.id == provider)
            .ok_or(AccessOutcome::Unavailable)?;
        let model = self
            .catalog_pages(stream, OrkaResource::Providers)
            .await
            .and_then(|items| {
                provider_model(&items, &self.binding().watched_namespace, &selected.name)
            });
        let tools = self
            .catalog_pages(stream, OrkaResource::Tools)
            .await
            .and_then(|items| catalog_tools(&items, &self.binding().watched_namespace));
        Ok(reply(model, tools))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_the_selected_provider_supplies_a_configured_default() {
        let items = vec![
            json!({"metadata":{"name":"other","namespace":"agents"},"spec":{"defaultModel":"wrong"}}),
            json!({"metadata":{"name":"selected","namespace":"agents"},"spec":{"defaultModel":"exact-model"}}),
        ];
        assert_eq!(
            provider_model(&items, "agents", "selected")
                .unwrap()
                .as_deref(),
            Some("exact-model")
        );
        assert!(provider_model(&items, "agents", "missing").is_err());
        assert!(provider_model(&items, "different", "selected").is_err());
        assert!(
            provider_model(&[items[1].clone(), items[1].clone()], "agents", "selected").is_err()
        );
    }
    #[test]
    fn absent_default_is_empty_not_an_invented_model_name() {
        let items = vec![json!({"metadata":{"name":"selected","namespace":"agents"},"spec":{}})];
        let reply = reply(provider_model(&items, "agents", "selected"), Ok(vec![]));
        assert!(reply.default_model.is_none());
        assert_eq!(
            reply.catalog.models,
            CatalogSection::Ready { items: vec![] }
        );
    }
    #[test]
    fn tools_keep_exact_kind_description_and_watched_namespace() {
        let items = vec![
            json!({"name":"web_fetch","builtin":true,"description":"Fetch","url":"private"}),
            json!({"name":"custom","builtin":false,"namespace":"agents","description":"Custom"}),
        ];
        let tools = catalog_tools(&items, "agents").unwrap();
        assert_eq!(tools[0].kind, CatalogToolKind::Builtin);
        assert_eq!(tools[1].kind, CatalogToolKind::Tool);
        assert_eq!(tools[1].description, "Custom");
        assert!(!serde_json::to_string(&tools).unwrap().contains("private"));
        assert!(catalog_tools(&items, "wrong").is_err());
        assert!(catalog_tools(&[json!({"name":"bad","builtin":"true"})], "agents").is_err());
    }
    #[test]
    fn malformed_sections_fail_independently_and_never_export_partial_items() {
        let good = catalog_tools(&[json!({"name":"web_fetch","builtin":true})], "agents");
        let invalid = reply(Ok(Some("auto".into())), good);
        assert_eq!(invalid.catalog.models, CatalogSection::Unavailable {});
        assert!(matches!(
            invalid.catalog.tools,
            CatalogSection::Ready { .. }
        ));
        let bad = catalog_tools(
            &[
                json!({"name":"a","builtin":true}),
                json!({"name":"a","builtin":true}),
            ],
            "agents",
        );
        let invalid = reply(Ok(Some("valid".into())), bad);
        assert!(matches!(
            invalid.catalog.models,
            CatalogSection::Ready { .. }
        ));
        assert_eq!(invalid.catalog.tools, CatalogSection::Unavailable {});
        let invalid = reply(Err(AccessOutcome::Unavailable), Err(AccessOutcome::Denied));
        assert!(invalid.default_model.is_none());
        assert_eq!(invalid.catalog.models, CatalogSection::Unavailable {});
        assert_eq!(invalid.catalog.tools, CatalogSection::Unavailable {});
    }
}
