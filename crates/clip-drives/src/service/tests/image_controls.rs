//! Public admission proves policy ordering and retained image-worker lifetimes.
use super::*;
use crate::{backup::ImagePhase as Phase, service::image_controls::KIND};
use std::sync::atomic::AtomicUsize;

#[test]
fn held_image_controls_keep_policy_controls_sync_and_shutdown_owned() {
    for case in [
        "denied",
        "cancel",
        "uncertain",
        "changed-owner",
        "unmount",
        "retired",
    ] {
        let fixture = TestDrives::new(4096);
        let image = fixture.saved_image();
        let before = fs::read(&image.path).unwrap();
        let state = fixture.temp.path().join("state");
        let db = Store::existing(&state.join("drives.sqlite"), false).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let (entered, release, gate) = worker_gate();
        let authority_state = state.clone();
        let action: backup::ImageAction = Arc::new(move |_, _, phase| {
            seen.fetch_add(1, Ordering::AcqRel);
            let policy: IndexPolicy = serde_json::from_slice(
                &fs::read(authority_state.join("index-policy.json")).unwrap(),
            )
            .unwrap();
            assert!(
                policy.excluded.contains(&PathBuf::from("/")),
                "Image effect preceded indexing exclusion"
            );
            phase(if case == "unmount" {
                Phase::Retiring
            } else {
                Phase::StartingLoop
            })?;
            gate();
            if case == "uncertain" {
                return Err("owned lost image authority reply".into());
            }
            phase(if case == "retired" {
                Phase::Retired
            } else {
                Phase::NoEffect
            })?;
            if case == "retired" {
                Ok("owned retirement complete".into())
            } else {
                Err("owned image permission refusal".into())
            }
        });
        let mut authority = fixture.image_authority();
        authority.images.mount = action.clone();
        authority.images.unmount = action;
        let service = fixture.start_authority(None, authority);
        let local = fixture.pair(&service, "small", "");
        let request = if case == "unmount" {
            DriveRequest::UnmountImage {
                backup: image.id.clone(),
            }
        } else {
            DriveRequest::MountImage {
                backup: image.id.clone(),
            }
        };
        let operation = accept(&service, request.clone());
        let mut closing = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_unfinished(&service, &operation);
            assert!(
                db.get::<serde_json::Value>(KIND, &image.id)
                    .unwrap()
                    .is_some()
            );
            assert_configuration_responsive(&service);
            fs::write(local.join("independent"), case.as_bytes()).unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                case.as_bytes(),
            );
            if case == "changed-owner" {
                let mut intent = db
                    .get::<serde_json::Value>(KIND, &image.id)
                    .unwrap()
                    .unwrap();
                intent["operation"] = "replacement-owner".into();
                db.put(KIND, &image.id, &intent).unwrap();
            }
            if case == "cancel" {
                service
                    .request(DriveRequest::Cancel {
                        operation: operation.clone(),
                    })
                    .unwrap();
                assert_unfinished(&service, &operation);
                closing = Some(shutdown_while_held(&service));
            }
        }));
        let _ = release.send(());
        if let Some(closing) = closing {
            finish_held_shutdown(closing);
        }
        let completed = wait_operation(&service, &operation, |op| op.finished.is_some());
        service.shutdown().unwrap();
        result.unwrap();
        assert_eq!(calls.load(Ordering::Acquire), 1);
        assert_eq!(fs::read(&image.path).unwrap(), before);
        let retained = db.get::<serde_json::Value>(KIND, &image.id).unwrap();
        if case == "uncertain" || case == "changed-owner" {
            assert_eq!(completed.state, "failed", "{}", completed.message);
            assert!(retained.is_some());
            assert!(
                published_policy(&service)
                    .excluded
                    .contains(&PathBuf::from("/"))
            );
        } else {
            assert_eq!(
                completed.state,
                if case == "cancel" {
                    "cancelled"
                } else if case == "retired" {
                    "complete"
                } else {
                    "failed"
                },
                "{}",
                completed.message
            );
            assert!(retained.is_none());
        }
        if case == "uncertain" {
            let replayed = fixture.start_authority(None, fixture.image_authority());
            match replayed
                .submit(uuid::Uuid::parse_str(&operation).unwrap(), request)
                .unwrap()
            {
                DriveReply::Accepted(id) => assert_eq!(id, operation),
                _ => panic!("expected retained image operation"),
            }
            failed(
                &replayed,
                DriveRequest::UnmountImage {
                    backup: image.id.clone(),
                },
                "uncertain",
            );
            assert!(
                status(&replayed)
                    .recovery
                    .iter()
                    .any(|row| row.id == format!("image-control:{}", image.id))
            );
            replayed.shutdown().unwrap();
            assert!(
                db.get::<serde_json::Value>(KIND, &image.id)
                    .unwrap()
                    .is_some()
            );
        }
    }
}

