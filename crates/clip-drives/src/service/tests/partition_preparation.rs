//! Pinning and pre-unmount plans retain their actual worker and original selection.
use super::*;

#[test]
fn failed_partition_scheduling_inventory_stops_before_any_preparation_effect() {
    for kind in ["eject", "image", "read-only"] {
        let fixture = TestDrives::new(4096);
        let destination = fixture.temp.path().join("not-created");
        let armed = Arc::new(AtomicBool::new(false));
        let failure = armed.clone();
        let drives = fixture.drives.clone();
        let service = Service::start_with_authority(
            DriveConfig {
                backup_directory: Some(destination.clone()),
                ram_min_available: u64::MAX,
                sync_interval_seconds: 3600,
                ..Default::default()
            },
            fixture.temp.path().join("state"),
            Arc::new(move || {
                if std::thread::current()
                    .name()
                    .is_some_and(|name| name.starts_with("drive-scope-"))
                    && failure.swap(false, Ordering::AcqRel)
                {
                    return Err("owned scheduling inventory failure".into());
                }
                Ok(drives.lock().unwrap().clone())
            }),
            None,
            Arc::new(fixture.image_authority()),
            Default::default(),
        )
        .unwrap();
        armed.store(true, Ordering::Release);
        let request = match kind {
            "image" => image_request(false),
            "read-only" => read_only::request("part:large"),
            _ => DriveRequest::Eject {
                drive: "part:large".into(),
            },
        };
        let result = completed(&service, request);
        assert_eq!(result.state, "failed", "{}", result.message);
        assert!(
            result
                .message
                .contains("owned scheduling inventory failure"),
            "{}",
            result.message
        );
        assert!(result.partition.is_none());
        assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
        assert!(!destination.exists());
        assert!(status(&service).backups.is_empty());
        service.shutdown().unwrap();
    }
}

#[test]
fn partition_inventory_never_reenters_the_coordinator() {
    use std::sync::atomic::AtomicUsize;
    for kind in ["eject", "image", "read-only"] {
        let fixture = TestDrives::new(4096);
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let service = fixture.start_observed_reads(Arc::new(move || {
            if std::thread::current().name() == Some("portable-drive-owner") {
                counter.fetch_add(1, Ordering::AcqRel);
            }
        }));
        finish(
            &service,
            DriveRequest::BackupDirectory {
                path: fixture.temp.path().join("backups"),
            },
        );
        fixture.pair(&service, "large", "");
        let request = match kind {
            "image" => image_request(false),
            "read-only" => read_only::request("part:large"),
            _ => DriveRequest::Eject {
                drive: "part:large".into(),
            },
        };
        let result = completed(&service, request);
        service.shutdown().unwrap();
        assert_eq!(
            result.state,
            if kind == "eject" {
                "complete"
            } else {
                "failed"
            },
            "{}",
            result.message
        );
        assert_eq!(
            calls.load(Ordering::Acquire),
            0,
            "{kind} inventory ran on the coordinator"
        );
    }
}

#[test]
fn retained_partition_preparation_keeps_sync_live_and_rejects_changed_inputs() {
    for outcome in [
        "success",
        "legacy",
        "cancel",
        "settings",
        "enrollment",
        "device",
    ] {
        held_case(outcome);
    }
}

