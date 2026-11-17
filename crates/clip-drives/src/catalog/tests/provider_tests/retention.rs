//! Retention must preserve providers independently of mounted-status publication.
use super::*;

#[test]
fn pruning_refuses_an_open_file_backup_until_its_provider_is_released() {
    let fixture = fixture();
    let (view, entry) = fixture.captured_file(Path::new("file.txt"));
    let first = fixture.backup().unwrap();
    let held = view.resolve(&entry).unwrap();
    assert_eq!(held.root.path, first.path);
    let current = fixture.backup().unwrap();
    let error = backup::prune(&fixture.db, &fixture.drive.id, 1, &AtomicBool::new(false))
        .expect_err("An open provider must prevent retirement");
    assert!(error.to_string().contains("busy"), "{error}");
    assert_eq!(
        fs::read(first.path.join(&entry.path)).unwrap(),
        b"catalog payload"
    );
    drop(held);
    backup::prune(&fixture.db, &fixture.drive.id, 1, &AtomicBool::new(false)).unwrap();
    assert!(!first.path.exists());
    assert!(current.path.exists());
}

#[test]
fn mounted_catalog_beyond_the_first_status_page_still_blocks_pruning() {
    let fixture = fixture();
    let mut catalog = fixture.capture(&DriveConfig::default()).unwrap();
    catalog.mount = Some(fixture.temp.path().join("retained-view"));
    fixture.db.put("catalog", &catalog.id, &catalog).unwrap();
    for _ in 0..1000 {
        let mut other = catalog.clone();
        other.id = uuid::Uuid::new_v4().to_string();
        other.drive = "another-drive".into();
        other.mount = None;
        fixture.db.put("catalog", &other.id, &other).unwrap();
    }
    let first = fixture.backup().unwrap();
    fixture.backup().unwrap();
    let error = backup::prune(&fixture.db, &fixture.drive.id, 1, &AtomicBool::new(false))
        .expect_err("Status pagination must not hide readers");
    assert!(error.to_string().contains("Unmount"), "{error}");
    assert!(first.path.exists());
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_provider_lease_survives_missing_catalog_status() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let first = fixture.backup().unwrap();
    let server = CatalogProcess::start(&fixture, &catalog);
    let held = File::open(server.mount.join("file.txt")).unwrap();
    // Model a missing status record while the independently owned server lives.
    fixture.db.remove("catalog", &catalog.id).unwrap();
    fixture.backup().unwrap();
    let error = backup::prune(&fixture.db, &fixture.drive.id, 1, &AtomicBool::new(false))
        .expect_err("A provider lease must work across processes");
    assert!(error.to_string().contains("busy"), "{error}");
    let mut bytes = [0; 15];
    held.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"catalog payload");
    drop(held);
    drop(server);
    backup::prune(&fixture.db, &fixture.drive.id, 1, &AtomicBool::new(false)).unwrap();
    assert!(!first.path.exists());
}