#[test]
fn image_control_never_starts_when_exclusion_installation_fails() {
    let fixture = TestDrives::new(4096);
    let image = fixture.saved_image();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut authority = fixture.image_authority();
    authority.images.mount = Arc::new(move |_, _, _| {
        seen.fetch_add(1, Ordering::AcqRel);
        Ok("unexpected effect".into())
    });
    let service = fixture.start_authority(None, authority);
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "seed".into(),
            catalog: true,
            index: true,
        },
    );
    let policy = service.state.join("index-policy.json");
    let retained = service.state.join("saved-policy.json");
    fs::rename(&policy, &retained).unwrap();
    fs::create_dir(&policy).unwrap();
    let result = completed(&service, DriveRequest::MountImage { backup: image.id });
    assert_eq!(result.state, "failed");
    assert!(
        result
            .message
            .contains("Could not protect image mount admission"),
        "{}",
        result.message
    );
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert!(policy.is_dir());
    fs::remove_dir(&policy).unwrap();
    fs::rename(&retained, &policy).unwrap();
    service.shutdown().unwrap();
}

#[test]
fn pending_image_reinstalls_exclusions_even_while_an_old_policy_observer_is_held() {
    let fixture = TestDrives::new(4096);
    let image = fixture.saved_image();
    let armed = Arc::new(AtomicBool::new(false));
    let active = armed.clone();
    let (policy_entered, policy_release, policy_gate) = worker_gate();
    let publisher = publication::Publication::new(Arc::new(move |input, previous| {
        if active.swap(false, Ordering::AcqRel) {
            policy_gate();
        }
        publication::observe(input, previous)
    }));
    let (entered, release, gate) = worker_gate();
    let mut authority = fixture.image_authority();
    authority.images.mount = Arc::new(move |_, _, phase| {
        phase(Phase::StartingLoop)?;
        gate();
        phase(Phase::NoEffect)?;
        Err("owned image refusal".into())
    });
    let service = fixture.start_with_publication(
        DriveConfig {
            ram_min_available: 0,
            ..Default::default()
        },
        None,
        authority,
        publisher,
    );
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "seed".into(),
            catalog: true,
            index: true,
        },
    );
    armed.store(true, Ordering::Release);
    assert_configuration_responsive(&service);
    let mut operation = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        policy_entered.recv_timeout(Duration::from_secs(5)).unwrap();
        operation = Some(accept(
            &service,
            DriveRequest::MountImage {
                backup: image.id.clone(),
            },
        ));
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
        fs::write(
            service.state.join("index-policy.json"),
            serde_json::to_vec(&IndexPolicy::default()).unwrap(),
        )
        .unwrap();
        assert_configuration_responsive(&service);
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/")),
            "Unchanged pending admission must reinstall its exclusion barrier"
        );
    }));
    let _ = policy_release.send(());
    if result.is_ok() {
        // The old successful observer cannot release a newer pending control.
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
    }
    let _ = release.send(());
    if let Some(operation) = operation {
        wait_operation(&service, &operation, |op| op.finished.is_some());
    }
    service.shutdown().unwrap();
    result.unwrap();
}

#[test]
fn interrupted_pre_effect_image_admission_recovers_without_replaying_authority() {
    let fixture = TestDrives::new(4096);
    let image = fixture.saved_image();
    let (captured, journal) = mpsc::channel();
    let mut authority = fixture.image_authority();
    authority.images.mount = Arc::new(move |db, id, phase| {
        captured
            .send(db.get::<serde_json::Value>(KIND, id)?.unwrap())
            .unwrap();
        phase(Phase::NoEffect)?;
        Err("owned pre-effect refusal".into())
    });
    let service = fixture.start_authority(None, authority);
    let operation = completed(
        &service,
        DriveRequest::MountImage {
            backup: image.id.clone(),
        },
    );
    let prepared = journal.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(prepared["phase"], "Prepared");
    service.shutdown().unwrap();
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    db.put(KIND, &image.id, &prepared).unwrap();
    let mut interrupted = operation.clone();
    interrupted.finished = None;
    db.put("operation", &operation.id, &interrupted).unwrap();
    let mut authority = fixture.image_authority();
    authority.images.mount = Arc::new(|_, _, _| panic!("Restart replayed image authority"));
    let restarted = fixture.start_authority(None, authority);
    wait_image_recovery(&db, &image.id);
    assert_eq!(
        status(&restarted)
            .operations
            .iter()
            .find(|op| op.id == operation.id)
            .unwrap()
            .state,
        "interrupted"
    );
    finish(
        &restarted,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "after-image-recovery".into(),
            catalog: true,
            index: true,
        },
    );
    assert!(
        !published_policy(&restarted)
            .excluded
            .contains(&PathBuf::from("/"))
    );
    restarted.shutdown().unwrap();
}

