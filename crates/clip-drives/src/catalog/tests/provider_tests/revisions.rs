//! Catalogs without a saved digest must still validate completed backup bytes.
use super::*;

#[test]
fn metadata_only_catalog_rejects_same_size_mtime_backup_tampering() {
    check_backup_tampering(&fixture(), Path::new("file.txt"));
}

#[test]
fn metadata_only_backup_validation_preserves_raw_file_names() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = fixture();
    let name = OsString::from_vec(b"raw-\xff.bin".to_vec());
    fs::rename(
        fixture.source.path.join("file.txt"),
        fixture.source.path.join(&name),
    )
    .unwrap();
    check_backup_tampering(&fixture, Path::new(&name));
}

fn check_backup_tampering(fixture: &Fixture, name: &Path) {
    let (view, entry) = fixture.captured_file(name);
    assert!(entry.hash.is_none());
    let version = fixture.backup().unwrap();
    assert_eq!(view.resolve(&entry).unwrap().root.path, version.path);
    let path = version.path.join(&entry.path);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    write_revision(&path, b"changed payload", modified);
    assert_eq!(fs::metadata(&path).unwrap().len(), entry.size);
    let error = view
        .resolve(&entry)
        .err()
        .expect("Altered backup must be stale");
    assert!(error.to_string().contains("stale"), "{error}");
    write_revision(&path, b"catalog payload", modified);
    let newer = fixture.backup().unwrap();
    write_revision(&newer.path.join(&entry.path), b"changed payload", modified);
    let open = view.resolve(&entry).unwrap();
    assert_eq!(
        open.root.path, version.path,
        "an altered newest backup must fall back"
    );
    let mut bytes = [0; 15];
    open.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"catalog payload");
    drop(open);
    fs::remove_dir_all(&version.path).unwrap();
    assert!(view.resolve(&entry).is_err());
    assert_eq!(
        fs::read(fixture.source.path.join(&entry.path)).unwrap(),
        b"catalog payload"
    );
}

#[test]
fn later_backup_cannot_substitute_a_different_source_revision_with_matching_mtime() {
    let fixture = fixture();
    let (view, entry) = fixture.captured_file(Path::new("file.txt"));
    assert!(entry.hash.is_none());
    let path = fixture.source.path.join(&entry.path);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    write_revision(&path, b"changed payload", modified);
    let changed = Entry::from_metadata(&entry.path, &fs::metadata(&path).unwrap());
    assert!(
        !entry.same_revision(&changed),
        "control must change the source revision"
    );
    fixture.backup().unwrap();
    let error = view
        .resolve(&entry)
        .err()
        .expect("Changed source revision must be stale");
    assert!(error.to_string().contains("stale"), "{error}");
}

