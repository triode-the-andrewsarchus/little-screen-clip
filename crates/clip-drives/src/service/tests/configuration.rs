//! Configuration preparation keeps its filesystem effects separate from metadata publication.
use super::*;

#[test]
fn configuration_backup_guard_keeps_legacy_pair_roots_beyond_the_status_page() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let previous = status(&service).config;
    finish(
        &service,
        DriveRequest::Configure {
            config: previous.clone(),
        },
    );
    let destination = fixture.temp.path().join("owned-roots/oldest-paired-folder");
    let other = fixture.temp.path().join("other-paired-folder");
    fs::create_dir_all(&destination).unwrap();
    fs::create_dir(&other).unwrap();
    fs::write(destination.join("user-file"), b"retain legacy contents").unwrap();
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let transaction = db.0.unchecked_transaction().unwrap();
    for number in 0..1101 {
        let pair = SyncPair {
            id: format!("legacy-{number:04}"),
            drive: "part:offline".into(),
            evidence: "owned".into(),
            relative: PathBuf::new(),
            local: if number == 0 {
                destination.clone()
            } else {
                other.clone()
            },
            local_identity: None,
            paused: true,
            deletions: false,
            ram: false,
            last_success: None,
            status: "Retained legacy enrollment".into(),
        };
        db.put("pair", &pair.id, &pair).unwrap();
    }
    transaction.commit().unwrap();
    assert!(
        db.list::<SyncPair>("pair", 0, 1000)
            .unwrap()
            .iter()
            .all(|pair| pair.local != destination)
    );
    let mut config = previous.clone();
    config.backup_directory = Some(destination.clone());
    failed(
        &service,
        DriveRequest::Configure { config },
        "overlaps a paired local folder",
    );
    assert_eq!(status(&service).config, previous);
    assert_eq!(
        fs::read(destination.join("user-file")).unwrap(),
        b"retain legacy contents"
    );
    fs::remove_file(destination.join("user-file")).unwrap();
    fs::remove_dir(&destination).unwrap();
    let mut config = previous.clone();
    config.backup_directory = Some(destination.parent().unwrap().to_path_buf());
    failed(
        &service,
        DriveRequest::Configure { config },
        "overlaps a paired local folder",
    );
    assert_eq!(status(&service).config, previous);
    service.shutdown().unwrap();
}

#[test]
fn basic_configuration_reload_remains_available_when_optional_view_metadata_is_uncertain() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    db.put("catalog-mount", "missing-header", &1_u64).unwrap();
    let mut config = status(&service).config;
    assert!(config.backup_directory.is_none());
    config.sync_interval_seconds += 1;
    finish(
        &service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    assert_eq!(status(&service).config, config);
    db.remove("catalog-mount", "missing-header").unwrap();
    service.shutdown().unwrap();
}

#[test]
fn failed_configuration_publication_preserves_settings_and_retry_uses_the_retained_directory() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let previous = status(&service).config;
    let mut config = previous.clone();
    let destination = fixture.temp.path().join("new-parent/backups");
    config.backup_directory = Some(destination.clone());
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let saved = db.get::<DriveConfig>("settings", "config").unwrap();
    db.0.execute_batch("CREATE TRIGGER reject_configuration BEFORE INSERT ON entities WHEN NEW.kind='settings' BEGIN SELECT RAISE(ABORT,'owned configuration publication fault'); END;").unwrap();
    failed(
        &service,
        DriveRequest::Configure {
            config: config.clone(),
        },
        "owned configuration publication fault",
    );
    assert_eq!(status(&service).config, previous);
    assert_eq!(db.get::<DriveConfig>("settings", "config").unwrap(), saved);
    assert!(destination.is_dir());
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
    db.0.execute_batch("DROP TRIGGER reject_configuration")
        .unwrap();
    finish(
        &service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    assert_eq!(status(&service).config, config);
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned server"]
fn stopped_configuration_destination_keeps_controls_sync_and_shutdown_owned() {
    for outcome in ["success", "cancel", "settings", "enrollment"] {
        configuration_case(outcome);
    }
}

fn configuration_case(outcome: &str) {
    let fixture = TestDrives::new(0);
    let (backing, id, mut native, view) = fixture.native_source("backup-target");
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let local = fixture.pair(&service, "small", "");
    let pair = status(&service).pairs[0].id.clone();
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let mut seeded: SyncPair = db.get("pair", &pair).unwrap().unwrap();
    seeded.last_success = Some(1);
    db.put("pair", &pair, &seeded).unwrap();
    let previous = status(&service).config;
    let mut config = previous.clone();
    config.backup_directory = Some(view.join("created/backups"));
    let mut operation = None;
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            let accepted = accept(
                &service,
                DriveRequest::Configure {
                    config: config.clone(),
                },
            );
            operation = Some(accepted.clone());
            let (tid, wait) = wait_fuse_request(|| {
                let mut tids = worker_threads("drive-settings-");
                tids.extend(worker_threads("portable-drive-"));
                tids
            });
            eprintln!("CONFIGURATION_PREPARATION_FUSE_BLOCKED tid={tid} wchan={wait}");
            finish(
                &service,
                DriveRequest::Pause {
                    pair: pair.clone(),
                    paused: false,
                },
            );
            fs::write(
                local.join("independent"),
                b"sync during settings preparation",
            )
            .unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during settings preparation",
            );
            finish(&service, DriveRequest::Sync { pair: pair.clone() });
            assert_ne!(
                db.get::<SyncPair>("pair", &pair)
                    .unwrap()
                    .unwrap()
                    .last_success,
                Some(1)
            );
            assert!(worker_threads("drive-settings-").contains(&tid));
            assert_unfinished(&service, &accepted);
            match outcome {
                "cancel" => {
                    let ram = accept(
                        &service,
                        DriveRequest::CopyToRam {
                            drive: "part:large".into(),
                        },
                    );
                    service
                        .request(DriveRequest::Cancel {
                            operation: ram.clone(),
                        })
                        .unwrap();
                    assert_eq!(
                        wait_operation(&service, &ram, |op| op.finished.is_some()).state,
                        "cancelled"
                    );
                    service
                        .request(DriveRequest::Cancel {
                            operation: accepted.clone(),
                        })
                        .unwrap();
                    assert_unfinished(&service, &accepted);
                    closing = Some(shutdown_while_held(&service));
                }
                "settings" => {
                    finish(
                        &service,
                        DriveRequest::BackupDirectory {
                            path: fixture.temp.path().join("other-backups"),
                        },
                    );
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
                _ => (),
            }
        });
    }));
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    if result.is_ok() {
        let completed = wait_operation(&service, operation.as_ref().unwrap(), |op| {
            op.finished.is_some()
        });
        let current = status(&service).config;
        match outcome {
            "success" => {
                assert_eq!(completed.state, "complete", "{}", completed.message);
                assert_eq!(current, config);
            }
            "cancel" => {
                assert_eq!(completed.state, "cancelled", "{}", completed.message);
                assert_eq!(current, previous);
            }
            _ => {
                assert_eq!(completed.state, "failed", "{}", completed.message);
                assert!(
                    completed.message.contains("Configuration inputs changed"),
                    "{}",
                    completed.message
                );
                if outcome == "settings" {
                    assert_eq!(
                        current.backup_directory,
                        Some(fixture.temp.path().join("other-backups"))
                    );
                } else {
                    assert_eq!(current, previous);
                }
            }
        }
        assert!(status(&service).ram.is_empty());
    }
    service.shutdown().unwrap();
    finish_native_fixture(backing, id, native, result);
}
