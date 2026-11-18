use super::*;

fn prepared(target: ImageTarget) -> (Scene, WorkspaceUpgrade) {
    let mut scene = Scene::new(target);
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (_send, closed) = watch::channel(false);
    owner
        .replace_with(
            &scene.source,
            vec![2],
            &closed,
            runtime.handle(),
            || scene.target.take().ok_or(RuntimeError::Unavailable),
            |_| Ok(()),
            available_space,
        )
        .unwrap();
    (scene, owner)
}

#[test]
fn creator_journal_admits_only_exact_prepared_destination_without_reopening_authority() {
    for target in [ImageTarget::WindowsAmd64, ImageTarget::MacosArm64] {
        let (scene, mut owner) = prepared(target);
        let journal = owner.take_destination().unwrap();
        let destination = journal.record().installation_id;
        let root = scene.local.join("local-vm-v1");
        drop(owner);
        assert!(
            admit_prepared(&scene.data, &root, &journal).is_ok(),
            "a creator-owned B must boot"
        );
        assert!(journal.require_fresh_creation().is_ok());
        assert!(
            admit(&scene.data, &root).is_err(),
            "ordinary admission must not bypass live custody"
        );
        let other = Scene::new(target);
        assert!(
            admit_prepared(&other.data, &other.local.join("local-vm-v1"), &journal).is_err(),
            "another root cannot borrow B authority"
        );
        drop(journal);
        assert!(
            admit(&scene.data, &root).is_err(),
            "persisted unconsumed B must remain recovery-only"
        );
        let reopened = Journal::open(&root).unwrap();
        assert_eq!(reopened.record().installation_id, destination);
        assert!(
            admit_prepared(&scene.data, &root, &reopened).is_err(),
            "opening B never recreates formatting authority"
        );
    }
}

