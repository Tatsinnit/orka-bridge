use super::{failed, Orka, REQUEST_BUDGET};
use bridge_runtime_protocol::*;
use serde_json::Value;
use std::{collections::HashSet, io, time::Instant};

fn models(value: &Value) -> io::Result<Vec<CatalogModel>> {
    let rows = value["data"]
        .as_array()
        .filter(|a| a.len() <= MAX_CATALOG_ITEMS)
        .ok_or_else(failed)?;
    let mut ids = HashSet::new();
    rows.iter()
        .map(|row| {
            let id = row["id"].as_str().ok_or_else(failed)?.to_owned();
            if !(ModelSelection {
                vendor: None,
                provider: "copilot".into(),
                model: id.clone(),
            })
            .valid()
                || !ids.insert(id.clone())
            {
                return Err(failed());
            }
            let picker_enabled = optional_bool(row, "model_picker_enabled")?;
            let chat_supported = match row.get("supported_endpoints") {
                None | Some(Value::Null) => None,
                Some(Value::Array(endpoints)) if endpoints.len() <= 64 => {
                    let mut supported = false;
                    for e in endpoints {
                        let e = e
                            .as_str()
                            .filter(|s| s.len() <= MAX_CATALOG_TEXT)
                            .ok_or_else(failed)?;
                        // Vekil also serves Chat through its Responses adapter.
                        supported |= matches!(
                            e,
                            "/chat/completions"
                                | "/v1/chat/completions"
                                | "/responses"
                                | "/v1/responses"
                                | "/v1/messages"
                        );
                    }
                    Some(supported)
                }
                _ => return Err(failed()),
            };
            Ok(CatalogModel {
                id,
                picker_enabled,
                chat_supported,
            })
        })
        .collect()
}
fn optional_bool(v: &Value, key: &str) -> io::Result<Option<bool>> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        _ => Err(failed()),
    }
}
fn tools(
    mut fetch: impl FnMut(&str) -> io::Result<Value>,
    deadline: Instant,
) -> io::Result<Vec<CatalogTool>> {
    let mut cursor = String::new();
    let mut cursors = HashSet::new();
    let mut ids = HashSet::new();
    let mut items = Vec::new();
    let mut bytes = 0;
    for _ in 0..64 {
        if Instant::now() >= deadline || !cursors.insert(cursor.clone()) {
            return Err(failed());
        }
        let page = fetch(&cursor)?;
        let rows = page["items"].as_array().ok_or_else(failed)?;
        if rows.len() > MAX_CATALOG_ITEMS.saturating_sub(items.len()) {
            return Err(failed());
        }
        for row in rows {
            let builtin = row["builtin"].as_bool().ok_or_else(failed)?;
            if builtin && !cursor.is_empty() {
                return Err(failed());
            }
            let name = row["name"].as_str().ok_or_else(failed)?.to_owned();
            let description = row["description"].as_str().ok_or_else(failed)?.to_owned();
            let kind = if builtin {
                CatalogToolKind::Builtin
            } else {
                CatalogToolKind::Tool
            };
            if name.is_empty()
                || name.len() > MAX_CATALOG_TEXT
                || name.chars().any(char::is_control)
                || description.len() > MAX_CATALOG_DESCRIPTION
                || description.contains('\0')
                || !ids.insert((kind, name.clone()))
            {
                return Err(failed());
            }
            let item = CatalogTool {
                kind,
                name,
                description,
                available: optional_bool(row, "available")?,
            };
            bytes += serde_json::to_vec(&item)?.len() + 1;
            if bytes > MAX_FILE {
                return Err(failed());
            }
            items.push(item);
        }
        let metadata = page["metadata"].as_object().ok_or_else(failed)?;
        let next = match metadata.get("continue") {
            None | Some(Value::Null) => "",
            Some(Value::String(s)) if s.len() <= MAX_CATALOG_TEXT => s,
            _ => return Err(failed()),
        };
        if Instant::now() >= deadline {
            return Err(failed());
        }
        if next.is_empty() {
            return Ok(items);
        }
        cursor = next.to_owned();
    }
    Err(failed())
}
fn fit(c: &mut WorkspaceCatalog) {
    if serde_json::to_vec(c).map_or(true, |b| b.len() > MAX_FILE) {
        c.tools = CatalogSection::Unavailable {};
    }
    if serde_json::to_vec(c).map_or(true, |b| b.len() > MAX_FILE) {
        c.models = CatalogSection::Unavailable {};
    }
}
impl Orka {
    /// A fixed read of this lease's retained model route; no caller-selected HTTP paths.
    pub fn catalog(&mut self, provider: &str, vendor: Option<ModelVendor>) -> WorkspaceCatalog {
        self.catalog_until(provider, vendor, Instant::now() + REQUEST_BUDGET)
    }
    pub(super) fn catalog_until(
        &mut self,
        provider: &str,
        vendor: Option<ModelVendor>,
        deadline: Instant,
    ) -> WorkspaceCatalog {
        let models = (|| {
            if !(ModelSelection {
                vendor: None,
                provider: provider.into(),
                model: "catalog".into(),
            })
            .valid()
            {
                return Err(failed());
            }
            // Vekil drops Anthropic's cursor metadata: this is a bounded first page.
            let query = if vendor == Some(ModelVendor::Anthropic) {
                "?limit=1000"
            } else {
                ""
            };
            let path=format!("/api/v1/namespaces/{ORKA_NAMESPACE}/services/http:{provider}-vekil:1337/proxy/v1/models{query}");
            let raw = self.run(&["get", "--raw", &path], None, deadline)?;
            if Instant::now() >= deadline {
                return Err(failed());
            }
            models(&serde_json::from_slice::<Value>(&raw).map_err(|_| failed())?)
        })();
        let tools = tools(
            |cursor| {
                let reply = self.http(
                    reqwest::Method::GET,
                    "tools",
                    None,
                    None,
                    Some(cursor),
                    deadline,
                )?;
                if reply.status != 200 {
                    return Err(failed());
                }
                Ok(reply.body)
            },
            deadline,
        );
        let mut catalog = WorkspaceCatalog {
            models: models
                .map(|items| CatalogSection::Ready { items })
                .unwrap_or(CatalogSection::Unavailable {}),
            tools: tools
                .map(|items| CatalogSection::Ready { items })
                .unwrap_or(CatalogSection::Unavailable {}),
        };
        fit(&mut catalog);
        catalog
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;
    #[test]
    fn models_preserve_only_ids_and_optional_chat_capabilities() {
        let got = models(&json!({"data":[{"id":"m:exact","model_picker_enabled":false,"supported_endpoints":["/chat/completions"],"token":"synthetic-private","url":"private"},{"id":"other","supported_endpoints":["/embeddings"]},{"id":"unknown"}]})).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            json!([{"id":"m:exact","pickerEnabled":false,"chatSupported":true},{"id":"other","chatSupported":false},{"id":"unknown"}])
        );
        assert!(models(&json!({"data":[{"id":"auto"}]})).is_err());
        assert!(models(&json!({"data":[{"id":"m"},{"id":"m"}]})).is_err());
        assert!(models(&json!({"data":[{"id":"m","model_picker_enabled":"yes"}]})).is_err());
        assert!(models(&json!({"data":[]})).unwrap().is_empty());
    }
    #[test]
    fn responses_only_models_support_chat_through_vekil_conversion() {
        let got=models(&json!({"data":[{"id":"responses-only","supported_endpoints":["/responses"]},{"id":"responses-v1","supported_endpoints":["/v1/responses"]}]})).unwrap();
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            json!([{"id":"responses-only","chatSupported":true},{"id":"responses-v1","chatSupported":true}])
        );
    }
    #[test]
    fn messages_models_are_chat_but_explicit_non_chat_endpoints_are_not() {
        let got = models(&json!({"data":[
            {"id":"claude-sonnet-5-5","supported_endpoints":["/v1/messages"]},
            {"id":"not-chat","supported_endpoints":["/embeddings"]}
        ]}))
        .unwrap();
        assert_eq!(got[0].chat_supported, Some(true));
        assert_eq!(got[1].chat_supported, Some(false));
    }
    #[test]
    fn tools_follow_pages_without_forwarding_specs_or_urls() {
        let mut seen = Vec::new();
        let got=tools(|cursor| {
            seen.push(cursor.to_owned());
            Ok(if cursor.is_empty() {json!({"items":[{"builtin":true,"name":"web_fetch","description":"Fetch","parameters":{"token":"private"}}],"metadata":{"continue":"a+/="}})} else {json!({"items":[{"builtin":false,"name":"custom","description":"Custom","available":false,"url":"http://private.invalid"}],"metadata":{}})})
        }, Instant::now()+Duration::from_secs(2)).unwrap();
        assert_eq!(seen, ["", "a+/="]);
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            json!([{"kind":"builtin","name":"web_fetch","description":"Fetch"},{"kind":"tool","name":"custom","description":"Custom","available":false}])
        );
    }
    #[test]
    fn loops_duplicates_and_expired_budget_are_not_complete_catalogs() {
        let page = json!({"items":[],"metadata":{"continue":"repeat"}});
        assert!(tools(
            |_| Ok(page.clone()),
            Instant::now() + Duration::from_secs(1)
        )
        .is_err());
        let page = json!({"items":[{"builtin":true,"name":"x","description":"x"}],"metadata":{"continue":"next"}});
        assert!(tools(
            |_| Ok(page.clone()),
            Instant::now() + Duration::from_secs(1)
        )
        .is_err());
        assert!(tools(
            |_| panic!("expired budget must not issue a request"),
            Instant::now()
        )
        .is_err());
    }
    #[test]
    fn missing_tool_pagination_metadata_is_not_a_complete_empty_catalog() {
        assert!(tools(
            |_| Ok(json!({"items":[]})),
            Instant::now() + Duration::from_secs(1)
        )
        .is_err());
    }
    #[test]
    fn oversized_combined_result_discards_tools_first() {
        let mut c = WorkspaceCatalog {
            models: CatalogSection::Ready {
                items: vec![CatalogModel {
                    id: "m".into(),
                    picker_enabled: None,
                    chat_supported: None,
                }],
            },
            tools: CatalogSection::Ready {
                items: (0..200)
                    .map(|i| CatalogTool {
                        kind: CatalogToolKind::Tool,
                        name: format!("tool-{i}"),
                        description: "d".repeat(8192),
                        available: None,
                    })
                    .collect(),
            },
        };
        fit(&mut c);
        assert!(matches!(c.models, CatalogSection::Ready { .. }));
        assert_eq!(c.tools, CatalogSection::Unavailable {});
        assert!(c.valid());
    }
}
