//! Inventory persistence at the confirmed-native-access boundary; no server or credential read.
use super::*;
use crate::overview_places::{Inventory, Place};

fn confirmed() -> Arc<RemoteAccess> {
    Arc::new(RemoteAccess {
        creator: Client::try_from(kube::Config::new("http://127.0.0.1:1".parse().unwrap()))
            .unwrap(),
        bearer: "never-persist-this-credential".into(),
        creator_bearer: "nor-the-creator".into(),
        expiry: None,
        binding: TargetBinding {
            cluster_identity: "a".repeat(64),
            context_id: ContextId(Uuid::new_v4()),
            context_name: "team".into(),
            server_address: "http://127.0.0.1:1".into(),
            service_name: "orka".into(),
            service_namespace: "control".into(),
            service_uid: "service".into(),
            deployment_name: "controller".into(),
            deployment_namespace: "control".into(),
            deployment_uid: "deployment".into(),
            watched_namespace: "agents".into(),
            image: "never-persist-image".into(),
            api_port: 8080,
        },
        orka: Arc::new(PodOrkaRead),
        providers: vec![ProviderView {
            id: ProviderId(Uuid::new_v4()),
            name: "never-persist-provider".into(),
            ready: true,
        }],
    })
}
#[tokio::test]
async fn confirmation_persists_check_time_and_duplicate_identity_replaces_without_restoring_access()
{
    let root = tempfile::tempdir().unwrap();
    let inventory = Inventory::with_storage(Some(root.path().into()));
    let access = confirmed();
    let key = Place::new(access.binding().clone()).key;
    inventory.remember(access);
    let checked = inventory.snapshot()[0].checked_at.unwrap();
    assert!(inventory.snapshot()[0].observed_at.is_none());
    assert!(checked > 0);
    inventory.remember(confirmed());
    assert_eq!(inventory.snapshot().len(), 1);
    let restored = Inventory::with_storage(Some(root.path().into()));
    let rows = restored.refresh().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, key);
    assert_eq!(rows[0].checked_at, inventory.snapshot()[0].checked_at);
    assert!(rows[0].observed_at.is_none());
    assert!(rows[0].stale);
    assert!(rows[0].agents.is_empty());
    let bytes = std::fs::read_to_string(root.path().join("clusters-v1.json")).unwrap();
    for forbidden in [
        "127.0.0.1",
        "credential",
        "creator",
        "image",
        "provider",
        "8080",
        "contextId",
        "controller",
    ] {
        assert!(!bytes.contains(forbidden), "persisted {forbidden}");
    }
    inventory.forget(&key).unwrap();
    assert!(Inventory::with_storage(Some(root.path().into()))
        .snapshot()
        .is_empty());
    inventory.remember(confirmed());
    inventory.invalidate(None);
    assert_eq!(
        Inventory::with_storage(Some(root.path().into()))
            .snapshot()
            .len(),
        1
    );
}
#[tokio::test]
async fn confirmation_after_corrupt_load_keeps_live_row_without_overwriting_prior_bytes() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("clusters-v1.json");
    std::fs::write(&file, b"corrupt original").unwrap();
    let inventory = Inventory::with_storage(Some(root.path().into()));
    inventory.remember(confirmed());
    assert_eq!(inventory.persistence_state(), "unavailable");
    assert_eq!(inventory.snapshot().len(), 1);
    assert_eq!(std::fs::read(&file).unwrap(), b"corrupt original");
    assert!(inventory.forget(&inventory.snapshot()[0].key).is_err());
}
#[tokio::test]
async fn parallel_confirmations_save_in_inventory_order_not_outdated_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let inventory = Arc::new(Inventory::with_storage(Some(root.path().into())));
    let accesses: Vec<_> = (0..8)
        .map(|i| {
            let mut access = confirmed();
            Arc::get_mut(&mut access).unwrap().binding.service_uid = format!("service-{i}");
            access
        })
        .collect();
    let handles: Vec<_> = accesses
        .into_iter()
        .map(|access| {
            let inventory = inventory.clone();
            std::thread::spawn(move || inventory.remember(access))
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    assert_eq!(inventory.persistence_state(), "complete");
    assert_eq!(
        Inventory::with_storage(Some(root.path().into()))
            .snapshot()
            .len(),
        8
    );
}
