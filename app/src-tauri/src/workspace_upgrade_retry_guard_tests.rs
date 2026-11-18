use super::*;
use crate::runtime::credential_store::{CredentialStore, StoreError};
use bridge_runtime_protocol::CredentialPayload;
struct Vault(std::sync::Mutex<Vec<Uuid>>);
impl CredentialStore for Vault {
    fn save(&self, _: Uuid, _: &[u8]) -> Result<(), StoreError> {
        panic!("retry saved a secret")
    }
    fn load(&self, _: Uuid) -> Result<CredentialPayload, StoreError> {
        panic!("retry loaded a secret")
    }
    fn delete(&self, slot: Uuid) -> Result<(), StoreError> {
        self.0.lock().unwrap().push(slot);
        Ok(())
    }
}
#[test]
fn retry_archived_metadata_guards_every_slot_and_removal_stays_removed_through_restore() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let transaction = id(&scene);
    let root = scene.local.join("local-vm-v1");
    let b = installation(&root).unwrap().unwrap();
    let (model, account) = restore::destination_metadata(&scene, b.installation);
    let archived_slot = SetupStore::upgrade_slot(Some(&model)).unwrap().unwrap();
    retry(&scene, transaction, None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    let intent = owner.read_intent().unwrap().unwrap();
    assert_eq!(intent.incomplete[0].model, Some(model));
    assert_eq!(intent.incomplete[0].account, Some(account));
    assert!(owner.guard_model(Some(archived_slot), true).is_err());
    let vault = Vault(std::sync::Mutex::new(vec![]));
    owner
        .remove_credential(transaction, archived_slot, &vault)
        .unwrap();
    assert_eq!(*vault.0.lock().unwrap(), vec![archived_slot]);
    owner.restore(transaction).unwrap();
    assert!(owner.slot_removed(archived_slot).unwrap());
    assert!(owner.guard_model(Some(archived_slot), false).is_err());
    assert!(owner.guard_model(Some(archived_slot), true).is_err());
    assert_ne!(
        SetupStore::upgrade_slot(Some(
            &fs::read(scene.data.join("setup-v1/model-v1")).unwrap()
        ))
        .unwrap(),
        Some(archived_slot)
    );
}
#[test]
fn retry_revalidates_all_retained_incomplete_identities_before_further_mutation() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let transaction = id(&scene);
    let root = scene.local.join("local-vm-v1");
    let b = installation(&root).unwrap().unwrap();
    retry(&scene, transaction, None).unwrap();
    let retained = scene
        .local
        .join(format!("retained-incomplete-{}", b.installation));
    fs::rename(&retained, scene.local.join("kept-original")).unwrap();
    fs::create_dir(&retained).unwrap();
    let before = fs::read(scene.local.join(STORE).join("intent.json")).unwrap();
    assert!(retry(&scene, transaction, None).is_err());
    assert_eq!(
        fs::read(scene.local.join(STORE).join("intent.json")).unwrap(),
        before
    );
}
#[test]
fn retry_never_adopts_an_unrecorded_destination_after_creation_gap() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    assert!(scene.replace(Some("intent")).is_err());
    let transaction = id(&scene);
    assert!(retry(&scene, transaction, Some("destination-created")).is_err());
    let root = scene.local.join("local-vm-v1");
    let unknown = installation(&root).unwrap();
    assert!(retry(&scene, transaction, None).is_err());
    assert!(WorkspaceUpgrade::open(&scene.data, &scene.local)
        .unwrap()
        .restore(transaction)
        .is_err());
    assert_eq!(installation(&root).unwrap(), unknown);
}
#[test]
fn retry_preserves_legacy_removed_disposition_without_a_slot_marker() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let transaction = id(&scene);
    {
        let owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
        let mut intent = owner.read_intent().unwrap().unwrap();
        intent.credential = CredentialDisposition::Removed;
        owner.persist(&intent).unwrap();
    }
    retry(&scene, transaction, None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    assert!(matches!(
        owner.transfer_credential(transaction),
        Err(RuntimeError::RetainedState)
    ));
    owner.restore(transaction).unwrap();
    assert!(!scene.data.join("setup-v1/model-v1").exists());
    assert!(!scene.data.join("setup-v1/model-account-v1").exists());
}

