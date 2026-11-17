use super::*;
use little_clip_core::catalog_metadata::{
    CATALOG_ENTRY_XATTR, CatalogEntryStatus, CatalogSpecialKind,
};
use std::os::unix::{
    ffi::OsStringExt,
    fs::{MetadataExt, symlink},
    net::UnixListener,
};

fn saved_entry_at(path: &Path) -> Entry {
    // Inspect the saved node itself, never a symlink's referent.
    let mut bytes = [0; 8192];
    let count = rustix::fs::lgetxattr(path, CATALOG_ENTRY_XATTR, &mut bytes[..]).unwrap();
    serde_json::from_slice(&bytes[..count]).unwrap()
}

fn assert_inert(path: &Path) {
    assert_eq!(
        fs::symlink_metadata(path).unwrap().mode() & libc::S_IFMT,
        libc::S_IFREG
    );
    let error = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .unwrap_err();
    assert_eq!(error, rustix::io::Errno::OPNOTSUPP);
}

#[test]
fn saved_entry_metadata_is_lossless_bounded_and_available_for_legacy_catalogs() {
    let fixture = fixture();
    let raw = OsString::from_vec(b"raw-\xff".to_vec());
    fs::write(fixture.source.path.join(&raw), b"content").unwrap();
    symlink(&raw, fixture.source.path.join("link")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let mut view = filesystem(&fixture.state.path, catalog);
    view.catalog.details = None;
    let (ino, mut entry) = view.lookup_path(Path::new("link")).unwrap().unwrap();
    let bytes = view.saved_entry(ino).unwrap().unwrap();
    let saved: Entry = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(saved.link, Some(PathBuf::from(raw)));
    assert!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["link"].is_array());
    entry.link = Some(OsString::from_vec(vec![0xff; 20000]).into());
    scan::put(&view.db, "catalog", &entry).unwrap();
    assert!(
        matches!(view.saved_entry(ino), Err(crate::Error::Io(error)) if error.raw_os_error() == Some(libc::E2BIG))
    );
}

