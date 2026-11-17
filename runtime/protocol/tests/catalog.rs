use bridge_runtime_protocol::*;
use serde_json::json;

#[test]
fn catalog_is_a_fixed_action_not_an_http_or_scope_escape() {
    assert_eq!(
        serde_json::from_value::<EngineActionWire>(json!({"type":"workspaceCatalog"})).unwrap(),
        EngineActionWire::WorkspaceCatalog {}
    );
    for key in ["provider", "path", "namespace", "url", "token"] {
        let mut v = json!({"type":"workspaceCatalog"});
        v[key] = json!("synthetic-private-value");
        assert!(serde_json::from_value::<EngineActionWire>(v).is_err());
    }
    assert!(OrkaRequest {
        path: "tools".into(),
        after: None,
        body: None
    }
    .validate()
    .is_err());
}
#[test]
fn catalog_keeps_unknown_capabilities_and_empty_distinct_from_unavailable() {
    let value = json!({"models":{"state":"ready","items":[{"id":"model:exact"}]},"tools":{"state":"unavailable"}});
    let catalog: WorkspaceCatalog = serde_json::from_value(value.clone()).unwrap();
    assert!(catalog.valid());
    assert_eq!(serde_json::to_value(catalog).unwrap(), value);
    let empty: WorkspaceCatalog = serde_json::from_value(
        json!({"models":{"state":"ready","items":[]},"tools":{"state":"ready","items":[]}}),
    )
    .unwrap();
    assert!(empty.valid());
}
#[test]
fn catalog_rejects_bad_ids_duplicates_and_unknown_projection_fields() {
    let fixture = json!({"models":{"state":"ready","items":[{"id":"m"}]},"tools":{"state":"ready","items":[{"kind":"tool","name":"custom","description":"Description","available":false}]}});
    for id in ["", "auto", "has space", "é", &"m".repeat(257)] {
        let mut v = fixture.clone();
        v["models"]["items"][0]["id"] = json!(id);
        assert!(!serde_json::from_value::<WorkspaceCatalog>(v)
            .unwrap()
            .valid());
    }
    let mut duplicates = fixture.clone();
    duplicates["models"]["items"] = json!([{"id":"m"},{"id":"m"}]);
    assert!(!serde_json::from_value::<WorkspaceCatalog>(duplicates)
        .unwrap()
        .valid());
    for (field, val) in [
        ("url", json!("http://private.invalid")),
        ("token", json!("synthetic-credential")),
    ] {
        let mut v = fixture.clone();
        v["tools"]["items"][0][field] = val;
        assert!(serde_json::from_value::<WorkspaceCatalog>(v).is_err());
    }
    assert!(serde_json::from_value::<WorkspaceCatalog>(
        json!({"models":{"state":"unavailable","items":[]},"tools":{"state":"unavailable"}})
    )
    .is_err());
}