#[test]
fn retry_ledger_limit_still_leaves_room_to_restore_the_current_metadata() {
    fn padded_metadata(scene: &Scene) {
        let destination = installation(&scene.local.join("local-vm-v1"))
            .unwrap()
            .unwrap();
        let (mut model, _) = restore::destination_metadata(scene, destination.installation);
        model.resize(16384, b' ');
        fs::write(scene.data.join("setup-v1/model-v1"), model).unwrap();
    }
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let transaction = id(&scene);
    padded_metadata(&scene);
    retry(&scene, transaction, None).unwrap();
    padded_metadata(&scene);
    let before = fs::read(scene.local.join(STORE).join("intent.json")).unwrap();
    let canonical = installation(&scene.local.join("local-vm-v1")).unwrap();
    assert!(matches!(
        retry(&scene, transaction, None),
        Err(RuntimeError::RetainedState)
    ));
    assert_eq!(
        fs::read(scene.local.join(STORE).join("intent.json")).unwrap(),
        before
    );
    assert_eq!(
        installation(&scene.local.join("local-vm-v1")).unwrap(),
        canonical
    );
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    assert_eq!(owner.restore(transaction).unwrap(), UpgradeStatus::Restored);
    assert_eq!(
        read_record(&scene.local.join("local-vm-v1"))
            .unwrap()
            .installation_id,
        scene.source_id
    );
}

#[test]
fn maximum_schema_one_source_and_destination_snapshots_fit_initial_replace_and_restore() {
    fn pad(scene: &Scene) {
        let model_path = scene.data.join("setup-v1/model-v1");
        let mut model = fs::read(&model_path).unwrap();
        model.resize(16384, b' ');
        fs::write(model_path, model).unwrap();
        let account_path = scene.data.join("setup-v1/model-account-v1");
        let mut account = fs::read(&account_path).unwrap();
        account.resize(256, b' ');
        fs::write(account_path, account).unwrap();
    }
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    pad(&scene);
    let source_model = fs::read(scene.data.join("setup-v1/model-v1")).unwrap();
    let source_account = fs::read(scene.data.join("setup-v1/model-account-v1")).unwrap();
    scene.replace(None).unwrap();
    let transaction = id(&scene);
    let b = installation(&scene.local.join("local-vm-v1"))
        .unwrap()
        .unwrap();
    restore::destination_metadata(&scene, b.installation);
    pad(&scene);
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    assert_eq!(owner.restore(transaction).unwrap(), UpgradeStatus::Restored);
    assert_eq!(
        fs::read(scene.data.join("setup-v1/model-v1")).unwrap(),
        source_model
    );
    assert_eq!(
        fs::read(scene.data.join("setup-v1/model-account-v1")).unwrap(),
        source_account
    );
}

#[test]
fn retention_target_collision_does_not_publish_an_unrecoverable_entry() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let transaction = id(&scene);
    let b = installation(&scene.local.join("local-vm-v1"))
        .unwrap()
        .unwrap();
    let collision = scene
        .local
        .join(format!("retained-incomplete-{}", b.installation));
    fs::create_dir(&collision).unwrap();
    fs::write(collision.join("keep"), b"unexpected retained files").unwrap();
    let before = fs::read(scene.local.join(STORE).join("intent.json")).unwrap();
    assert!(matches!(
        retry(&scene, transaction, None),
        Err(RuntimeError::RetainedState)
    ));
    assert_eq!(
        fs::read(scene.local.join(STORE).join("intent.json")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(collision.join("keep")).unwrap(),
        b"unexpected retained files"
    );
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    assert_eq!(owner.restore(transaction).unwrap(), UpgradeStatus::Restored);
}

#[test]
fn oversized_retention_record_cannot_replace_the_last_readable_intent() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    let mut intent = owner.read_intent().unwrap().unwrap();
    let before = fs::read(scene.local.join(STORE).join("intent.json")).unwrap();
    // The existing reader has a 128 KiB boundary. A growing retention ledger
    // must fail publication before replacing the last recoverable intent.
    intent.account = Some(vec![1; 128 * 1024]);
    assert!(owner.persist(&intent).is_err());
    assert_eq!(
        fs::read(scene.local.join(STORE).join("intent.json")).unwrap(),
        before
    );
}