#[test]
fn eligibility_refuses_malformed_metadata_and_distinguishes_every_saved_phase() {
    for target in [ImageTarget::WindowsAmd64, ImageTarget::MacosArm64] {
        for (phase, expected) in [
            ("stored", "eligible"),
            ("connected", "eligible"),
            ("storing", "attention"),
            ("pending", "attention"),
            ("cleared", "attention"),
        ] {
            let scene = Scene::new(target);
            credentials::phase(&scene, phase);
            let dto = upgrade_status(&scene.data, &scene.local, Some(&scene.source));
            assert_eq!(dto.state, "eligible");
            assert_eq!(dto.credential, expected, "{phase}");
            assert!(dto.can_replace);
            // Limits are admission ceilings, not installed-image sizes. Count
            // actual target cache + private files, including the fresh state disk.
            let (profile, host) = match &scene.source {
                ImageSource::Pinned { profile, host } => (profile, host),
                #[cfg(debug_assertions)]
                ImageSource::Development => unreachable!(),
            };
            let image =
                crate::runtime::manifest::resolve_image(&profile.pin, &profile.manifest, host)
                    .unwrap();
            let boot: u64 = image
                .assets()
                .iter()
                .filter(|a| !matches!(a.role(), ImageAssetRole::Chunk(_)))
                .map(|a| a.bytes())
                .sum();
            let required = image.compressed_bytes()
                + 2 * image.disk_bytes()
                + 2 * boot
                + bridge_runtime_protocol::STATE_BYTES;
            assert_eq!(dto.additional_bytes, Some(required));
        }
        let scene = Scene::new(target);
        let path = scene.data.join("setup-v1/model-v1");
        fs::write(&path, b"unrecognized record kept intact").unwrap();
        let before = fs::read(&path).unwrap();
        let dto = upgrade_status(&scene.data, &scene.local, Some(&scene.source));
        assert!(
            !dto.can_replace,
            "unreadable metadata is not absent metadata"
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn transaction_projection_offers_explicit_credential_reuse_and_interrupted_forward_retry() {
    for phase in ["stored", "connected", "storing", "pending", "cleared"] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        credentials::phase(&scene, phase);
        scene.replace(None).unwrap();
        let owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
        let dto = owner.status_dto(Some(&scene.source));
        assert_eq!(dto.state, "incomplete");
        assert!(
            dto.can_replace,
            "the incomplete transaction must offer retry, not only Restore"
        );
        assert!(dto.can_restore);
        assert_eq!(
            dto.credential,
            if matches!(phase, "stored" | "connected") {
                "eligible"
            } else {
                "attention"
            }
        );
    }
}

#[test]
fn restored_transaction_warns_that_another_setup_cannot_reopen_retained_b() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    let transaction = owner.read_intent().unwrap().unwrap().transaction;
    owner.restore(transaction).unwrap();
    let dto = owner.status_dto(Some(&scene.source));
    assert_eq!(dto.state, "restored");
    assert!(dto.repeated);
    assert!(dto.can_replace);
    assert!(!dto.can_restore);
}

#[test]
fn interrupted_restore_projection_offers_exact_transaction_continuation() {
    for target in [ImageTarget::WindowsAmd64, ImageTarget::MacosArm64] {
        let mut scene = Scene::new(target);
        scene.replace(None).unwrap();
        let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
        let intent = owner.read_intent().unwrap().unwrap();
        let root = scene.local.join("local-vm-v1");
        let retained_a = scene.local.join(&intent.retained);
        let a = installation(&retained_a).unwrap();
        let b = installation(&root).unwrap();
        let a_record = fs::read(retained_a.join("record.json")).unwrap();
        let b_record = fs::read(root.join("record.json")).unwrap();
        assert!(matches!(
            owner.restore_with(intent.transaction, |point| {
                if point == "restore-intent" {
                    Err(RuntimeError::Closed)
                } else {
                    Ok(())
                }
            }),
            Err(RuntimeError::Closed)
        ));
        drop(owner);
        let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
        let dto = owner.status_dto(Some(&scene.source));
        assert_eq!(dto.state, "restoring");
        assert_eq!(dto.transaction, Some(intent.transaction));
        assert!(dto.can_restore);
        assert_eq!(
            owner.restore(intent.transaction).unwrap(),
            UpgradeStatus::Restored
        );
        let dto = owner.status_dto(Some(&scene.source));
        assert_eq!(dto.transaction, Some(intent.transaction));
        assert_eq!(dto.state, "restored");
        assert!(!dto.can_restore);
        let retained_b = scene
            .local
            .join(format!("retained-new-{}", intent.transaction));
        assert_eq!(installation(&root).unwrap(), a);
        assert_eq!(installation(&retained_b).unwrap(), b);
        assert_eq!(fs::read(root.join("record.json")).unwrap(), a_record);
        assert_eq!(fs::read(retained_b.join("record.json")).unwrap(), b_record);
    }
}

#[test]
fn forward_and_retry_publish_real_materialization_progress() {
    for retrying in [false, true] {
        let mut scene = Scene::new(ImageTarget::WindowsAmd64);
        if retrying {
            scene.replace(None).unwrap();
        }
        let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = events.clone();
        owner.set_progress(std::sync::Arc::new(move |event| {
            observed.lock().unwrap().push(event)
        }));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (_send, closed) = watch::channel(false);
        let cache = tempfile::tempdir().unwrap();
        if retrying {
            let transaction = owner.read_intent().unwrap().unwrap().transaction;
            owner
                .retry_with(
                    transaction,
                    &scene.source,
                    vec![2],
                    &closed,
                    runtime.handle(),
                    || {
                        PreparedImage::from_verified(fixture::verified_with_identity(
                            cache.path(),
                            ImageTarget::WindowsAmd64,
                            Some("new-state"),
                            Some("new-image"),
                        ))
                        .map_err(Into::into)
                    },
                    |_| Ok(()),
                    available_space,
                )
                .unwrap();
        } else {
            owner
                .replace_with(
                    &scene.source,
                    vec![2],
                    &closed,
                    runtime.handle(),
                    || scene.target.take().ok_or(RuntimeError::Unavailable),
                    |_| Ok(()),
                    available_space,
                )
                .unwrap();
        }
        assert!(events.lock().unwrap().iter().any(|event| matches!(event, ImageProgress::Materializing { bytes, total } if *bytes > 0 && bytes <= total)), "real materialization movement must reach the native progress observer, retry={retrying}");
    }
}

#[test]
fn transaction_mutations_reject_foreign_and_nil_ids_without_changing_records() {
    let mut scene = Scene::new(ImageTarget::WindowsAmd64);
    scene.replace(None).unwrap();
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local).unwrap();
    let before = fs::read(scene.local.join(STORE).join("intent.json")).unwrap();
    let canonical = installation(&scene.local.join("local-vm-v1")).unwrap();
    for foreign in [Uuid::nil(), Uuid::new_v4()] {
        assert!(matches!(
            owner.restore(foreign),
            Err(RuntimeError::RetainedState)
        ));
        assert!(matches!(
            owner.transfer_credential(foreign),
            Err(RuntimeError::RetainedState)
        ));
        assert!(matches!(
            owner.resolve_removal_slot(foreign),
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
    }
}
