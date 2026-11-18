//! Explicit filesystem-UUID enrollment keeps its local state and backup bucket.
use super::*;
use crate::sync_edge_tests::native_fs;
use std::collections::BTreeSet;

#[test]
fn media_without_either_uuid_cannot_create_automatic_enrollment() {
    let fixture = TestDrives::new(0);
    let id = {
        let mut drives = fixture.drives.lock().unwrap();
        let drive = &mut drives[0];
        drive.part_uuid = None;
        drive.filesystem_uuid = None;
        drive.id = format!("device:{}", drive.device.display());
        drive.id.clone()
    };
    let service = fixture.start_authority(None, fixture.image_authority());
    let local = fixture.temp.path().join("unidentified-target");
    failed(
        &service,
        DriveRequest::Pair {
            drive: id,
            relative: PathBuf::new(),
            local: local.clone(),
        },
        "persistent media identity",
    );
    assert!(status(&service).pairs.is_empty());
    assert!(!local.exists());
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires an owned native filesystem without a partition table in the disposable guest"]
fn native_filesystem_uuid_enrollment_and_backups_survive_restart_without_inventing_a_partuuid() {
    let drive = native_fs::guest_drive();
    assert!(drive.part_uuid.is_none());
    assert!(drive.filesystem_uuid.is_some());
    let source = drive.mount.as_ref().unwrap();
    fs::write(source.join("portable.txt"), b"portable before enrollment").unwrap();
    let mut names = fs::read_dir(source)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<BTreeSet<_>>();
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let local = temp.path().join("local");
    let backups = temp.path().join("backups");
    let observed = drive.clone();
    let inventory: Inventory = Arc::new(move || Ok(vec![observed.clone()]));
    let config = DriveConfig {
        ram_min_available: u64::MAX,
        ..Default::default()
    };
    let service =
        Service::start_with_inventory(config.clone(), state.clone(), inventory.clone()).unwrap();
    assert!(status(&service).pairs.is_empty());
    assert!(status(&service).backups.is_empty());
    assert!(
        !local.exists() && !backups.exists(),
        "Discovery cannot enroll or back up implicitly"
    );
    let pair = enroll_pair(&service, &drive.id, Path::new(""), &local);
    assert_eq!(
        pair.drive,
        format!("fs:{}", drive.filesystem_uuid.as_ref().unwrap())
    );
    finish(
        &service,
        DriveRequest::Sync {
            pair: pair.id.clone(),
        },
    );
    fs::write(local.join("local-only.txt"), b"explicit enrolled union").unwrap();
    finish(
        &service,
        DriveRequest::Sync {
            pair: pair.id.clone(),
        },
    );
    finish(
        &service,
        DriveRequest::BackupDirectory {
            path: backups.clone(),
        },
    );
    let request = DriveRequest::Backup {
        drive: drive.id.clone(),
        kind: BackupKind::Files,
        restore_mounted: false,
    };
    finish(&service, request.clone());
    let first = status(&service).backups.pop().unwrap();
    let bucket = backups.join(format!("fs-{}", drive.filesystem_uuid.as_ref().unwrap()));
    assert_eq!(first.path.parent(), Some(bucket.as_path()));
    assert_eq!(
        fs::read(first.path.join("portable.txt")).unwrap(),
        b"portable before enrollment"
    );
    service.shutdown().unwrap();

    let restarted = Service::start_with_inventory(config, state.clone(), inventory).unwrap();
    let saved = status(&restarted);
    assert_eq!(saved.backup_directory.as_ref(), Some(&backups));
    assert_eq!(saved.pairs.len(), 1);
    assert_eq!(saved.pairs[0].id, pair.id);
    assert_eq!(saved.pairs[0].drive, pair.drive);
    assert_eq!(saved.pairs[0].evidence, pair.evidence);
    fs::write(
        source.join("after-restart.txt"),
        b"same filesystem identity",
    )
    .unwrap();
    finish(&restarted, DriveRequest::Sync { pair: pair.id });
    assert_eq!(
        fs::read(local.join("after-restart.txt")).unwrap(),
        b"same filesystem identity"
    );
    finish(&restarted, request);
    let versions = status(&restarted).backups;
    assert_eq!(versions.len(), 2);
    for version in versions {
        assert_eq!(version.drive, drive.id);
        assert_eq!(version.path.parent(), Some(bucket.as_path()));
        let mut bytes = u64::MAX;
        backup::read_manifest(
            &version,
            backup::completed_manifest(&version).unwrap(),
            &AtomicBool::new(false),
            &mut bytes,
            Duration::MAX,
            |db| {
                let captured: Drive = db.get("source", "drive")?.unwrap();
                assert!(captured.part_uuid.is_none());
                assert_eq!(captured.filesystem_uuid, drive.filesystem_uuid);
                Ok(())
            },
        )
        .unwrap();
    }
    assert_eq!(
        fs::read(first.path.join("portable.txt")).unwrap(),
        b"portable before enrollment"
    );
    restarted.shutdown().unwrap();
    names.extend(["local-only.txt".into(), "after-restart.txt".into()]);
    assert_eq!(
        fs::read_dir(source)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>(),
        names,
        "Operational identity, manifests and baselines must stay off the source media"
    );
}
