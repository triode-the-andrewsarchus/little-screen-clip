//! Real guest UDisks/PolicyKit acceptance; no host bus or block device is exposed.
use super::*;
use crate::sync_edge_tests::native_fs;
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) fn selected(fixture: &Drive) -> Drive {
    assert_eq!(
        std::env::var("LITTLE_CLIP_DRIVE_TEST_AUTHORITY").unwrap(),
        "1"
    );
    assert!(
        fixture.part_uuid.is_some(),
        "Use the partitioned guest fixture"
    );
    let actual = discovery::select(discovery::discover().unwrap(), &fixture.id).unwrap();
    assert_eq!(actual.device, fixture.device);
    assert_eq!(actual.capacity, fixture.capacity);
    assert_eq!(actual.disk_sequence, fixture.disk_sequence);
    assert_eq!(actual.filesystem_uuid, fixture.filesystem_uuid);
    actual
}

pub(super) fn digest(mut file: File) -> String {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            return format!("{:x}", hash.finalize());
        }
        hash.update(&buffer[..count]);
    }
}
pub(super) fn assert_busy_refusal(service: &Service, request: DriveRequest) {
    let refused = completed(service, request);
    assert_eq!(refused.state, "failed", "{}", refused.message);
    assert!(
        refused.message.to_lowercase().contains("busy"),
        "{}",
        refused.message
    );
    assert!(status(service).backups.is_empty());
}

fn replaced_node_is_refused(drive: &Drive) {
    use std::os::unix::fs::MetadataExt;
    let nodes = tempfile::tempdir_in("/dev").unwrap();
    let alias = nodes.path().join("partition");
    fs::hard_link(&drive.device, &alias).unwrap();
    let mut aliased = drive.clone();
    aliased.device = alias.clone();
    let source = crate::device::Source::capture(&aliased).unwrap();
    let number = fs::metadata(&alias).unwrap().rdev();
    fs::rename(&alias, nodes.path().join("held-original")).unwrap();
    let created = little_clip_core::process::CommandRequest::new("mknod")
        .arg(&alias)
        .arg("b")
        .arg(libc::major(number).to_string())
        .arg(libc::minor(number).to_string())
        .run()
        .unwrap();
    assert!(created.status.success());
    assert_eq!(fs::metadata(&alias).unwrap().rdev(), number);
    let error = source.validate().unwrap_err();
    assert!(error.to_string().contains("removed or replaced"), "{error}");
}

#[test]
#[ignore = "requires the isolated partition/authority guest"]
fn native_image_authorization_is_denied_to_unprivileged_fixture() {
    if std::env::var_os("LITTLE_CLIP_DRIVE_TEST_DENIAL").is_none() {
        return;
    }
    assert_eq!(rustix::process::geteuid().as_raw(), 65534);
    let drive = selected(&native_fs::guest_drive());
    let source = crate::device::Source::capture(&drive).unwrap();
    let error = backup::open_image(&drive, &source).unwrap_err();
    assert!(
        error.to_string().contains("UDisks authorization"),
        "{error}"
    );
    assert!(error.to_string().contains("NotAuthorized"), "{error}");
    println!("\nGUEST_IMAGE_AUTHORIZATION_DENIED");
}

