use super::*;
use std::fs::{self, FileTimes};
use std::time::SystemTime;
mod metadata;
mod types;

struct CatalogProcess {
    child: std::process::Child,
    mount: PathBuf,
}
impl CatalogProcess {
    fn start(fixture: &Fixture, catalog: &Catalog) -> Self {
        use little_clip_core::process::CommandRequest;
        let helper = std::env::var_os("LITTLE_CLIP_DRIVE_MOUNT_HELPER")
            .expect("Build and explicitly select the current clip-drive-mount helper");
        let mount = fixture
            .temp
            .path()
            .join(format!("provider-view-{}", catalog.id));
        fs::create_dir(&mount).unwrap();
        let child = CommandRequest::new(helper)
            .arg(&fixture.state.path)
            .arg(&catalog.id)
            .arg(&mount)
            .die_with_parent()
            .build_command()
            .spawn()
            .unwrap();
        let mut owned = Self { child, mount };
        let started = std::time::Instant::now();
        while crate::mounts::catalog_mount(&owned.mount, &catalog.id)
            .unwrap()
            .is_none()
        {
            assert!(owned.child.try_wait().unwrap().is_none());
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        owned
    }
}
impl Drop for CatalogProcess {
    fn drop(&mut self) {
        let _ = little_clip_core::process::CommandRequest::new("fusermount3")
            .arg("-u")
            .arg(&self.mount)
            .timeout(Duration::from_secs(5))
            .run();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_revision(path: &Path, bytes: &[u8], modified: SystemTime) {
    fs::write(path, bytes).unwrap();
    File::open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
}

fn fingerprinted_entry(fixture: &Fixture) -> Entry {
    scan::observe(
        &fixture.source,
        Path::new("file.txt"),
        true,
        u64::MAX,
        &AtomicBool::new(false),
    )
    .unwrap()
    .unwrap()
}

#[test]
fn provider_validation_hashes_the_retained_file_instead_of_a_replacement_name() {
    let fixture = fixture();
    let relative = Path::new("file.txt");
    let entry = fingerprinted_entry(&fixture);
    let path = fixture.source.path.join(relative);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    write_revision(&path, b"changed payload", modified);
    let retained = fixture.source.input(relative).unwrap();
    fs::rename(&path, path.with_extension("retained")).unwrap();
    write_revision(&path, b"catalog payload", modified);
    assert_eq!(
        fixture
            .source
            .digest(relative, u64::MAX, &AtomicBool::new(false))
            .unwrap(),
        *entry.hash.as_ref().unwrap(),
        "a second path lookup would incorrectly accept this provider"
    );
    let error = OpenFile::validated(Root::open(&fixture.source.path).unwrap(), retained, &entry)
        .err()
        .unwrap();
    assert!(error.to_string().contains("contents are stale"), "{error}");
}

#[test]
fn matching_digest_does_not_override_changed_provider_timestamps() {
    let fixture = fixture();
    let relative = Path::new("file.txt");
    let entry = fingerprinted_entry(&fixture);
    let file = fixture.source.input(relative).unwrap();
    file.set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
        .unwrap();
    let error = OpenFile::validated(Root::open(&fixture.source.path).unwrap(), file, &entry)
        .err()
        .unwrap();
    assert!(error.to_string().contains("revision is stale"), "{error}");
}

#[test]
fn stale_backup_hash_falls_back_to_an_older_matching_provider() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    let (_, mut entry) = view.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    let older = fixture.backup().unwrap();
    let newer = fixture.backup().unwrap();
    let path = newer.path.join("file.txt");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    entry.hash = Some(
        fixture
            .source
            .digest(&entry.path, entry.size, &AtomicBool::new(false))
            .unwrap(),
    );
    write_revision(&path, b"changed payload", modified);
    let open = view.resolve(&entry).unwrap();
    assert_eq!(open.root.path, older.path);
    let mut bytes = [0; 15];
    open.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"catalog payload");
    fs::remove_dir_all(&older.path).unwrap();
    let error = view.resolve(&entry).err().unwrap();
    assert!(error.to_string().contains("stale"), "{error}");
}

#[test]
fn replaced_backup_directory_cannot_supply_metadata_identical_contents() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    let (_, entry) = view.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    assert!(entry.hash.is_none());
    let backup = fixture.backup().unwrap();
    let original = backup.path.join("file.txt");
    let modified = fs::metadata(&original).unwrap().modified().unwrap();
    fs::rename(&backup.path, backup.path.with_extension("retained")).unwrap();
    fs::create_dir(&backup.path).unwrap();
    write_revision(&original, b"changed payload", modified);
    let error = view.resolve(&entry).err().unwrap();
    assert!(error.to_string().contains("stale"), "{error}");
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_reads_retain_one_provider_and_reject_replaced_payloads() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let first = fixture.backup().unwrap();
    // Keep the client separate from the server, as in the actual application.
    // Fork/exec in an in-process FUSE server can flush its own inherited client FD.
    let server = CatalogProcess::start(&fixture, &catalog);
    let held = File::open(server.mount.join("file.txt")).unwrap();
    let mut bytes = [0; 7];
    held.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"catalog");
    let path = first.path.join("file.txt");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    write_revision(&path, b"changed payload", modified);
    fixture.backup().unwrap();
    assert_eq!(
        fs::read(server.mount.join("file.txt")).unwrap(),
        b"catalog payload"
    );
    let error = held.read_at(&mut bytes, 8).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::ESTALE));
    drop(held);
    drop(server);
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_partial_listings_disclose_details_and_refuse_successful_empty_or_absent_results() {
    use little_clip_core::catalog_metadata::{
        CATALOG_DETAILS_XATTR, CatalogEntryStatus, CatalogMount,
    };
    let fixture = fixture();
    let catalog = coverage_tests::limited_subtree(&fixture);
    let server = CatalogProcess::start(&fixture, &catalog);
    let limited = server.mount.join("limited");
    let mount = CatalogMount::detect(&server.mount).unwrap();
    assert_eq!(mount.status(&limited), Some(CatalogEntryStatus::Incomplete));
    let error = little_clip_core::file_inventory::read_directory(&limited, &AtomicBool::new(false))
        .unwrap_err();
    assert!(error.contains("Partial catalog"), "{error}");
    let mut bytes = [0; 2048];
    let len = rustix::fs::getxattr(&limited, CATALOG_DETAILS_XATTR, &mut bytes[..]).unwrap();
    let details: serde_json::Value = serde_json::from_slice(&bytes[..len]).unwrap();
    assert_eq!(details["directory"]["issue"]["reason"], "Depth");
    let error = fs::read_dir(&limited)
        .and_then(|rows| rows.collect::<std::io::Result<Vec<_>>>())
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EIO));
    assert_eq!(
        fs::metadata(limited.join("unseen"))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EIO)
    );
    assert_eq!(
        fs::metadata(server.mount.join("absent"))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOENT)
    );
    drop(server);

    let catalog = fixture
        .capture(&DriveConfig {
            catalog_max_entries: 1,
            ..Default::default()
        })
        .unwrap();
    let server = CatalogProcess::start(&fixture, &catalog);
    assert_eq!(
        fs::metadata(server.mount.join("unobserved-name"))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EIO)
    );
    let error = fs::read_dir(&server.mount)
        .and_then(|rows| rows.collect::<std::io::Result<Vec<_>>>())
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(libc::EIO));
    drop(server);
}

#[test]
#[ignore = "requires an explicitly retained version-1 LITTLE_CLIP_DRIVE_LEGACY_MOUNT_HELPER"]
fn older_helpers_refuse_typed_catalogs_before_mounting_incomplete_listings() {
    let helper = std::env::var_os("LITTLE_CLIP_DRIVE_LEGACY_MOUNT_HELPER")
        .expect("Select a retained version-1 catalog helper");
    let fixture = fixture();
    let catalog = coverage_tests::limited_subtree(&fixture);
    let mount = fixture.temp.path().join("legacy-refusal");
    fs::create_dir(&mount).unwrap();
    let output = little_clip_core::process::CommandRequest::new(helper)
        .arg(&fixture.state.path)
        .arg(&catalog.id)
        .arg(&mount)
        .timeout(Duration::from_secs(5))
        .run()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Unsupported drive database version"));
    assert!(
        crate::mounts::catalog_mount(&mount, &catalog.id)
            .unwrap()
            .is_none()
    );
}
