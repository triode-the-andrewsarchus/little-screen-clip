//! Read-only admission and compensation without privileged host mounts.
use super::*;

pub(super) fn request(drive: &str) -> DriveRequest {
    DriveRequest::ReadOnlyBackup {
        drive: drive.into(),
    }
}
fn assert_restored(
    fixture: &TestDrives,
    service: &Service,
    operation: &DriveOperation,
    original: &Drive,
) {
    let journal = operation.partition.as_ref().unwrap();
    assert_eq!(journal.file_mode, FileBackupMode::ReadOnly);
    assert_eq!(journal.phase, PartitionPhase::MountRestored);
    assert!(journal.restore_error.is_none());
    assert_eq!(&fixture.drives.lock().unwrap()[1], original);
    assert!(status(service).backups.is_empty());
}

#[test]
fn read_only_mode_cannot_consume_an_image_review_or_reuse_a_live_request_identity() {
    let (_fixture, service, destination, review) = backup_review::reviewed_files();
    let key = uuid::Uuid::new_v4();
    let live = backup_review::start(&review, false);
    service.submit(key, live).unwrap();
    wait_operation(&service, &key.to_string(), |op| op.finished.is_some());
    let changed = DriveRequest::StartReadOnlyBackup {
        token: review.token,
        remember_destination: false,
    };
    assert!(
        service
            .submit(key, changed)
            .unwrap_err()
            .to_string()
            .contains("different parameters")
    );
    let image = backup_review::preview(&service, BackupKind::Image, &destination);
    let before = status(&service).operations.len();
    let error = service
        .request(DriveRequest::StartReadOnlyBackup {
            token: image.token.clone(),
            remember_destination: false,
        })
        .unwrap_err();
    assert!(
        error.to_string().contains("requires a file backup review"),
        "{error}"
    );
    assert_eq!(status(&service).operations.len(), before);
    let previews = service.previews.lock().unwrap();
    assert_eq!(
        previews.get(&image.token).unwrap().info.kind,
        BackupKind::Image
    );
    drop(previews);
    failed(
        &service,
        backup_review::start(&image, false),
        "owned image authorization declined",
    );
    service.shutdown().unwrap();
}

#[test]
fn an_inventory_read_only_claim_cannot_replace_the_filesystem_guard_and_restores_access() {
    let fixture = TestDrives::new(4096);
    let original = fixture.drives.lock().unwrap()[1].clone();
    let mut authority = fixture.image_authority();
    let drives = fixture.drives.clone();
    let path = original.mount.clone();
    authority.mount = Arc::new(move |desired| {
        let mut drives = drives.lock().unwrap();
        let actual = drives
            .iter_mut()
            .find(|drive| drive.device == desired.device)
            .unwrap();
        actual.mount = path.clone();
        actual.read_only = desired.read_only;
        Ok(())
    });
    let service = fixture.start_authority(None, authority);
    let operation = failed(
        &service,
        request("part:large"),
        "Read-only collection requires",
    );
    assert_restored(&fixture, &service, &operation, &original);
    service.shutdown().unwrap();
}

#[test]
fn declined_read_only_mount_restores_the_original_access_without_copying() {
    let fixture = TestDrives::new(4096);
    let original = fixture.drives.lock().unwrap()[1].clone();
    let mut authority = fixture.image_authority();
    let mount = authority.mount.clone();
    authority.mount = Arc::new(move |drive| {
        if drive.read_only {
            Err("read-only authority declined".into())
        } else {
            mount(drive)
        }
    });
    let service = fixture.start_authority(None, authority);
    let operation = failed(
        &service,
        request("part:large"),
        "read-only authority declined",
    );
    assert_restored(&fixture, &service, &operation, &original);
    service.shutdown().unwrap();
}

#[test]
fn cancellation_during_accepted_unmount_restores_access_without_starting_read_only_mount() {
    let fixture = TestDrives::new(4096);
    let original = fixture.drives.lock().unwrap()[1].clone();
    let (mut authority, entered, release) = fixture.held_unmount_authority();
    let mount = authority.mount.clone();
    authority.mount = Arc::new(move |drive| {
        assert!(!drive.read_only);
        mount(drive)
    });
    let service = fixture.start_authority(None, authority);
    let cleanup_release = release;
    let id = accept(&service, request("part:large"));
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let operation = cancel_and_finish_released(&service, &id, cleanup_release);
    assert_eq!(operation.state, "cancelled", "{}", operation.message);
    assert_restored(&fixture, &service, &operation, &original);
    service.shutdown().unwrap();
}

