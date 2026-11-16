//! Backup review fixtures use private directories and metadata-only device stand-ins.
use super::*;

fn preview(service: &Service, kind: BackupKind, destination: &Path) -> BackupPreview {
    match service
        .request(DriveRequest::PreviewBackup {
            drive: "part:large".into(),
            kind,
            destination: Some(destination.into()),
        })
        .unwrap()
    {
        DriveReply::BackupPreview(preview) => *preview,
        _ => panic!("expected backup review"),
    }
}
fn start(preview: &BackupPreview, remember_destination: bool) -> DriveRequest {
    DriveRequest::StartBackup {
        token: preview.token.clone(),
        restore_mounted: false,
        remember_destination,
    }
}
fn destination(fixture: &TestDrives) -> PathBuf {
    let path = fixture.temp.path().join("reviewed-backup");
    fs::create_dir(&path).unwrap();
    path
}

fn reviewed_files() -> (TestDrives, Service, PathBuf, BackupPreview) {
    let fixture = TestDrives::new(4096);
    let service = fixture.start_authority(None, fixture.image_authority());
    let path = destination(&fixture);
    let review = preview(&service, BackupKind::Files, &path);
    (fixture, service, path, review)
}

#[test]
fn cancelling_a_review_creates_no_job_or_preference_and_missing_paths_are_not_created() {
    let fixture = TestDrives::new(4096);
    let service = fixture.start_authority(None, fixture.image_authority());
    let path = destination(&fixture);
    let before = status(&service);
    let review = preview(&service, BackupKind::Files, &path);
    assert_eq!(review.drive.capacity, 1024 * 1024);
    assert!(review.space.available_bytes > 0);
    service
        .request(DriveRequest::DiscardBackupReview {
            token: review.token.clone(),
        })
        .unwrap();
    assert!(service.request(start(&review, true)).is_err());
    let missing = path.join("not-created-by-review");
    assert!(
        service
            .request(DriveRequest::PreviewBackup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                destination: Some(missing.clone())
            })
            .is_err()
    );
    assert!(!missing.exists());
    let after = status(&service);
    assert_eq!(after.operations.len(), before.operations.len());
    assert_eq!(after.backup_directory, before.backup_directory);
    assert!(fs::read_dir(&path).unwrap().next().is_none());
    service.shutdown().unwrap();
}

#[test]
fn reviewed_destination_survives_preference_changes_and_is_used_once_without_changing_them() {
    let (fixture, service, path, review) = reviewed_files();
    let replacement = fixture.temp.path().join("future-backups");
    finish(
        &service,
        DriveRequest::BackupDirectory {
            path: replacement.clone(),
        },
    );
    finish(&service, start(&review, false));
    let saved = status(&service);
    assert_eq!(saved.backup_directory, Some(replacement));
    assert_eq!(saved.backups.len(), 1);
    let backup = &saved.backups[0];
    assert!(backup.path.starts_with(&path));
    assert_eq!(
        fs::read(backup.path.join("large-sparse")).unwrap(),
        vec![0; 4096]
    );
    assert!(service.request(start(&review, false)).is_err());
    // A one-shot destination stays managed after completion and after restart.
    let config = saved.config;
    service.shutdown().unwrap();
    let drives = fixture.drives.clone();
    let restarted = Service::start_with_inventory(
        config,
        fixture.temp.path().join("state"),
        Arc::new(move || Ok(drives.lock().unwrap().clone())),
    )
    .unwrap();
    let policy: IndexPolicy = serde_json::from_slice(
        &fs::read(fixture.temp.path().join("state/index-policy.json")).unwrap(),
    )
    .unwrap();
    assert!(policy.excluded.contains(&path));
    let operation = accept(
        &restarted,
        DriveRequest::Pair {
            drive: "part:small".into(),
            relative: PathBuf::new(),
            local: path.join("must-not-sync"),
        },
    );
    let operation = wait_operation(&restarted, &operation, |op| op.finished.is_some());
    assert_eq!(operation.state, "failed");
    assert!(
        operation.message.contains("managed backups"),
        "{}",
        operation.message
    );
    restarted.shutdown().unwrap();
}