fn held_case(outcome: &str) {
    let fixture = TestDrives::new(4096);
    let armed = Arc::new(AtomicBool::new(false));
    let (entered, release, observe) = worker_observer("drive-partition-", armed.clone());
    let service = fixture.start_observed_reads(observe);
    let small = fixture.pair(&service, "small", "");
    let large = fixture.pair(&service, "large", "");
    if outcome == "legacy" {
        let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
        let selected = status(&service)
            .pairs
            .into_iter()
            .find(|pair| pair.drive == "part:large")
            .unwrap();
        let transaction = db.0.unchecked_transaction().unwrap();
        for index in 0..1101 {
            let mut legacy = selected.clone();
            legacy.id = format!("legacy-paused-{index:04}");
            legacy.local = fixture.temp.path().join(format!("unused-legacy-{index}"));
            legacy.local_identity = None;
            legacy.paused = true;
            db.put("pair", &legacy.id, &legacy).unwrap();
        }
        transaction.commit().unwrap();
        assert!(
            db.list::<SyncPair>("pair", 0, 1000)
                .unwrap()
                .iter()
                .all(|pair| pair.id != selected.id)
        );
    }
    armed.store(true, Ordering::Release);
    let operation = accept(
        &service,
        DriveRequest::Eject {
            drive: "part:large".into(),
        },
    );
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    fs::write(
        small.join("independent"),
        b"sync during partition preparation",
    )
    .unwrap();
    wait_contents(
        &fixture.temp.path().join("small/independent"),
        b"sync during partition preparation",
    );
    assert_unfinished(&service, &operation);
    let mut closing = None;
    match outcome {
        "success" | "legacy" => {
            fs::write(large.join("before-unmount"), b"flush after preparation").unwrap();
        }
        "cancel" => {
            service
                .request(DriveRequest::Cancel {
                    operation: operation.clone(),
                })
                .unwrap();
            assert_unfinished(&service, &operation);
            closing = Some(shutdown_while_held(&service));
        }
        "settings" => {
            let mut config = status(&service).config;
            config.sync_interval_seconds += 1;
            finish(&service, DriveRequest::Configure { config });
        }
        "enrollment" => {
            finish(
                &service,
                DriveRequest::Pair {
                    drive: "part:small".into(),
                    relative: PathBuf::new(),
                    local: fixture.temp.path().join("another-pair"),
                },
            );
        }
        "device" => {
            fixture.drives.lock().unwrap()[1].disk_sequence = Some(2);
        }
        _ => unreachable!(),
    }
    release.send(()).unwrap();
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    let completed = wait_operation(&service, &operation, |op| op.finished.is_some());
    assert_eq!(
        completed.state,
        match outcome {
            "success" | "legacy" => "complete",
            "cancel" => "cancelled",
            _ => "failed",
        },
        "{outcome}: {}",
        completed.message
    );
    if ["success", "legacy"].contains(&outcome) {
        assert_eq!(
            fs::read(fixture.temp.path().join("large/before-unmount")).unwrap(),
            b"flush after preparation"
        );
        assert!(fixture.drives.lock().unwrap()[1].mount.is_none());
    } else {
        assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
    }
    if ["settings", "enrollment"].contains(&outcome) {
        assert!(
            completed.message.contains("inputs changed"),
            "{}",
            completed.message
        );
    }
    if outcome == "device" {
        assert!(
            completed.message.contains("changed since scheduling"),
            "{}",
            completed.message
        );
    }
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and packaged RAM helper; stops only its owned server"]
fn stopped_partition_roots_keep_controls_sync_and_shutdown_owned() {
    for kind in ["eject-local", "image-target", "read-only-target"] {
        for outcome in ["success", "cancel", "settings"] {
            stopped_case(kind, outcome);
        }
    }
}

fn stopped_case(kind: &str, outcome: &str) {
    let fixture = TestDrives::new(4096);
    let (backing, id, mut native, view) = fixture.native_source("partition-preparation");
    fixture.drives.lock().unwrap()[1].mount = Some(fixture.temp.path().join("large"));
    let armed = Arc::new(AtomicBool::new(false));
    let (entered, release, observe) = worker_observer("drive-partition-", armed.clone());
    let service = fixture.start_observed_reads(observe);
    let small = fixture.pair(&service, "small", "");
    let small_id = status(&service).pairs[0].id.clone();
    let destination = view.join("backup-target");
    let request = if kind == "eject-local" {
        finish(
            &service,
            DriveRequest::Pair {
                drive: "part:large".into(),
                relative: PathBuf::new(),
                local: view.clone(),
            },
        );
        DriveRequest::Eject {
            drive: "part:large".into(),
        }
    } else {
        fs::create_dir(&destination).unwrap();
        let review = backup_review::preview(
            &service,
            if kind == "image-target" {
                BackupKind::Image
            } else {
                BackupKind::Files
            },
            &destination,
        );
        if kind == "image-target" {
            backup_review::start(&review, false)
        } else {
            DriveRequest::StartReadOnlyBackup {
                token: review.token,
                remember_destination: false,
            }
        }
    };
    armed.store(true, Ordering::Release);
    let operation = accept(&service, request);
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            release.send(()).unwrap();
            let (tid, wait) = wait_fuse_request(|| {
                let mut tids = worker_threads("drive-partition");
                tids.extend(worker_threads("portable-drive-"));
                tids
            });
            eprintln!(
                "PARTITION_ROOTS_FUSE_BLOCKED kind={kind} outcome={outcome} tid={tid} wchan={wait}"
            );
            finish(
                &service,
                DriveRequest::Pause {
                    pair: small_id,
                    paused: false,
                },
            );
            fs::write(
                small.join("independent"),
                b"sync during stopped partition preparation",
            )
            .unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during stopped partition preparation",
            );
            assert!(worker_threads("drive-partition").contains(&tid));
            assert_unfinished(&service, &operation);
            if outcome == "settings" {
                let mut config = status(&service).config;
                config.sync_interval_seconds += 1;
                finish(&service, DriveRequest::Configure { config });
            }
            if outcome == "cancel" {
                service
                    .request(DriveRequest::Cancel {
                        operation: operation.clone(),
                    })
                    .unwrap();
                assert_unfinished(&service, &operation);
                closing = Some(shutdown_while_held(&service));
            }
        });
    }));
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    if result.is_ok() {
        let completed = wait_operation(&service, &operation, |op| op.finished.is_some());
        assert_stopped_outcome(&fixture, &service, &completed, &destination, kind, outcome);
    }
    service.shutdown().unwrap();
    finish_native_fixture(backing, id, native, result);
}

fn assert_stopped_outcome(
    fixture: &TestDrives,
    service: &Service,
    operation: &DriveOperation,
    destination: &Path,
    kind: &str,
    outcome: &str,
) {
    let expected = if outcome == "cancel" {
        "cancelled"
    } else if kind == "eject-local" && outcome == "success" {
        "complete"
    } else {
        "failed"
    };
    assert_eq!(
        operation.state, expected,
        "{kind}/{outcome}: {}",
        operation.message
    );
    if outcome != "success" {
        assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
        assert!(
            operation.partition.is_none(),
            "Preparation changed mounted state before acceptance"
        );
    }
    if outcome == "settings" {
        assert!(
            operation.message.contains("inputs changed"),
            "{}",
            operation.message
        );
    }
    if kind == "image-target" && outcome == "success" {
        assert!(
            operation
                .message
                .contains("owned image authorization declined"),
            "{}",
            operation.message
        );
        assert!(fixture.drives.lock().unwrap()[1].mount.is_none());
    }
    assert!(status(service).backups.is_empty());
    if kind != "eject-local" {
        assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
    }
}
