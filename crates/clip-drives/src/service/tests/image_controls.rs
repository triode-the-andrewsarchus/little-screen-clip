//! Public admission proves policy ordering and retained image-worker lifetimes.
use super::*;
use crate::{backup::ImagePhase as Phase, service::image_controls::KIND};
use std::sync::atomic::AtomicUsize;

fn counted_image_authority(fixture: &TestDrives) -> (partition::Authority, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mut authority = fixture.image_authority();
    authority.images.mount = Arc::new(move |_, _, _| {
        seen.fetch_add(1, Ordering::AcqRel);
        Err("unexpected native admission".into())
    });
    (authority, calls)
}

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
    let (authority, calls) = counted_image_authority(&fixture);
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
fn image_control_rechecks_its_saved_input_before_calling_authority() {
    for event in ["INSERT", "UPDATE"] {
        let fixture = TestDrives::new(4096);
        let image = fixture.saved_image();
        let bytes = fs::read(&image.path).unwrap();
        let (authority, calls) = counted_image_authority(&fixture);
        let service = fixture.start_authority(None, authority);
        let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
        // The INSERT precedes initial worker admission; the UPDATE occurs when
        // host-origin binding completes, after the first input check.
        db.0.execute_batch(&format!(
            "CREATE TRIGGER change_image_input AFTER {event} ON entities
         WHEN NEW.kind='image-control'
         BEGIN UPDATE entities SET json=json_set(json,'$.kind','Files')
         WHERE kind='backup' AND id=NEW.id; END;",
        ))
        .unwrap();
        let result = completed(
            &service,
            DriveRequest::MountImage {
                backup: image.id.clone(),
            },
        );
        assert_eq!(result.state, "failed");
        assert!(
            result
                .message
                .contains("input changed before native admission"),
            "{}",
            result.message
        );
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert!(
            db.get::<serde_json::Value>(KIND, &image.id)
                .unwrap()
                .is_some()
        );
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
        assert_eq!(fs::read(&image.path).unwrap(), bytes);
        service.shutdown().unwrap();
    }
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
        native_image_checkpoint(stage, &partition, "");
    }
}

#[test]
#[ignore = "requires the isolated partition/authority guest"]
fn native_partial_image_cleanup_retries_only_its_verified_attachment() {
    let partition = crate::sync_edge_tests::native_fs::guest_drive();
    for fault in [
        "legacy",
        "unmount-record",
        "retired-record",
        "raw-reader",
        "reused",
    ] {
        native_image_checkpoint(Phase::Retiring, &partition, fault);
    }
}

fn native_image_checkpoint(stage: Phase, partition: &Drive, fault: &str) {
    let original = native_images::selected(partition);
    let fixture = TestDrives::new(4096);
    let backup_destination = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR")
        .map(|base| tempfile::tempdir_in(base).unwrap());
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
        backup_directory: Some(backup_destination.as_ref().map_or_else(
            || fixture.temp.path().join("backup"),
            |directory| directory.path().to_path_buf(),
        )),
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
    let mut fault_fixture = NativeImageFault::prepare(fault, &db, &image, fixture.temp.path());
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
        if stage == Phase::LoopOwned || matches!(fault, "unmount-record" | "raw-reader") {
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
        stage == Phase::Ready || (!fault.is_empty() && fault != "legacy"),
    );
    assert_eq!(
        native_images::digest(File::open(&image.path).unwrap()),
        expected
    );
    let saved: DriveBackup = db.get("backup", &image.id).unwrap().unwrap();
    if stage == Phase::Ready || (!fault.is_empty() && fault != "legacy") {
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
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
        } else {
            fault_fixture.after_operation(&db, &completed);
        }
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
        if stage == Phase::Ready {
            wait_image_recovery(&db, &image.id);
        }
        fault_fixture.verify_reused(&restarted, unmounting.clone());
        finish(&restarted, unmounting);
        assert!(
            db.get::<serde_json::Value>("image-loop", &image.id)
                .unwrap()
                .is_none()
        );
        assert!(
            db.get::<serde_json::Value>(KIND, &image.id)
                .unwrap()
                .is_none()
        );
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
    eprintln!(
        "Native image checkpoint {stage:?}/{fault}: {}",
        completed.state
    );
}