#[test]
fn interrupted_read_only_journals_disclose_observed_mounts_without_replaying_authority() {
    let fixture = TestDrives::new(4096);
    let service = fixture.start_authority(None, fixture.image_authority());
    service.shutdown().unwrap();
    let observed = fixture.temp.path().join("last-observed-read-only");
    let db = Store::open(&fixture.temp.path().join("state/drives.sqlite")).unwrap();
    for phase in [
        PartitionPhase::MountingReadOnly,
        PartitionPhase::ReadOnlyMounted,
        PartitionPhase::CopyingFiles,
        PartitionPhase::FilesPublished,
        PartitionPhase::RestoringMount,
    ] {
        let id = format!("read-only-interruption-{phase:?}");
        db.put(
            "operation",
            &id,
            &DriveOperation {
                id: id.clone(),
                title: "Read-only collection".into(),
                state: "running".into(),
                message: String::new(),
                files: 0,
                bytes: 0,
                started: now(),
                finished: None,
                partition: Some(PartitionPreparation {
                    drive: fixture.drives.lock().unwrap()[1].clone(),
                    destination: None,
                    phase,
                    unmounted_by_job: true,
                    restore_mounted: true,
                    completed_backup: None,
                    restore_error: None,
                    file_mode: FileBackupMode::ReadOnly,
                    collection_mount: (phase != PartitionPhase::MountingReadOnly)
                        .then(|| observed.clone()),
                }),
            },
        )
        .unwrap();
    }
    drop(db);
    let mut authority = fixture.image_authority();
    authority.unmount = Arc::new(|_| panic!("restart replayed an unmount"));
    authority.mount = Arc::new(|_| panic!("restart replayed a mount"));
    let restarted = fixture.start_authority(None, authority);
    for operation in status(&restarted).operations {
        assert_eq!(operation.state, "interrupted");
        assert!(
            operation.message.contains("read-only"),
            "{}",
            operation.message
        );
        assert!(
            operation
                .message
                .contains("no mount action was replayed automatically")
        );
        if operation.partition.unwrap().collection_mount.is_some() {
            assert!(operation.message.contains("last-observed-read-only"));
        }
    }
    assert!(!observed.exists());
    restarted.shutdown().unwrap();
}

#[test]
fn live_compensation_keeps_the_completed_result_when_its_journal_is_unavailable() {
    let fixture = TestDrives::new(4096);
    let original = fixture.drives.lock().unwrap()[1].clone();
    let authority = fixture.image_authority();
    let context = partition::BackupContext {
        original: original.clone(),
        source: Arc::new(crate::device::Source::capture(&original).unwrap()),
        claims: Vec::new(),
        restore_mounted: true,
        unmounted_by_job: true,
        file_mode: FileBackupMode::ReadOnly,
        read_only_mount: None,
    };
    let drives = fixture.drives.clone();
    let inventory: Inventory = Arc::new(move || Ok(drives.lock().unwrap().clone()));
    let copied = fixture.temp.path().join("completed-payload");
    fs::write(&copied, b"retained completed bytes").unwrap();
    let db = Store::open(&fixture.temp.path().join("journal-without-operation")).unwrap();
    for journal in [Ok(&db), Err("could not open restoration journal".into())] {
        (authority.unmount)(&original).unwrap();
        let result = context
            .finish(
                journal,
                "missing-journal-entry",
                &inventory,
                &authority,
                Ok(format!("Backup completed: {}", copied.display())),
            )
            .unwrap();
        assert!(result.contains("Backup completed:"));
        assert!(result.contains("restoration checkpoint failed"));
        assert!(result.contains("restoration journal update failed"));
        assert_eq!(fixture.drives.lock().unwrap()[1], original);
        assert_eq!(fs::read(&copied).unwrap(), b"retained completed bytes");
    }
}
