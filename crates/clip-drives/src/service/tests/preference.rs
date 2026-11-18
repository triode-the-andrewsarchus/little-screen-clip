//! Preference effects and backup continuations keep their accepted completion.
use super::*;

fn settings_service(fixture: &TestDrives, settings: &Path) -> Service {
    let drives = fixture.drives.clone();
    Service::start_with_authority(
        DriveConfig {
            ram_min_available: u64::MAX,
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        fixture.temp.path().join("state"),
        Arc::new(move || Ok(drives.lock().unwrap().clone())),
        Some(settings.into()),
        Arc::new(fixture.image_authority()),
        Default::default(),
    )
    .unwrap()
}

#[test]
fn saved_preference_file_is_not_hidden_by_a_later_registry_failure() {
    for file in [false, true] {
        let fixture = TestDrives::new(0);
        let settings = fixture.temp.path().join("settings.toml");
        fs::write(
            &settings,
            "# preserve this preference\n[other]\nvalue = 9\n",
        )
        .unwrap();
        let service = if file {
            settings_service(&fixture, &settings)
        } else {
            fixture.start_observed_reads(Arc::new(|| {}))
        };
        let previous = status(&service).config;
        let target = fixture.temp.path().join("selected");
        let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
        db.0.execute_batch("CREATE TRIGGER reject_preference BEFORE INSERT ON entities WHEN NEW.kind='settings' BEGIN SELECT RAISE(ABORT,'owned preference registry fault'); END;").unwrap();
        let result = completed(
            &service,
            DriveRequest::BackupDirectory {
                path: target.clone(),
            },
        );
        assert_eq!(result.state, "failed", "{}", result.message);
        assert!(
            result.message.contains("owned preference registry fault"),
            "{}",
            result.message
        );
        assert_eq!(
            status(&service).backup_directory,
            if file {
                Some(target.clone())
            } else {
                previous.backup_directory
            }
        );
        if file {
            assert!(result.message.contains("file saved"));
            assert_eq!(
                little_clip_core::AppConfig::load_from_path(&settings)
                    .unwrap()
                    .drives
                    .backup_directory,
                Some(target)
            );
            assert!(
                fs::read_to_string(&settings)
                    .unwrap()
                    .contains("# preserve this preference")
            );
        }
        service.shutdown().unwrap();
    }
}

#[test]
fn remembered_destination_survives_backup_completion_and_later_image_refusal() {
    for kind in [BackupKind::Files, BackupKind::Image] {
        let fixture = TestDrives::new(4096);
        let settings = fixture.temp.path().join("settings.toml");
        fs::write(&settings, "# selected only after review\n").unwrap();
        let service = settings_service(&fixture, &settings);
        let target = backup_review::destination(&fixture);
        let review = backup_review::preview(&service, kind, &target);
        let result = completed(&service, backup_review::start(&review, true));
        assert_eq!(
            result.state,
            if kind == BackupKind::Files {
                "complete"
            } else {
                "failed"
            },
            "{}",
            result.message
        );
        assert!(
            result.message.contains("preference saved"),
            "{}",
            result.message
        );
        let saved = status(&service);
        assert_eq!(saved.backup_directory, Some(target.clone()));
        assert_eq!(
            little_clip_core::AppConfig::load_from_path(&settings)
                .unwrap()
                .drives
                .backup_directory,
            Some(target.clone())
        );
        if kind == BackupKind::Files {
            assert_eq!(saved.backups.len(), 1);
            assert!(saved.backups[0].path.starts_with(&target));
            assert_eq!(
                fs::read(saved.backups[0].path.join("large-sparse")).unwrap(),
                vec![0; 4096]
            );
        } else {
            assert!(
                result
                    .message
                    .contains("owned image authorization declined")
            );
            assert!(saved.backups.is_empty());
        }
        service.shutdown().unwrap();
    }
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned server"]
fn stopped_preference_file_keeps_controls_and_preserves_completed_saves_after_cancel() {
    for remember in [false, true] {
        stopped_file_case(remember);
    }
}

fn stopped_file_case(remember: bool) {
    let fixture = TestDrives::new(4096);
    let (backing, source_id, mut native, view) = fixture.native_source("settings");
    // The stopped filesystem contains only the configuration file. Backup
    // source/destination and the independent pair remain ordinary owned paths.
    fixture.drives.lock().unwrap()[1].mount = Some(fixture.temp.path().join("large"));
    let settings = view.join("config.toml");
    fs::write(
        &settings,
        "# preserve unrelated settings\n[other]\nvalue = 9\n",
    )
    .unwrap();
    let service = settings_service(&fixture, &settings);
    let small = fixture.pair(&service, "small", "");
    let pair = status(&service).pairs[0].id.clone();
    let target = fixture.temp.path().join("chosen-backups");
    fs::create_dir(&target).unwrap();
    let request = if remember {
        let review = backup_review::preview(&service, BackupKind::Files, &target);
        backup_review::start(&review, true)
    } else {
        DriveRequest::BackupDirectory {
            path: target.clone(),
        }
    };
    let key = uuid::Uuid::new_v4();
    let mut operation = None;
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            let id = match service.submit(key, request.clone()).unwrap() {
                DriveReply::Accepted(id) => id,
                _ => panic!("accepted preference"),
            };
            operation = Some(id.clone());
            let (tid, wait) = wait_fuse_request(|| {
                let mut tids = worker_threads("drive-prefer");
                tids.extend(worker_threads("portable-drive-"));
                tids
            });
            eprintln!("PREFERENCE_FILE_FUSE_BLOCKED remember={remember} tid={tid} wchan={wait}");
            finish(
                &service,
                DriveRequest::Pause {
                    pair: pair.clone(),
                    paused: false,
                },
            );
            fs::write(
                small.join("independent"),
                b"sync during preference file I/O",
            )
            .unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during preference file I/O",
            );
            assert!(worker_threads("drive-prefer").contains(&tid));
            service
                .request(DriveRequest::Cancel {
                    operation: id.clone(),
                })
                .unwrap();
            assert_unfinished(&service, &id);
            assert!(status(&service).backup_directory.is_none());
            assert!(status(&service).backups.is_empty());
            closing = Some(shutdown_while_held(&service));
        });
    }));
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    } else {
        service.shutdown().unwrap();
    }
    if result.is_ok() {
        let id = operation.unwrap();
        let completed = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(
            completed.state,
            if remember { "cancelled" } else { "complete" },
            "{}",
            completed.message
        );
        if remember {
            assert!(
                completed.message.contains("preference saved"),
                "{}",
                completed.message
            );
        }
        assert_eq!(status(&service).backup_directory, Some(target.clone()));
        assert!(status(&service).backups.is_empty());
        assert_eq!(
            fs::read_dir(&target).unwrap().count(),
            0,
            "Cancelled continuation copied backup files"
        );
        let config = little_clip_core::AppConfig::load_from_path(&settings)
            .unwrap()
            .drives;
        assert_eq!(config.backup_directory, Some(target));
        let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
        let mut interrupted: DriveOperation = db.get("operation", &id).unwrap().unwrap();
        interrupted.finished = None;
        db.put("operation", &id, &interrupted).unwrap();
        let drives = fixture.drives.clone();
        let restarted = Service::start_with_settings(
            config,
            service.state.clone(),
            Arc::new(move || Ok(drives.lock().unwrap().clone())),
            Some(settings.clone()),
        )
        .unwrap();
        let recovered = wait_operation(&restarted, &id, |op| op.finished.is_some());
        assert_eq!(recovered.state, "interrupted");
        if remember {
            assert!(recovered.message.contains("preference saved"));
        }
        match restarted.submit(key, request).unwrap() {
            DriveReply::Accepted(replayed) => assert_eq!(replayed, id),
            _ => panic!("expected the retained preference operation"),
        }
        assert!(status(&restarted).backups.is_empty());
        restarted.shutdown().unwrap();
    }
    finish_native_fixture(backing, source_id, native, result);
}