#[test]
fn metadata_only_providers_require_a_verified_manifest_and_source_observation() {
    for fault in [
        "checksum",
        "missing",
        "location",
        "header",
        "root",
        "root-missing",
        "entry-missing",
        "entry-path",
        "entry-changed",
        "entry-inode",
        "entry-kind",
        "entry-large",
        "digest-missing",
        "digest-invalid",
        "catalog-legacy",
    ] {
        let fixture = fixture();
        let catalog = fixture.capture(&DriveConfig::default()).unwrap();
        let mut view = filesystem(&fixture.state.path, catalog);
        let entry = view.lookup_path(Path::new("file.txt")).unwrap().unwrap().1;
        let mut version = fixture.backup().unwrap();
        let id = version.id.clone();
        match fault {
            "checksum" => version.manifest_sha256 = Some("0".repeat(64)),
            "missing" => version.manifest = None,
            "location" => {
                let renamed = version.path.with_extension("wrong.sqlite");
                fs::rename(version.manifest.as_ref().unwrap(), &renamed).unwrap();
                version.manifest = Some(renamed);
            }
            "catalog-legacy" => view.catalog.details = None,
            _ => fixture.rewrite_manifest(&mut version, |manifest| {
                if fault == "header" {
                    manifest.remove("backup", &id).unwrap();
                } else if fault == "root-missing" {
                    manifest.remove("source", "root").unwrap();
                } else if fault == "root" {
                    let mut root: Entry = manifest.get("source", "root").unwrap().unwrap();
                    root.inode += 1;
                    manifest.put("source", "root", &root).unwrap();
                } else {
                    let scan = format!("backup:{id}");
                    let mut saved = scan::entry(manifest, &scan, &entry.path).unwrap().unwrap();
                    match fault {
                        "entry-missing" => {
                            manifest
                                .0
                                .execute(
                                    "DELETE FROM entries WHERE scan=?1 AND path=?2",
                                    params![scan, entry.path.as_os_str().as_bytes()],
                                )
                                .unwrap();
                            return;
                        }
                        "entry-path" => saved.path = "another-name".into(),
                        "entry-changed" => saved.changed = None,
                        "entry-inode" => saved.inode += 1,
                        "entry-kind" => saved.kind = "directory".into(),
                        "entry-large" => saved.warning = Some("x".repeat(70_000)),
                        "digest-missing" => saved.hash = None,
                        "digest-invalid" => saved.hash = Some("not-a-sha256".into()),
                        _ => unreachable!(),
                    }
                    // Keep the lookup key independent of a deliberately invalid row path.
                    manifest
                        .0
                        .execute(
                            "UPDATE entries SET json=?1 WHERE scan=?2 AND path=?3",
                            params![
                                serde_json::to_string(&saved).unwrap(),
                                scan,
                                entry.path.as_os_str().as_bytes()
                            ],
                        )
                        .unwrap();
                }
            }),
        }
        fixture.db.put("backup", &version.id, &version).unwrap();
        let error = view.resolve(&entry).err().expect(fault);
        assert!(error.to_string().contains("stale"), "{fault}: {error}");
        assert_eq!(view.lookup_path(&entry.path).unwrap().unwrap().1.size, 15);
        assert_eq!(
            fs::read(version.path.join(&entry.path)).unwrap(),
            b"catalog payload"
        );
    }
}

#[test]
fn a_saved_catalog_digest_can_authorize_a_provider_without_optional_manifest_enrichment() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let mut view = filesystem(&fixture.state.path, catalog);
    let mut entry = fingerprinted_entry(&fixture);
    assert!(entry.hash.is_some());
    let mut version = fixture.backup().unwrap();
    version.manifest = None;
    fixture.db.put("backup", &version.id, &version).unwrap();
    view.catalog.details = None;
    let open = view.resolve(&entry).unwrap();
    assert_eq!(open.root.path, version.path);
    drop(open);
    let path = version.path.join(&entry.path);
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    write_revision(&path, b"changed payload", modified);
    assert!(view.resolve(&entry).is_err());
    entry.hash = None;
    assert!(view.resolve(&entry).is_err());
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_metadata_only_catalog_rejects_altered_backup_and_manifest() {
    check_fuse_revision(&fixture());
}

fn check_fuse_revision(fixture: &Fixture) {
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog.clone());
    assert!(
        view.lookup_path(Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .1
            .hash
            .is_none()
    );
    drop(view);
    let version = fixture.backup().unwrap();
    let server = CatalogProcess::start(fixture, &catalog);
    let path = server.mount.join("file.txt");
    assert_eq!(fs::read(&path).unwrap(), b"catalog payload");
    let held = File::open(&path).unwrap();
    let output = version.path.join("file.txt");
    let modified = fs::metadata(&output).unwrap().modified().unwrap();
    write_revision(&output, b"changed payload", modified);
    assert_eq!(
        fs::read(&path).unwrap_err().raw_os_error(),
        Some(libc::ESTALE)
    );
    let mut bytes = [0; 15];
    assert_eq!(
        held.read_at(&mut bytes, 0).unwrap_err().raw_os_error(),
        Some(libc::ESTALE)
    );
    drop(held);
    write_revision(&output, b"catalog payload", modified);
    assert_eq!(fs::read(&path).unwrap(), b"catalog payload");
    Store::existing(version.manifest.as_ref().unwrap(), false)
        .unwrap()
        .put("unverified", "change", &true)
        .unwrap();
    assert_eq!(
        fs::read(&path).unwrap_err().raw_os_error(),
        Some(libc::ESTALE)
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), 15);
    assert_eq!(fs::read(output).unwrap(), b"catalog payload");
    drop(server);
}

#[test]
#[ignore = "requires the disposable FAT32/exFAT/NTFS guest and current FUSE helper"]
fn native_metadata_only_catalog_validates_file_backup_bytes() {
    let (_native, fixture) = native_fixture();
    check_backup_tampering(&fixture, Path::new("file.txt"));
    check_fuse_revision(&fixture);
}