#[test]
fn replaced_reviewed_roots_and_expired_reviews_fail_before_copy_or_preference_changes() {
    for changed in ["source", "destination", "expiry"] {
        let (fixture, service, path, review) = reviewed_files();
        if changed == "expiry" {
            service
                .previews
                .lock()
                .unwrap()
                .get_mut(&review.token)
                .unwrap()
                .created = Instant::now() - preflight::LIFETIME;
            assert!(service.request(start(&review, true)).is_err());
            assert!(status(&service).backups.is_empty());
            service.shutdown().unwrap();
            continue;
        } else {
            let replaced = if changed == "source" {
                fixture.temp.path().join("large")
            } else {
                path.clone()
            };
            fs::rename(&replaced, replaced.with_extension("old")).unwrap();
            fs::create_dir(&replaced).unwrap();
        }
        let id = accept(&service, start(&review, true));
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(
            operation.state, "failed",
            "{changed}: {}",
            operation.message
        );
        assert!(status(&service).backups.is_empty());
        assert_ne!(status(&service).backup_directory, Some(path.clone()));
        assert!(fs::read_dir(path).unwrap().next().is_none());
        service.shutdown().unwrap();
    }
}

#[test]
fn changed_partition_reviews_fail_before_unmount_and_image_authority() {
    for changed in ["disk sequence", "capacity", "evidence"] {
        let fixture = TestDrives::new(4096);
        let mut authority = fixture.image_authority();
        authority.unmount = Arc::new(|_| panic!("changed review must not request unmount"));
        authority.open_image =
            Arc::new(|_, _| panic!("changed review must not request an image descriptor"));
        let service = fixture.start_authority(None, authority);
        let path = destination(&fixture);
        let review = preview(&service, BackupKind::Image, &path);
        assert_eq!(
            review.space.full_image_bytes,
            Some(review.drive.capacity + 64 * 1024 * 1024)
        );
        {
            let mut drives = fixture.drives.lock().unwrap();
            let drive = &mut drives[1];
            match changed {
                "disk sequence" => drive.disk_sequence = Some(2),
                "capacity" => drive.capacity += 512,
                _ => drive.evidence.push_str("replacement"),
            }
        }
        let id = accept(&service, start(&review, true));
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(operation.message.contains("changed since review"));
        assert!(status(&service).backups.is_empty());
        assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
        service.shutdown().unwrap();
    }
}

#[test]
fn failed_preference_write_never_starts_the_reviewed_backup() {
    for kind in [BackupKind::Files, BackupKind::Image] {
        let fixture = TestDrives::new(4096);
        let path = destination(&fixture);
        let settings = fixture.temp.path().join("settings.toml");
        let contents = "# owned malformed settings stay unchanged\n[drives\n";
        fs::write(&settings, contents).unwrap();
        let drives = fixture.drives.clone();
        let mut authority = fixture.image_authority();
        authority.unmount = Arc::new(|_| panic!("preference failure must precede unmount"));
        let service = Service::start_with_authority(
            DriveConfig {
                backup_directory: Some(fixture.temp.path().join("old-backup")),
                ..Default::default()
            },
            fixture.temp.path().join("state"),
            Arc::new(move || Ok(drives.lock().unwrap().clone())),
            Some(settings.clone()),
            Arc::new(authority),
        )
        .unwrap();
        let review = preview(&service, kind, &path);
        let id = accept(&service, start(&review, true));
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(
            operation
                .message
                .contains("Could not save backup directory preference"),
            "{}",
            operation.message
        );
        assert!(status(&service).backups.is_empty());
        assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
        assert!(fs::read_dir(&path).unwrap().next().is_none());
        assert_eq!(fs::read_to_string(settings).unwrap(), contents);
        service.shutdown().unwrap();
    }
}

#[test]
fn review_storage_is_bounded_expires_and_is_cleared_by_shutdown() {
    let fixture = TestDrives::new(4096);
    let service = fixture.start_authority(None, fixture.image_authority());
    let path = destination(&fixture);
    for _ in 0..16 {
        preview(&service, BackupKind::Files, &path);
    }
    assert!(
        service
            .request(DriveRequest::PreviewBackup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                destination: Some(path.clone())
            })
            .is_err()
    );
    for review in service.previews.lock().unwrap().values_mut() {
        review.created = Instant::now() - preflight::LIFETIME;
    }
    preview(&service, BackupKind::Files, &path);
    assert_eq!(service.previews.lock().unwrap().len(), 1);
    service.shutdown().unwrap();
    assert!(service.previews.lock().unwrap().is_empty());
    assert!(
        service
            .request(DriveRequest::PreviewBackup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                destination: Some(path)
            })
            .is_err()
    );
}