fn special_catalog(fixture: &Fixture) -> (UnixListener, Catalog) {
    fixture.db.remove("rule", "exclude").unwrap();
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        fixture.source.path.join("pipe.ts"),
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let socket = UnixListener::bind(fixture.source.path.join("socket")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    assert!(catalog.complete, "{}", catalog.coverage);
    (socket, catalog)
}

#[test]
fn special_file_capture_keeps_original_metadata_without_claiming_missing_payloads() {
    let fixture = fixture();
    let (_socket, catalog) = special_catalog(&fixture);
    let view = filesystem(&fixture.state.path, catalog);
    assert!(
        view.catalog
            .details
            .as_ref()
            .unwrap()
            .directory
            .subtree_complete
    );
    for (name, kind) in [
        ("pipe.ts", CatalogSpecialKind::Fifo),
        ("socket", CatalogSpecialKind::Socket),
    ] {
        let metadata = fs::symlink_metadata(fixture.source.path.join(name)).unwrap();
        let (ino, entry) = view.lookup_path(Path::new(name)).unwrap().unwrap();
        assert_eq!(entry.kind, "special");
        assert_eq!(entry.mode, metadata.mode());
        assert_eq!(
            (entry.modified, entry.nanos),
            (metadata.mtime(), metadata.mtime_nsec())
        );
        assert!(entry.hash.is_none());
        assert!(entry.warning.is_none());
        assert_eq!(
            view.coverage(ino).unwrap(),
            Some(CatalogEntryStatus::MetadataOnly(kind))
        );
        let saved: Entry =
            serde_json::from_slice(&view.saved_entry(ino).unwrap().unwrap()).unwrap();
        assert_eq!(saved.mode, entry.mode);
        assert_eq!(saved.size, metadata.len());
        assert_eq!(saved.uid, Some(metadata.uid()));
        assert_eq!(saved.gid, Some(metadata.gid()));
        assert_eq!(
            view.attr(ino, Some(&entry)).unwrap().kind,
            FileType::RegularFile
        );
    }
    // Payload collectors retain their existing unsupported-special warning.
    let coverage = scan::scan(
        &fixture.db,
        &fixture.source,
        "ordinary",
        &Limits {
            hashes: false,
            ..Default::default()
        },
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    assert!(!coverage.complete);
    assert!(
        scan::entry(&fixture.db, "ordinary", Path::new("pipe.ts"))
            .unwrap()
            .unwrap()
            .warning
            .is_some()
    );
}

#[test]
fn device_metadata_retains_rdev_without_opening_the_device_and_old_records_stay_unknown() {
    let original = fs::symlink_metadata("/dev/null").unwrap();
    let entry = Entry::from_metadata(Path::new("saved-device"), &original);
    assert_eq!(entry.special_device, Some(original.rdev()));
    let mut old = serde_json::to_value(&entry).unwrap();
    old.as_object_mut().unwrap().remove("special_device");
    assert!(
        serde_json::from_value::<Entry>(old)
            .unwrap()
            .special_device
            .is_none()
    );
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_special_files_are_inert_and_saved_types_remain_visible() {
    let fixture = fixture();
    let (_socket, catalog) = special_catalog(&fixture);
    let server = CatalogProcess::start(&fixture, &catalog);
    let rows =
        little_clip_core::file_inventory::read_directory(&server.mount, &AtomicBool::new(false))
            .unwrap();
    for (name, kind) in [
        ("pipe.ts", CatalogSpecialKind::Fifo),
        ("socket", CatalogSpecialKind::Socket),
    ] {
        let path = server.mount.join(name);
        let row = rows.entries.iter().find(|row| row.path == path).unwrap();
        assert_eq!(
            row.catalog_status,
            Some(CatalogEntryStatus::MetadataOnly(kind))
        );
        assert!(!row.regular_file);
        assert!(!row.typescript);
        assert_inert(&path);
        let entry = saved_entry_at(&path);
        assert_eq!(CatalogSpecialKind::from_mode(entry.mode), kind);
        assert_eq!(entry.path, Path::new(name));
    }
    drop(server);
}

#[test]
#[ignore = "requires the disposable guest; creates inert owned device-node fixtures"]
fn guest_device_nodes_retain_exact_metadata_without_exposing_device_io() {
    crate::require_disposable_guest();
    let fixture = fixture();
    for (name, kind, mode) in [
        (
            "char-device",
            rustix::fs::FileType::CharacterDevice,
            libc::S_IFCHR,
        ),
        (
            "block-device",
            rustix::fs::FileType::BlockDevice,
            libc::S_IFBLK,
        ),
    ] {
        let dev = rustix::fs::makedev(240, 240);
        rustix::fs::mknodat(
            rustix::fs::CWD,
            fixture.source.path.join(name),
            kind,
            rustix::fs::Mode::RUSR,
            dev,
        )
        .unwrap();
        let original = fs::symlink_metadata(fixture.source.path.join(name)).unwrap();
        assert_eq!(original.mode() & libc::S_IFMT, mode);
        let catalog = fixture.capture(&DriveConfig::default()).unwrap();
        let server = CatalogProcess::start(&fixture, &catalog);
        let path = server.mount.join(name);
        let entry = saved_entry_at(&path);
        assert_eq!(entry.special_device, Some(original.rdev()));
        assert_eq!(entry.mode, original.mode());
        assert_inert(&path);
        assert_eq!(
            rustix::fs::mknodat(
                rustix::fs::CWD,
                server.mount.join("new-device"),
                kind,
                rustix::fs::Mode::RUSR,
                dev
            )
            .unwrap_err(),
            rustix::io::Errno::ROFS
        );
        drop(server);
    }
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_symlinks_preserve_local_targets_and_expose_blocked_targets_as_metadata() {
    let fixture = fixture();
    fixture.backup().unwrap();
    fs::create_dir(fixture.source.path.join("nested")).unwrap();
    for (name, target) in [
        ("file-link", "file.txt"),
        ("root-link", "."),
        ("nested/up", ".."),
        ("cycle", "cycle"),
        ("broken", "missing"),
        ("escape", "../outside"),
        ("absolute", "/etc/passwd"),
    ] {
        symlink(target, fixture.source.path.join(name)).unwrap();
    }
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let server = CatalogProcess::start(&fixture, &catalog);
    assert_eq!(
        fs::read(server.mount.join("file-link")).unwrap(),
        b"catalog payload"
    );
    for (name, target) in [
        ("file-link", "file.txt"),
        ("root-link", "."),
        ("nested/up", ".."),
        ("cycle", "cycle"),
    ] {
        assert_eq!(
            fs::read_link(server.mount.join(name)).unwrap(),
            Path::new(target)
        );
    }
    assert!(server.mount.join("root-link").is_dir());
    assert!(server.mount.join("nested/up").is_dir());
    assert_eq!(
        fs::metadata(server.mount.join("cycle"))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ELOOP)
    );
    for (name, target) in [
        ("broken", "missing"),
        ("escape", "../outside"),
        ("absolute", "/etc/passwd"),
    ] {
        let path = server.mount.join(name);
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&path).unwrap_err().raw_os_error(),
            Some(libc::ENODATA)
        );
        let entry = saved_entry_at(&path);
        assert_eq!(entry.link.as_deref(), Some(Path::new(target)));
    }
    drop(server);
}
