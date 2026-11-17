use super::*;
#[path = "workspace_upgrade_retry_guard_tests.rs"]
mod guards;

pub(super) fn id(scene: &Scene) -> Uuid {
    WorkspaceUpgrade::open(&scene.data, &scene.local)
        .unwrap()
        .read_intent()
        .unwrap()
        .unwrap()
        .transaction
}
pub(super) fn retry(
    scene: &Scene,
    transaction: Uuid,
    stop: Option<&str>,
) -> Result<UpgradeStatus, RuntimeError> {
    let mut owner = WorkspaceUpgrade::open(&scene.data, &scene.local)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (_send, closed) = watch::channel(false);
    let cache = tempfile::tempdir().unwrap();
    owner.retry_with(
        transaction,
        &scene.source,
        vec![2],
        &closed,
        runtime.handle(),
        || {
            PreparedImage::from_verified(fixture::verified_with_identity(
                cache.path(),
                match &scene.source {
                    ImageSource::Pinned { host, .. } => host.target,
                    #[cfg(debug_assertions)]
                    ImageSource::Development => unreachable!(),
                },
                Some("new-state"),
                Some("new-image"),
            ))
            .map_err(Into::into)
        },
        |point| {
            if Some(point) == stop {
                Err(RuntimeError::Closed)
            } else {
                Ok(())
            }
        },
        available_space,
    )
}

#[test]
fn retry_each_forward_boundary_preserves_a_and_creates_a_new_destination_then_restores_a() {
    for target in [ImageTarget::MacosArm64, ImageTarget::WindowsAmd64] {
        for point in [
            "intent",
            "model-archived",
            "account-archived",
            "source-retained",
            "destination-recorded",
            "image-prepared",
        ] {
            let mut s = Scene::new(target);
            let root = s.local.join("local-vm-v1");
            let a = fs::read(root.join("state.disk")).unwrap();
            assert!(s.replace(Some(point)).is_err(), "{point}");
            let transaction = id(&s);
            let old_b = WorkspaceUpgrade::open(&s.data, &s.local)
                .unwrap()
                .read_intent()
                .unwrap()
                .unwrap()
                .destination;
            assert_eq!(
                retry(&s, transaction, None).unwrap(),
                UpgradeStatus::Fresh,
                "{point}"
            );
            let current = installation(&root).unwrap().unwrap();
            assert_ne!(current.installation, s.source_id);
            if let Some(old) = old_b {
                assert_ne!(current.installation, old.installation);
                assert_eq!(
                    installation(
                        &s.local
                            .join(format!("retained-incomplete-{}", old.installation))
                    )
                    .unwrap(),
                    Some(old)
                );
            }
            let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
            assert_eq!(owner.restore(transaction).unwrap(), UpgradeStatus::Restored);
            assert_eq!(read_record(&root).unwrap().installation_id, s.source_id);
            assert_eq!(fs::read(root.join("state.disk")).unwrap(), a);
        }
    }
}

#[test]
fn retry_retains_preparing_partial_bytes_and_all_prior_retry_destinations() {
    let mut s = Scene::new(ImageTarget::WindowsAmd64);
    s.replace(None).unwrap();
    let transaction = id(&s);
    let root = s.local.join("local-vm-v1");
    let mut previous = Vec::new();
    for _ in 0..3 {
        let old = installation(&root).unwrap().unwrap();
        let mut record = read_record(&root).unwrap();
        record.phase = StatePhase::Preparing;
        fs::write(
            root.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        fs::write(root.join("state.disk"), b"partial disk, never format again").unwrap();
        assert_eq!(retry(&s, transaction, None).unwrap(), UpgradeStatus::Fresh);
        previous.push(old);
        for old in &previous {
            let path = s
                .local
                .join(format!("retained-incomplete-{}", old.installation));
            assert_eq!(installation(&path).unwrap().as_ref(), Some(old));
            assert_eq!(
                fs::read(path.join("state.disk")).unwrap(),
                b"partial disk, never format again"
            );
        }
    }
}

#[test]
fn retry_retention_crashes_resume_exactly_and_restore_can_recover_each_pending_move() {
    for point in [
        "retry-intent",
        "retry-model-archived",
        "retry-account-archived",
        "retry-destination-retained",
        "retry-recorded",
        "destination-recorded",
        "image-prepared",
    ] {
        for finish_restore in [false, true] {
            let mut s = Scene::new(ImageTarget::WindowsAmd64);
            s.replace(None).unwrap();
            let transaction = id(&s);
            let root = s.local.join("local-vm-v1");
            let b = installation(&root).unwrap().unwrap();
            restore::destination_metadata(&s, b.installation);
            assert!(retry(&s, transaction, Some(point)).is_err(), "{point}");
            if finish_restore {
                let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
                assert_eq!(
                    owner.restore(transaction).unwrap(),
                    UpgradeStatus::Restored,
                    "{point}"
                );
                assert_eq!(read_record(&root).unwrap().installation_id, s.source_id);
            } else {
                assert_eq!(
                    retry(&s, transaction, None).unwrap(),
                    UpgradeStatus::Fresh,
                    "{point}"
                );
            }
            assert_eq!(
                installation(
                    &s.local
                        .join(format!("retained-incomplete-{}", b.installation))
                )
                .unwrap(),
                Some(b)
            );
        }
    }
}

#[test]
fn retry_rejects_foreign_consumed_owned_and_unknown_canonical_without_mutation() {
    for scenario in [
        "foreign", "consumed", "live", "unknown", "no-space", "offline",
    ] {
        let mut s = Scene::new(ImageTarget::WindowsAmd64);
        s.replace(None).unwrap();
        let transaction = id(&s);
        let root = s.local.join("local-vm-v1");
        let mut held = None;
        match scenario {
            "consumed" => {
                restore::consumed_destination(&s);
            }
            "live" => held = Some(Journal::open(&root).unwrap()),
            "unknown" => {
                fs::rename(&root, s.local.join("unexpected-kept")).unwrap();
                drop(Journal::create(&root, BackendKind::Qemu, vec![9]).unwrap());
            }
            _ => {}
        }
        let before = fs::read(s.local.join(STORE).join("intent.json")).unwrap();
        let canonical = installation(&root).unwrap();
        let result = if scenario == "no-space" || scenario == "offline" {
            let mut owner = WorkspaceUpgrade::open(&s.data, &s.local).unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let (_send, closed) = watch::channel(false);
            owner.retry_with(
                transaction,
                &s.source,
                vec![2],
                &closed,
                runtime.handle(),
                || {
                    if scenario == "offline" {
                        return Err(RuntimeError::Unavailable);
                    }
                    let cache = tempfile::tempdir().unwrap();
                    PreparedImage::from_verified(fixture::verified_with_identity(
                        cache.path(),
                        ImageTarget::WindowsAmd64,
                        Some("new-state"),
                        Some("new-image"),
                    ))
                    .map_err(Into::into)
                },
                |_| Ok(()),
                |_| Ok(0),
            )
        } else {
            retry(
                &s,
                if scenario == "foreign" {
                    Uuid::new_v4()
                } else {
                    transaction
                },
                None,
            )
        };
        assert!(result.is_err(), "{scenario}");
        assert_eq!(
            fs::read(s.local.join(STORE).join("intent.json")).unwrap(),
            before,
            "{scenario}"
        );
        assert_eq!(installation(&root).unwrap(), canonical, "{scenario}");
        drop(held);
    }
}