#[test]
fn independent_image_space_is_checked_before_unmount_for_reviewed_and_direct_requests() {
    let destination = tempfile::tempdir_in("/dev/shm").unwrap();
    refused_image_destination(destination.path(), u64::MAX, "available bytes");
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_fat32_image_size_is_checked_before_unmount() {
    let (destination, _native_fixture, drive) = crate::sync_edge_tests::native_fs::fixture();
    if drive.filesystem == "vfat" {
        let root = Root::open(destination.path()).unwrap();
        root.check_file_size(u32::MAX as u64).unwrap();
        refused_image_destination(destination.path(), u32::MAX as u64 + 1, "FAT32");
    }
}

fn refused_image_destination(destination: &Path, capacity: u64, reason: &str) {
    let fixture = TestDrives::new(4096);
    fixture.drives.lock().unwrap()[1].capacity = capacity;
    let mut authority = fixture.image_authority();
    authority.unmount = Arc::new(|_| panic!("destination refusal must precede unmount"));
    let service = fixture.start_authority(None, authority);
    let error = service
        .request(DriveRequest::PreviewBackup {
            drive: "part:large".into(),
            kind: BackupKind::Image,
            destination: Some(destination.into()),
        })
        .unwrap_err();
    assert!(error.to_string().contains(reason), "{error}");
    finish(
        &service,
        DriveRequest::BackupDirectory {
            path: destination.into(),
        },
    );
    let operation = completed(
        &service,
        DriveRequest::Backup {
            drive: "part:large".into(),
            kind: BackupKind::Image,
            restore_mounted: false,
        },
    );
    assert_eq!(operation.state, "failed");
    assert!(operation.message.contains(reason), "{}", operation.message);
    assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
    assert!(status(&service).backups.is_empty());
    assert!(fs::read_dir(destination).unwrap().next().is_none());
    service.shutdown().unwrap();
}

#[test]
fn a_failed_one_shot_publication_retains_its_destination_exclusion() {
    let (_fixture, service, path, review) = reviewed_files();
    fs::create_dir_all(path.join("large/current-files.json")).unwrap();
    let operation = completed(&service, start(&review, false));
    assert_eq!(operation.state, "failed");
    assert!(status(&service).backups.is_empty());
    let policy: IndexPolicy =
        serde_json::from_slice(&fs::read(service.state.join("index-policy.json")).unwrap())
            .unwrap();
    assert!(policy.excluded.contains(&path));
    let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
    assert!(
        preflight::managed_backup_paths(&db)
            .unwrap()
            .contains(&path)
    );
    assert_ne!(status(&service).backup_directory, Some(path));
    service.shutdown().unwrap();
}

#[test]
fn a_readable_but_unwritable_destination_is_refused_without_creating_a_job() {
    if rustix::process::geteuid().is_root() {
        eprintln!("unprivileged directory-permission fixture is unavailable as root");
        return;
    }
    let fixture = TestDrives::new(4096);
    let service = fixture.start_authority(None, fixture.image_authority());
    let path = destination(&fixture);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).unwrap();
    let error = service
        .request(DriveRequest::PreviewBackup {
            drive: "part:large".into(),
            kind: BackupKind::Image,
            destination: Some(path.clone()),
        })
        .unwrap_err();
    assert!(
        error.to_string().contains("not writable/searchable"),
        "{error}"
    );
    assert!(status(&service).operations.is_empty());
    assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires an explicitly selected writable Btrfs fixture directory"]
fn a_btrfs_review_allows_shared_allocation_when_an_independent_image_would_not_fit() {
    let base = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR")
        .expect("set LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR");
    let destination = tempfile::Builder::new()
        .prefix("little-clip-review-")
        .tempdir_in(base)
        .unwrap();
    let fixture = TestDrives::new(4096);
    fixture.drives.lock().unwrap()[1].capacity = u64::MAX;
    let service = fixture.start_authority(None, fixture.image_authority());
    let review = preview(&service, BackupKind::Image, destination.path());
    assert!(review.space.btrfs);
    assert!(review.space.full_image_bytes.unwrap() > review.space.available_bytes);
    assert!(status(&service).operations.is_empty());
    service
        .request(DriveRequest::DiscardBackupReview {
            token: review.token,
        })
        .unwrap();
    assert!(fs::read_dir(destination.path()).unwrap().next().is_none());
    service.shutdown().unwrap();
}
