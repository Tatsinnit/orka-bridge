use super::*;
use kube::config::Kubeconfig;
use serde_json::json;
use std::sync::Arc;

#[test]
fn current_context_is_offline_metadata_not_an_implicit_selection() {
    for (current, expected) in [
        (None, [false, false]),
        (Some("missing"), [false, false]),
        (Some("other"), [false, true]),
    ] {
        let mut config: Kubeconfig = serde_json::from_value(json!({
            "contexts": [{"name": "team", "context": {"cluster": "fixture"}},
                         {"name": "other", "context": {"cluster": "fixture"}}]
        }))
        .unwrap();
        config.current_context = current.map(String::from);
        let client = ClusterClient::new(config.clone(), Arc::new(PodOrkaRead));
        let contexts = client.contexts().unwrap();
        for (index, current) in expected.into_iter().enumerate() {
            let view = serde_json::to_value(&contexts[index]).unwrap();
            assert_eq!(view["current"], current);
            assert_eq!(view.as_object().unwrap().len(), 3);
        }
        assert_eq!(contexts[0].name, "team");
        assert_eq!(contexts[1].name, "other");
        assert_eq!(client.contexts().unwrap()[0].id, contexts[0].id);
        let reloaded = ClusterClient::new(config, Arc::new(PodOrkaRead));
        assert_ne!(reloaded.contexts().unwrap()[0].id, contexts[0].id);
    }
}
