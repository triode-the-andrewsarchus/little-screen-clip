//! Real read-only collection and compensation through the guest's private authority.
use super::*;
use crate::sync_edge_tests::native_fs;

fn held_mount(read_only: bool) -> (partition::Authority, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered, release, gate) = worker_gate();
    let mut authority = partition::Authority::default();
    let mount = authority.mount.clone();
    authority.mount = Arc::new(move |drive| {
        mount(drive)?;
        if drive.read_only == read_only {
            gate();
        }
        Ok(())
    });
    (authority, entered, release)
}
fn config(temp: &Path) -> DriveConfig {
    DriveConfig {
        backup_directory: Some(temp.join("backups")),
        ..Default::default()
    }
}

#[test]
#[ignore = "requires the isolated partition/authority guest"]
fn native_read_only_backup_restores_access_and_retains_completed_copy_after_late_cancel() {
    let original = native_images::selected(&native_fs::guest_partition());
    let temp = tempfile::tempdir().unwrap();
    let name = format!("readonly-{}.txt", uuid::Uuid::new_v4());
    let payload = b"read-only collection payload";
    let source = original.mount.as_ref().unwrap().join(&name);
    fs::write(&source, payload).unwrap();
    let (authority, entered, release) = held_mount(false);
    let service = Service::start_with_authority(
        config(temp.path()),
        temp.path().join("state"),
        Arc::new(discovery::discover),
        None,
        Arc::new(authority),
    )
    .unwrap();
    // Release the gate before Service joins its owner during unwinding.
    let cleanup_release = release;
    let busy = File::open(&source).unwrap();
    native_images::assert_busy_refusal(&service, read_only::request(&original.id));
    drop(busy);

    let destination = temp.path().join("backups");
    let preview = match service
        .request(DriveRequest::PreviewBackup {
            drive: original.id.clone(),
            kind: BackupKind::Files,
            destination: Some(destination),
        })
        .unwrap()
    {
        DriveReply::BackupPreview(preview) => preview,
        _ => panic!("expected reviewed selection"),
    };
    let id = accept(
        &service,
        DriveRequest::StartReadOnlyBackup {
            token: preview.token.clone(),
            remember_destination: false,
        },
    );
    entered.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_unfinished(&service, &id);
    let first = status(&service).backups.pop().unwrap();
    assert_eq!(fs::read(first.path.join(&name)).unwrap(), payload);
    assert!(!native_images::selected(&original).read_only);
    let operation = cancel_and_finish_released(&service, &id, cleanup_release);
    assert_eq!(operation.state, "complete", "{}", operation.message);
    assert!(
        operation
            .message
            .contains("verified read-only filesystem collection")
    );
    let preparation = operation.partition.unwrap();
    assert_eq!(preparation.file_mode, FileBackupMode::ReadOnly);
    assert_eq!(preparation.phase, PartitionPhase::MountRestored);
    assert!(preparation.collection_mount.is_some());
    assert_eq!(
        preparation.completed_backup.as_deref(),
        Some(first.id.as_str())
    );
    assert!(preparation.restore_error.is_none());
    let manifest = Store::existing(first.manifest.as_ref().unwrap(), true).unwrap();
    assert_eq!(
        native_images::digest(File::open(first.manifest.as_ref().unwrap()).unwrap()),
        first.manifest_sha256.as_ref().unwrap().as_str()
    );
    assert_eq!(
        manifest
            .get::<String>("collection", "consistency")
            .unwrap()
            .as_deref(),
        Some(
            "Read-only filesystem collection; source mount and superblock checked through publication"
        )
    );
    let captured: Drive = manifest.get("source", "drive").unwrap().unwrap();
    assert!(captured.read_only);

    // An originally read-only partition remains read-only after a second job.
    let mounted = native_images::selected(&original);
    partition::unmount(&mounted).unwrap();
    let mut desired = mounted;
    desired.read_only = true;
    (partition::Authority::default().mount)(&desired).unwrap();
    assert!(native_images::selected(&original).read_only);
    finish(&service, read_only::request(&original.id));
    assert!(native_images::selected(&original).read_only);
    assert_eq!(status(&service).backups.len(), 2);
    service.shutdown().unwrap();
    let mounted = native_images::selected(&original);
    partition::unmount(&mounted).unwrap();
    let mut desired = mounted;
    desired.read_only = false;
    (partition::Authority::default().mount)(&desired).unwrap();
    let restored = native_images::selected(&original);
    assert!(!restored.read_only);
    assert_eq!(
        fs::read(restored.mount.unwrap().join(name)).unwrap(),
        payload
    );
}

#[test]
#[ignore = "requires the isolated partition/authority guest"]
fn native_cancel_after_read_only_mount_restores_access_and_keeps_other_pair_responsive() {
    let original = native_images::selected(&native_fs::guest_partition());
    let temp = tempfile::tempdir().unwrap();
    let independent = temp.path().join("independent");
    fs::create_dir(&independent).unwrap();
    fs::write(independent.join("payload"), b"independent contents").unwrap();
    let local = temp.path().join("local");
    let fake = fixture_drive(&independent, "independent");
    let inventory: Inventory = Arc::new(move || {
        let mut drives = discovery::discover()?;
        drives.push(fake.clone());
        Ok(drives)
    });
    let (authority, entered, release) = held_mount(true);
    let service = Service::start_with_authority(
        config(temp.path()),
        temp.path().join("state"),
        inventory,
        None,
        Arc::new(authority),
    )
    .unwrap();
    let cleanup_release = release;
    let id = accept(&service, read_only::request(&original.id));
    entered.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_unfinished(&service, &id);
    assert!(native_images::selected(&original).read_only);
    finish(
        &service,
        DriveRequest::Pair {
            drive: "part:independent".into(),
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    assert_eq!(
        fs::read(local.join("payload")).unwrap(),
        b"independent contents"
    );
    let operation = cancel_and_finish_released(&service, &id, cleanup_release);
    assert_eq!(operation.state, "cancelled", "{}", operation.message);
    assert_eq!(
        operation.partition.unwrap().phase,
        PartitionPhase::MountRestored
    );
    assert!(status(&service).backups.is_empty());
    assert!(!native_images::selected(&original).read_only);
    service.shutdown().unwrap();
}
