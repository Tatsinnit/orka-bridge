use super::*;
use serde_json::json;
use std::sync::Mutex;

struct Read {
    pages: Vec<Result<Value, AccessOutcome>>,
    calls: Mutex<usize>,
}
impl OrkaRead for Read {
    fn get<'a>(
        &'a self,
        _: &'a Client,
        _: &'a TargetBinding,
        _: &'a str,
        resource: OrkaResource,
        _: &'a str,
        _: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, AccessOutcome>> + Send + 'a>> {
        Box::pin(async move {
            if resource == OrkaResource::Providers {
                return Ok(
                    json!({"items":[{"metadata":{"name":"selected","namespace":"agents"},"spec":{"defaultModel":"exact"}}],"metadata":{}}),
                );
            }
            let mut call = self.calls.lock().unwrap();
            let page = self.pages[(*call).min(self.pages.len() - 1)].clone();
            *call += 1;
            page
        })
    }
}
fn access(read: Arc<Read>) -> RemoteAccess {
    RemoteAccess {
        creator: Client::try_from(kube::Config::new("https://127.0.0.1:1".parse().unwrap()))
            .unwrap(),
        bearer: "fixture-bearer".into(),
        creator_bearer: "fixture-creator".into(),
        expiry: None,
        binding: TargetBinding {
            cluster_identity: "fixture-cluster".into(),
            context_id: ContextId(Uuid::new_v4()),
            context_name: "test".into(),
            server_address: "https://127.0.0.1:1".into(),
            service_name: "orka".into(),
            service_namespace: "control".into(),
            service_uid: "svc".into(),
            deployment_name: "orka".into(),
            deployment_namespace: "control".into(),
            deployment_uid: "dep".into(),
            watched_namespace: "agents".into(),
            image: "fixture-image".into(),
            api_port: 8080,
        },
        orka: read,
        providers: vec![ProviderView {
            id: ProviderId(Uuid::nil()),
            name: "selected".into(),
            ready: true,
        }],
    }
}
#[tokio::test]
async fn remote_catalog_pages_are_complete_and_never_partial_after_error_or_cycle() {
    use bridge_runtime_protocol::CatalogSection;
    for (pages, ready, count) in [
        (
            vec![
                Ok(json!({"items":[{"name":"one","builtin":true}],"metadata":{"continue":"next"}})),
                Ok(
                    json!({"items":[{"name":"two","builtin":false,"namespace":"agents"}],"metadata":{}}),
                ),
            ],
            true,
            2,
        ),
        (
            vec![
                Ok(json!({"items":[{"name":"one","builtin":true}],"metadata":{"continue":"next"}})),
                Err(AccessOutcome::Denied),
            ],
            false,
            2,
        ),
        (
            vec![Ok(json!({"items":[],"metadata":{"continue":"loop"}}))],
            false,
            2,
        ),
        (
            vec![Ok(json!({"items":[],"metadata":{"continue":42}}))],
            false,
            1,
        ),
        (
            vec![Ok(
                json!({"items":[{"name":"fixture-bearer","builtin":true}],"metadata":{}}),
            )],
            false,
            1,
        ),
        (
            vec![Ok(
                json!({"items":[{"name":"fixture-creator","builtin":true}],"metadata":{}}),
            )],
            false,
            1,
        ),
    ] {
        let read = Arc::new(Read {
            pages,
            calls: Mutex::new(0),
        });
        let access = access(read.clone());
        let result = access
            .designer_catalog(&*read, ProviderId(Uuid::nil()))
            .await
            .unwrap();
        assert_eq!(result.default_model.as_deref(), Some("exact"));
        if ready {
            assert!(
                matches!(result.catalog.tools, CatalogSection::Ready { ref items } if items.len() == 2)
            );
        } else {
            assert_eq!(result.catalog.tools, CatalogSection::Unavailable {});
        }
        assert_eq!(*read.calls.lock().unwrap(), count);
        assert!(access
            .designer_catalog(&*read, ProviderId(Uuid::new_v4()))
            .await
            .is_err());
    }
}
#[tokio::test]
async fn remote_catalog_total_budget_rejects_an_oversized_inventory() {
    use bridge_runtime_protocol::{CatalogSection, MAX_CATALOG_ITEMS};
    for page in [
        json!({"items":[{"name":"one","builtin":true,"description":"x".repeat(1024*1024)}],"metadata":{}}),
        json!({"items":vec![json!({"name":"one","builtin":true}); MAX_CATALOG_ITEMS+1],"metadata":{}}),
    ] {
        let read = Arc::new(Read {
            pages: vec![Ok(page)],
            calls: Mutex::new(0),
        });
        let result = access(read.clone())
            .designer_catalog(&*read, ProviderId(Uuid::nil()))
            .await
            .unwrap();
        assert_eq!(result.catalog.tools, CatalogSection::Unavailable {});
    }
}
#[tokio::test]
async fn catalog_command_rejects_unconfirmed_ids_without_reading_configuration() {
    use super::super::commands::ClusterSession;
    let session = ClusterSession::default();
    assert!(session
        .catalog(&Uuid::new_v4().to_string(), &Uuid::new_v4().to_string())
        .await
        .is_err());
}