struct NativeImageFault<'a> {
    kind: &'a str,
    reader: Option<File>,
    device: Option<PathBuf>,
    foreign: PathBuf,
}
impl<'a> NativeImageFault<'a> {
    fn prepare(kind: &'a str, db: &Store, image: &DriveBackup, temp: &Path) -> Self {
        let mut fault = Self {
            kind,
            reader: None,
            device: None,
            foreign: temp.join("foreign.img"),
        };
        if kind.is_empty() {
            return fault;
        }
        let mounted: DriveBackup = db.get("backup", &image.id).unwrap().unwrap();
        let device = native_images::loop_device(mounted.loop_object.as_deref().unwrap());
        if kind == "legacy" {
            db.remove("image-loop", &image.id).unwrap();
        } else if kind == "raw-reader" {
            fault.reader = Some(File::open(&device).unwrap());
        } else {
            let condition = if kind == "unmount-record" {
                "json_extract(NEW.json,'$.loop_object') IS NOT NULL AND json_extract(NEW.json,'$.mount') IS NULL"
            } else {
                "json_extract(NEW.json,'$.loop_object') IS NULL"
            };
            db.0.execute_batch(&format!("CREATE TRIGGER partial_image_record_fault BEFORE UPDATE ON entities WHEN NEW.kind='backup' AND ({condition}) BEGIN SELECT RAISE(FAIL,'partial image record fault'); END;")).unwrap();
        }
        fault.device = Some(device);
        fault
    }
    fn after_operation(&mut self, db: &Store, operation: &DriveOperation) {
        if self.kind == "raw-reader" {
            assert!(
                operation
                    .message
                    .contains("deferred by open device readers"),
                "{}",
                operation.message
            );
        } else {
            db.0.execute_batch("DROP TRIGGER partial_image_record_fault")
                .unwrap();
            assert!(
                operation.message.contains("partial image record fault"),
                "{}",
                operation.message
            );
        }
        drop(self.reader.take());
        if self.kind == "reused" {
            fs::write(&self.foreign, b"foreign owned fixture").unwrap();
            File::options()
                .write(true)
                .open(&self.foreign)
                .unwrap()
                .set_len(256 * 1024 * 1024)
                .unwrap();
            let result = CommandRequest::new("losetup")
                .arg("--read-only")
                .arg(self.device.as_ref().unwrap())
                .arg(&self.foreign)
                .run()
                .unwrap();
            assert!(result.status.success(), "{}", result.stderr_lossy());
        }
    }
    fn verify_reused(&self, service: &Service, unmount: DriveRequest) {
        if self.kind != "reused" {
            return;
        }
        failed(service, unmount, "attachment changed");
        let foreign_loop = CommandRequest::new("losetup")
            .arg("-j")
            .arg(&self.foreign)
            .run()
            .unwrap();
        assert!(foreign_loop.status.success() && !foreign_loop.stdout.is_empty());
        let mut prefix = [0_u8; 21];
        std::os::unix::fs::FileExt::read_exact_at(
            &File::open(&self.foreign).unwrap(),
            &mut prefix,
            0,
        )
        .unwrap();
        assert_eq!(&prefix, b"foreign owned fixture");
        let detached = CommandRequest::new("losetup")
            .arg("--detach")
            .arg(self.device.as_ref().unwrap())
            .run()
            .unwrap();
        assert!(detached.status.success(), "{}", detached.stderr_lossy());
    }
}

#[test]
fn an_explicit_retry_keeps_an_old_unidentified_origin_for_recovery() {
    let fixture = TestDrives::new(4096);
    let image = fixture.saved_image();
    let mut authority = fixture.image_authority();
    authority.images.mount = Arc::new(|_, _, phase| {
        phase(Phase::StartingLoop)?;
        Err("owned missing loop reply".into())
    });
    let service = fixture.start_authority(None, authority);
    failed(
        &service,
        DriveRequest::MountImage {
            backup: image.id.clone(),
        },
        "missing loop reply",
    );
    service.shutdown().unwrap();
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let mut intent = db
        .get::<serde_json::Value>(KIND, &image.id)
        .unwrap()
        .unwrap();
    intent["origin"]["host"] = serde_json::Value::Null;
    intent["origin"]["boot"] = serde_json::to_value(uuid::Uuid::new_v4()).unwrap();
    db.put(KIND, &image.id, &intent).unwrap();
    let (authority, calls) = counted_image_authority(&fixture);
    let service = fixture.start_authority(None, authority);
    failed(
        &service,
        DriveRequest::UnmountImage {
            backup: image.id.clone(),
        },
        "previous boot",
    );
    service.shutdown().unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 0);
    assert_eq!(
        db.get::<serde_json::Value>(KIND, &image.id)
            .unwrap()
            .unwrap(),
        intent
    );
    assert!(
        published_policy(&service)
            .excluded
            .contains(&PathBuf::from("/"))
    );
}