#[test]
fn image_receipt_failure_preserves_completion_and_recovers_only_matching_versions() {
    for case in ["refused", "retired", "changed-version"] {
        let fixture = TestDrives::new(4096);
        let image = fixture.saved_image();
        let bytes = fs::read(&image.path).unwrap();
        let db = Store::existing(&fixture.temp.path().join("state/drives.sqlite"), false).unwrap();
        db.0.execute_batch("CREATE TRIGGER image_receipt_fault BEFORE DELETE ON entities WHEN OLD.kind='image-control' BEGIN SELECT RAISE(ABORT,'owned image receipt fault'); END;").unwrap();
        let mut authority = fixture.image_authority();
        authority.images.mount = Arc::new(move |_, _, phase| {
            phase(Phase::StartingLoop)?;
            phase(if case == "refused" {
                Phase::NoEffect
            } else {
                Phase::Retired
            })?;
            if case == "refused" {
                Err("owned authority refusal".into())
            } else {
                Ok("owned retirement completed".into())
            }
        });
        let service = fixture.start_authority(None, authority);
        let operation = completed(
            &service,
            DriveRequest::MountImage {
                backup: image.id.clone(),
            },
        );
        assert_eq!(
            operation.state,
            if case == "refused" {
                "failed"
            } else {
                "complete"
            },
            "{}",
            operation.message
        );
        assert!(operation.message.contains("indexing remains paused"));
        assert!(
            db.get::<serde_json::Value>(KIND, &image.id)
                .unwrap()
                .is_some()
        );
        service.shutdown().unwrap();
        db.0.execute_batch("DROP TRIGGER image_receipt_fault")
            .unwrap();
        if case == "changed-version" {
            let mut replacement = image.clone();
            replacement.path = image.path.with_file_name("replaced.img");
            fs::write(&replacement.path, b"independent replacement").unwrap();
            db.put("backup", &image.id, &replacement).unwrap();
        }
        let restarted = fixture.start_authority(None, fixture.image_authority());
        if case == "changed-version" {
            let started = Instant::now();
            while !status(&restarted)
                .recovery
                .iter()
                .any(|row| row.message.contains("identity changed"))
            {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(
                db.get::<serde_json::Value>(KIND, &image.id)
                    .unwrap()
                    .is_some()
            );
            assert!(
                published_policy(&restarted)
                    .excluded
                    .contains(&PathBuf::from("/"))
            );
        } else {
            wait_image_recovery(&db, &image.id);
            assert!(
                status(&restarted)
                    .recovery
                    .iter()
                    .all(|row| row.id != format!("image-control:{}", image.id))
            );
        }
        assert_eq!(fs::read(&image.path).unwrap(), bytes);
        restarted.shutdown().unwrap();
    }
}

fn wait_image_recovery(db: &Store, id: &str) {
    let started = Instant::now();
    while db.get::<serde_json::Value>(KIND, id).unwrap().is_some() {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires an explicitly saved pre-version-3 catalog helper"]
fn legacy_helper_refuses_image_control_registry_before_mounting() {
    let fixture = TestDrives::new(4096);
    let image = fixture.saved_image();
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Catalog {
            drive: "part:large".into(),
        },
    );
    let catalog = status(&service).catalogs.pop().unwrap();
    let result = completed(&service, DriveRequest::MountImage { backup: image.id });
    assert_eq!(result.state, "failed");
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let version: u32 =
        db.0.query_row("SELECT value FROM version WHERE id=1", [], |row| row.get(0))
            .unwrap();
    assert_eq!(version, 3);
    let target = fixture.temp.path().join("legacy-mount");
    fs::create_dir(&target).unwrap();
    let helper = std::env::var_os("LITTLE_CLIP_LEGACY_IMAGE_HELPER")
        .expect("explicit legacy helper fixture");
    let output = CommandRequest::new(helper)
        .arg(&service.state)
        .arg(&catalog.id)
        .arg(&target)
        .timeout(Duration::from_secs(5))
        .die_with_parent()
        .run()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        output
            .stderr_lossy()
            .contains("Unsupported drive database version"),
        "{}",
        output.stderr_lossy()
    );
    assert!(
        crate::mounts::inventory()
            .unwrap()
            .iter()
            .all(|mount| mount.target != target)
    );
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires the isolated partition/authority guest"]
fn native_image_checkpoints_keep_sync_live_and_preserve_actual_cancellation_results() {
    let partition = crate::sync_edge_tests::native_fs::guest_drive();
    for stage in [Phase::LoopOwned, Phase::Ready, Phase::Retiring] {
        native_image_checkpoint(stage, &partition);
    }
}

fn native_image_checkpoint(stage: Phase, partition: &Drive) {
    let original = native_images::selected(partition);
    let fixture = TestDrives::new(4096);
    fs::write(
        original.mount.as_ref().unwrap().join("retained-image.txt"),
        b"real retained image source",
    )
    .unwrap();
    let small = fixture.drives.lock().unwrap()[0].clone();
    let inventory: Inventory = Arc::new(move || {
        let mut drives = discovery::discover()?;
        drives.push(small.clone());
        Ok(drives)
    });
    let (entered, release, gate) = worker_gate();
    let mut authority = partition::Authority::default();
    let action = if stage == Phase::Retiring {
        &mut authority.images.unmount
    } else {
        &mut authority.images.mount
    };
    let native = action.clone();
    *action = Arc::new(move |db, id, checkpoint| {
        native(db, id, &mut |phase| {
            checkpoint(phase)?;
            if phase == stage {
                gate();
            }
            Ok(())
        })
    });
    let config = DriveConfig {
        backup_directory: Some(fixture.temp.path().join("backup")),
        ram_min_available: u64::MAX,
        sync_interval_seconds: 3600,
        ..Default::default()
    };
    let service = Service::start_with_authority(
        config.clone(),
        fixture.temp.path().join("state"),
        inventory.clone(),
        None,
        Arc::new(authority),
        Default::default(),
    )
    .unwrap();
    finish(
        &service,
        DriveRequest::Backup {
            drive: original.id,
            kind: BackupKind::Image,
            restore_mounted: true,
        },
    );
    let image = status(&service).backups.pop().unwrap();
    let expected = native_images::digest(File::open(&image.path).unwrap());
    let local = fixture.pair(&service, "small", "");
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let mounting = DriveRequest::MountImage {
        backup: image.id.clone(),
    };
    let unmounting = DriveRequest::UnmountImage {
        backup: image.id.clone(),
    };
    let request = if stage == Phase::Retiring {
        finish(&service, mounting.clone());
        unmounting.clone()
    } else {
        mounting
    };
    let operation = accept(&service, request);
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_unfinished(&service, &operation);
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
        assert_configuration_responsive(&service);
        fs::write(
            local.join("independent"),
            b"sync during native image control",
        )
        .unwrap();
        wait_contents(
            &fixture.temp.path().join("small/independent"),
            b"sync during native image control",
        );
        if stage == Phase::Ready {
            db.0.execute_batch(
                "CREATE TRIGGER retain_ready_image_control BEFORE DELETE ON entities
                 WHEN OLD.kind='image-control'
                 BEGIN SELECT RAISE(FAIL,'retain completed native image'); END;",
            )
            .unwrap();
        }
        service
            .request(DriveRequest::Cancel {
                operation: operation.clone(),
            })
            .unwrap();
        assert_unfinished(&service, &operation);
        closing = Some(shutdown_while_held(&service));
    }));
    let _ = release.send(());
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    service.shutdown().unwrap();
    result.unwrap();
    let completed = wait_operation(&service, &operation, |op| op.finished.is_some());
    assert_eq!(
        completed.state,
        if stage == Phase::LoopOwned {
            "cancelled"
        } else {
            "complete"
        },
        "{}",
        completed.message
    );
    assert_eq!(
        db.get::<serde_json::Value>(KIND, &image.id)
            .unwrap()
            .is_some(),
        stage == Phase::Ready,
    );
    assert_eq!(
        native_images::digest(File::open(&image.path).unwrap()),
        expected
    );
    let saved: DriveBackup = db.get("backup", &image.id).unwrap().unwrap();
    if stage == Phase::Ready {
        assert!(
            completed.message.contains("retain completed native image"),
            "{}",
            completed.message
        );
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
        db.0.execute_batch("DROP TRIGGER retain_ready_image_control")
            .unwrap();
        backup::ImageMount::validate_view(&db, &saved).unwrap();
        assert_eq!(
            fs::read(saved.mount.as_ref().unwrap().join("retained-image.txt")).unwrap(),
            b"real retained image source"
        );
        let mut authority = partition::Authority::default();
        authority.images.mount = Arc::new(|_, _, _| panic!("Recovery must not replay LoopSetup"));
        let restarted = Service::start_with_authority(
            config,
            service.state.clone(),
            inventory,
            None,
            Arc::new(authority),
            Default::default(),
        )
        .unwrap();
        wait_image_recovery(&db, &image.id);
        finish(&restarted, unmounting);
        restarted.shutdown().unwrap();
    } else {
        assert!(saved.mount.is_none() && saved.loop_object.is_none());
    }
    let loops = CommandRequest::new("losetup")
        .arg("-j")
        .arg(&image.path)
        .run()
        .unwrap();
    assert!(loops.status.success());
    assert!(loops.stdout.is_empty(), "{}", loops.stdout_lossy());
    eprintln!("Native image checkpoint {stage:?}: {}", completed.state);
}
