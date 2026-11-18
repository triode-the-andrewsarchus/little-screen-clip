//! Bulk admission retains blocked root checks outside the coordinator.
use super::*;

#[test]
fn backup_worker_rechecks_a_destination_enrolled_after_preparation() {
    let fixture = TestDrives::new(4096);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    fixture.pair(&service, "small", "");
    let drive = fixture.drives.lock().unwrap()[1].clone();
    let source = Arc::new(Root::open(drive.mount.as_ref().unwrap()).unwrap());
    let target = Arc::new(Root::create(&fixture.temp.path().join("backups")).unwrap());
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    policy::validate_backup_storage(&db, &service.state, &target).unwrap();
    let mut pair = status(&service).pairs[0].clone();
    pair.local = target.path.clone();
    pair.local_identity = Some(target.storage_identity().unwrap());
    pair.paused = true;
    db.put("pair", &pair.id, &pair).unwrap();
    let result = background::Worker {
        state: service.state.clone(),
        id: "owned-after-preparation-guard".into(),
        request: DriveRequest::Backup {
            drive: drive.id.clone(),
            kind: BackupKind::Files,
            restore_mounted: false,
        },
        drive,
        config: status(&service).config,
        roots: vec![source, target.clone()],
        context: None,
        cancel: Arc::new(AtomicBool::new(false)),
        authority: Arc::new(fixture.image_authority()),
        inventory: service.inventory.clone(),
    }
    .run()
    .unwrap_err();
    assert!(
        result.to_string().contains("paired local folder"),
        "{result}"
    );
    assert_eq!(fs::read_dir(&target.path).unwrap().count(), 0);
    assert!(status(&service).backups.is_empty());
    service.shutdown().unwrap();
}

#[test]
fn file_backup_and_catalog_discovery_never_reenter_the_coordinator() {
    for mode in ["direct", "review", "catalog"] {
        let fixture = TestDrives::new(4096);
        let armed = Arc::new(AtomicBool::new(false));
        let owner_reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let active = armed.clone();
        let reads = owner_reads.clone();
        let service = fixture.start_observed_reads(Arc::new(move || {
            if active.load(Ordering::Acquire)
                && std::thread::current().name() == Some("portable-drive-owner")
            {
                reads.fetch_add(1, Ordering::AcqRel);
            }
        }));
        let destination = backup_review::destination(&fixture);
        finish(
            &service,
            DriveRequest::BackupDirectory {
                path: destination.clone(),
            },
        );
        let request = if mode == "review" {
            backup_review::start(
                &backup_review::preview(&service, BackupKind::Files, &destination),
                false,
            )
        } else if mode == "catalog" {
            DriveRequest::Catalog {
                drive: "part:large".into(),
            }
        } else {
            DriveRequest::Backup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                restore_mounted: false,
            }
        };
        armed.store(true, Ordering::Release);
        let result = completed(&service, request);
        armed.store(false, Ordering::Release);
        service.shutdown().unwrap();
        assert_eq!(result.state, "complete", "{}", result.message);
        assert_eq!(
            owner_reads.load(Ordering::Acquire),
            0,
            "{mode} discovery ran on owner"
        );
    }
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and packaged RAM helper; stops only its owned server"]
fn stopped_bulk_preparation_keeps_controls_and_retains_actual_cancellation() {
    for mode in ["direct", "review", "cancel", "settings", "catalog"] {
        stopped_case(mode);
    }
}

fn stopped_case(mode: &str) {
    let fixture = TestDrives::new(4096);
    let (backing, id, mut native, view) = fixture.native_source("bulk-preparation");
    if mode == "direct" {
        fixture.drives.lock().unwrap()[1].mount = Some(fixture.temp.path().join("large"));
    }
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let small = fixture.pair(&service, "small", "");
    let pair = status(&service).pairs[0].id.clone();
    let destination = if mode == "direct" {
        view.join("backups")
    } else {
        fixture.temp.path().join("backups")
    };
    fs::create_dir(&destination).unwrap();
    let request = match mode {
        "catalog" => DriveRequest::Catalog {
            drive: "part:large".into(),
        },
        "direct" => {
            finish(
                &service,
                DriveRequest::BackupDirectory {
                    path: destination.clone(),
                },
            );
            DriveRequest::Backup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                restore_mounted: false,
            }
        }
        _ => backup_review::start(
            &backup_review::preview(&service, BackupKind::Files, &destination),
            false,
        ),
    };
    let mut operation = None;
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            let accepted = accept(&service, request);
            operation = Some(accepted.clone());
            let (tid, wait) = wait_fuse_request(|| {
                let mut tids = worker_threads("drive-scope-");
                tids.extend(worker_threads("portable-drive-"));
                tids
            });
            eprintln!("BULK_PREPARATION_FUSE_BLOCKED mode={mode} tid={tid} wchan={wait}");
            finish(
                &service,
                DriveRequest::Pause {
                    pair: pair.clone(),
                    paused: false,
                },
            );
            if mode != "direct" {
                // A one-shot reviewed destination is not a configured managed
                // root. Independent reconciliation can therefore validate its
                // own policy without entering this stopped ordinary filesystem.
                fs::write(small.join("independent"), b"sync during bulk preparation").unwrap();
                wait_contents(
                    &fixture.temp.path().join("small/independent"),
                    b"sync during bulk preparation",
                );
            }
            assert!(worker_threads("drive-scope-").contains(&tid));
            assert_unfinished(&service, &accepted);
            if mode == "settings" {
                let mut config = status(&service).config;
                config.sync_interval_seconds += 1;
                finish(&service, DriveRequest::Configure { config });
            }
            if ["direct", "cancel"].contains(&mode) {
                service
                    .request(DriveRequest::Cancel {
                        operation: accepted.clone(),
                    })
                    .unwrap();
                assert_unfinished(&service, &accepted);
                closing = Some(shutdown_while_held(&service));
            }
        });
    }));
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    if result.is_ok() {
        let completed = wait_operation(&service, &operation.unwrap(), |op| op.finished.is_some());
        assert_outcome(&service, &completed, &destination, mode);
    }
    service.shutdown().unwrap();
    finish_native_fixture(backing, id, native, result);
}

fn assert_outcome(service: &Service, completed: &DriveOperation, destination: &Path, mode: &str) {
    let expected = match mode {
        "direct" | "cancel" => "cancelled",
        "settings" => "failed",
        _ => "complete",
    };
    assert_eq!(completed.state, expected, "{mode}: {}", completed.message);
    let saved = status(service);
    if mode == "settings" {
        assert!(
            completed.message.contains("inputs changed"),
            "{}",
            completed.message
        );
    }
    if ["direct", "cancel", "settings"].contains(&mode) {
        assert!(saved.backups.is_empty());
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    } else if mode == "catalog" {
        assert_eq!(saved.catalogs.len(), 1);
    } else {
        assert_eq!(saved.backups.len(), 1);
        assert_eq!(
            fs::read(saved.backups[0].path.join("edit")).unwrap(),
            b"original portable content"
        );
    }
}