#[test]
#[ignore = "requires the isolated partition/authority guest"]
fn native_image_backup_checks_authority_busy_source_and_read_only_mount() {
    let original = selected(&native_fs::guest_drive());
    let layout = original.partition_layout.as_ref().unwrap();
    assert_eq!(layout.number, Some(1));
    assert_eq!(layout.start_512_sectors, Some(2048));
    assert_eq!(layout.offset_bytes, Some(1_048_576));
    assert_eq!(layout.logical_sector_bytes, Some(512));
    assert_eq!(layout.table_type.as_deref(), Some("gpt"));
    assert!(uuid::Uuid::parse_str(layout.table_uuid.as_deref().unwrap()).is_ok());
    assert_eq!(
        uuid::Uuid::parse_str(layout.partition_type.as_deref().unwrap())
            .unwrap()
            .to_string(),
        "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7"
    );
    replaced_node_is_refused(&original);
    let denied = little_clip_core::process::CommandRequest::new("env")
        .args(["LITTLE_CLIP_DRIVE_TEST_DENIAL=1", "setpriv", "--reuid=65534", "--regid=65534", "--clear-groups"])
        .arg(std::env::current_exe().unwrap())
        .args(["service::tests::native_images::native_image_authorization_is_denied_to_unprivileged_fixture", "--exact", "--ignored", "--nocapture"])
        .timeout(Duration::from_secs(15))
        .own_process_group()
        .run()
        .unwrap();
    assert!(
        denied.status.success(),
        "{}{}",
        String::from_utf8_lossy(&denied.stdout),
        String::from_utf8_lossy(&denied.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&denied.stdout)
            .lines()
            .filter(|line| *line == "GUEST_IMAGE_AUTHORIZATION_DENIED")
            .count(),
        1,
        "The unprivileged child must execute its authorization check"
    );
    let temp = tempfile::tempdir().unwrap();
    let source_path = original.mount.as_ref().unwrap().join("image-source.txt");
    fs::write(&source_path, b"first authorized partition backup").unwrap();
    let seconds_path = original.mount.as_ref().unwrap().join("seconds.txt");
    fs::write(&seconds_path, b"whole-second payload").unwrap();
    for (path, nanos) in [(&source_path, 120_000_000), (&seconds_path, 0)] {
        File::open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::new(1_600_000_000, nanos)),
            )
            .unwrap();
    }
    let service = Service::start_with_inventory(
        DriveConfig {
            backup_directory: Some(temp.path().join("backups")),
            ..Default::default()
        },
        temp.path().join("state"),
        Arc::new(discovery::discover),
    )
    .unwrap();
    finish(
        &service,
        DriveRequest::Catalog {
            drive: original.id.clone(),
        },
    );
    let catalog = status(&service).catalogs.pop().unwrap();
    let request = || DriveRequest::Backup {
        drive: original.id.clone(),
        kind: BackupKind::Image,
        restore_mounted: false,
    };
    let busy = File::open(&source_path).unwrap();
    assert_busy_refusal(&service, request());
    assert!(selected(&original).mount.is_some());
    drop(busy);
    finish(&service, request());
    let drive = selected(&original);
    assert!(drive.mount.is_none());
    let first = status(&service).backups.pop().unwrap();
    assert_eq!(
        first.path.parent().unwrap().file_name().unwrap(),
        original.part_uuid.as_ref().unwrap().as_str()
    );
    assert_eq!(fs::metadata(&first.path).unwrap().len(), original.capacity);
    let source = crate::device::Source::capture(&drive).unwrap();
    let input = backup::open_image(&drive, &source).unwrap();
    source.validate_input(&input).unwrap();
    let mut stale = drive.clone();
    stale.disk_sequence = Some(drive.disk_sequence.unwrap() + 1);
    let stale = crate::device::Source::capture(&stale).unwrap();
    let error = stale.validate_input(&input).unwrap_err();
    assert!(
        error.to_string().contains("Disk lifetime changed"),
        "{error}"
    );
    let expected = digest(input);
    assert_eq!(first.sha256.as_deref(), Some(expected.as_str()));
    assert_eq!(digest(File::open(&first.path).unwrap()), expected);
    let manifest_path = first.manifest.as_ref().unwrap();
    assert_eq!(
        digest(File::open(manifest_path).unwrap()),
        first.manifest_sha256.as_ref().unwrap().as_str()
    );
    let manifest = Store::existing(manifest_path, true).unwrap();
    let captured: Drive = manifest.get("source", "drive").unwrap().unwrap();
    assert_eq!(captured, drive);
    assert_eq!(captured.partition_layout, original.partition_layout);
    let current: DriveBackup = serde_json::from_slice(
        &fs::read(first.path.parent().unwrap().join("current-image.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(current.id, first.id);
    finish(
        &service,
        DriveRequest::MountCatalog {
            catalog: catalog.id.clone(),
        },
    );
    let catalog_mount = status(&service)
        .catalogs
        .into_iter()
        .find(|c| c.id == catalog.id)
        .unwrap()
        .mount
        .unwrap();
    let catalog_file = catalog_mount.join("image-source.txt");
    assert_eq!(fs::metadata(&catalog_file).unwrap().len(), 33);
    assert_eq!(
        fs::read(&catalog_file).unwrap_err().raw_os_error(),
        Some(libc::ENOMEDIUM)
    );
    let image_state = Store::existing(&temp.path().join("state/drives.sqlite"), false).unwrap();
    image_state.0.execute_batch(
        "CREATE TRIGGER image_mount_publication_fault BEFORE INSERT ON entities
         WHEN NEW.kind='image-mount' BEGIN SELECT RAISE(FAIL,'image mount publication fault'); END;",
    ).unwrap();
    let failed_mount = completed(
        &service,
        DriveRequest::MountImage {
            backup: first.id.clone(),
        },
    );
    image_state
        .0
        .execute_batch("DROP TRIGGER image_mount_publication_fault")
        .unwrap();
    assert_eq!(failed_mount.state, "failed");
    assert!(
        failed_mount
            .message
            .contains("image mount publication fault"),
        "{}",
        failed_mount.message
    );
    let retired: DriveBackup = image_state.get("backup", &first.id).unwrap().unwrap();
    assert!(retired.mount.is_none() && retired.loop_object.is_none());
    assert!(
        backup::ImageMount::load(&image_state, &retired)
            .unwrap()
            .is_none()
    );
    let loops = little_clip_core::process::CommandRequest::new("losetup")
        .arg("-j")
        .arg(&first.path)
        .run()
        .unwrap();
    assert!(loops.status.success());
    assert!(loops.stdout.is_empty(), "{}", loops.stdout_lossy());
    finish(
        &service,
        DriveRequest::MountImage {
            backup: first.id.clone(),
        },
    );
    let mounted_backup = status(&service)
        .backups
        .into_iter()
        .find(|b| b.id == first.id)
        .unwrap();
    let mounted = mounted_backup.mount.as_ref().unwrap();
    let observed = backup::ImageMount::load(&image_state, &mounted_backup)
        .unwrap()
        .unwrap();
    observed.validate(&Root::open(mounted).unwrap()).unwrap();
    assert_eq!(
        fs::read(mounted.join("image-source.txt")).unwrap(),
        b"first authorized partition backup"
    );
    let catalog_db = Store::existing(
        &temp
            .path()
            .join("state/catalogs")
            .join(format!("{}.sqlite", catalog.id)),
        true,
    )
    .unwrap();
    let entry = crate::scan::entry(&catalog_db, "catalog", Path::new("image-source.txt"))
        .unwrap()
        .unwrap();
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(mounted.join("image-source.txt")).unwrap();
    if (entry.modified, entry.nanos) == (metadata.mtime(), metadata.mtime_nsec()) {
        assert_eq!(
            fs::read(&catalog_file).unwrap(),
            b"first authorized partition backup"
        );
    } else {
        // Metadata-only catalogs cannot prove equivalence after a provider
        // loses timestamp precision. Never silently accept an uncertain file.
        assert_eq!(
            fs::read(&catalog_file).unwrap_err().raw_os_error(),
            Some(libc::ESTALE)
        );
        eprintln!(
            "Catalog provider lost timestamp precision: {}.{} versus {}.{}",
            entry.modified,
            entry.nanos,
            metadata.mtime(),
            metadata.mtime_nsec()
        );
    }
    assert_eq!(
        fs::read(catalog_mount.join("seconds.txt")).unwrap(),
        b"whole-second payload"
    );
    let seconds = catalog_mount.join("seconds.txt");
    let held = File::open(&seconds).unwrap();
    let replacement = native_fs::GuestMount::attach(mounted, mounted);
    assert!(observed.validate(&Root::open(mounted).unwrap()).is_err());
    assert_eq!(
        fs::read(&seconds).unwrap_err().raw_os_error(),
        Some(libc::ESTALE)
    );
    use std::os::unix::fs::FileExt;
    assert_eq!(
        held.read_at(&mut [0; 1], 0).unwrap_err().raw_os_error(),
        Some(libc::ESTALE)
    );
    drop(held);
    drop(replacement);
    assert_eq!(fs::read(&seconds).unwrap(), b"whole-second payload");
    image_state.remove("image-mount", &first.id).unwrap();
    assert_eq!(
        fs::read(&seconds).unwrap_err().raw_os_error(),
        Some(libc::ENOMEDIUM)
    );
    for field in ["boot", "namespace", "unique_mount"] {
        let mut changed = serde_json::to_value(&observed).unwrap();
        match field {
            "boot" => changed[field] = serde_json::to_value(uuid::Uuid::new_v4()).unwrap(),
            "namespace" => changed[field]["mount"] = serde_json::Value::Null,
            "unique_mount" => {
                changed[field] = serde_json::json!(changed[field].as_u64().unwrap() + 1)
            }
            _ => unreachable!(),
        }
        image_state.put("image-mount", &first.id, &changed).unwrap();
        assert_eq!(
            fs::read(&seconds).unwrap_err().raw_os_error(),
            Some(libc::ESTALE),
            "{field}"
        );
    }
    image_state
        .put("image-mount", &first.id, &observed)
        .unwrap();
    assert_eq!(fs::read(&seconds).unwrap(), b"whole-second payload");
    assert_eq!(
        fs::write(mounted.join("must-not-write"), b"x")
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EROFS)
    );
    finish(
        &service,
        DriveRequest::UnmountImage {
            backup: first.id.clone(),
        },
    );
    assert!(
        image_state
            .get::<serde_json::Value>("image-mount", &first.id)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fs::read(&catalog_file).unwrap_err().raw_os_error(),
        Some(libc::ENOMEDIUM)
    );
    finish(
        &service,
        DriveRequest::UnmountCatalog {
            catalog: catalog.id,
        },
    );
    let authority = partition::Authority::default();
    (authority.mount)(&original).unwrap();
    let mounted_source = selected(&original).mount.unwrap();
    fs::write(mounted_source.join("image-source.txt"), b"second version").unwrap();
    finish(
        &service,
        DriveRequest::Backup {
            drive: original.id.clone(),
            kind: BackupKind::Image,
            restore_mounted: true,
        },
    );
    assert!(selected(&original).mount.is_some());
    let versions = status(&service).backups;
    assert_eq!(versions.len(), 2);
    let second = versions.iter().find(|b| b.id != first.id).unwrap();
    assert_ne!(second.sha256, first.sha256);
    assert_eq!(digest(File::open(&first.path).unwrap()), expected);
    assert_eq!(
        fs::read(selected(&original).mount.unwrap().join("image-source.txt")).unwrap(),
        b"second version"
    );
    service.shutdown().unwrap();
}
