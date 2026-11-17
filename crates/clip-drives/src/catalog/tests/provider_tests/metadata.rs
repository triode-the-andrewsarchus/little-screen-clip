use super::*;
use std::os::unix::fs::MetadataExt;

fn raw_errno(error: crate::Error) -> Option<i32> {
    match error {
        crate::Error::Io(error) => error.raw_os_error(),
        error => panic!("Expected an I/O error, got {error}"),
    }
}

#[test]
fn invalid_catalog_timestamps_report_overflow_instead_of_substituting_dates() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let mut fs = filesystem(&fixture.state.path, catalog);
    let (ino, mut entry) = fs.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    for nanos in [-1, 1_000_000_000, i64::MAX] {
        entry.nanos = nanos;
        assert_eq!(
            raw_errno(fs.attr(ino, Some(&entry)).unwrap_err()),
            Some(libc::EOVERFLOW)
        );
    }
    entry.modified = -2;
    entry.nanos = 250_000_000;
    assert_eq!(
        fs.attr(ino, Some(&entry)).unwrap().mtime,
        SystemTime::UNIX_EPOCH - Duration::from_millis(1750)
    );
    fs.catalog.redacted = Some(u64::MAX);
    assert_eq!(
        raw_errno(fs.attr(ino, Some(&entry)).unwrap_err()),
        Some(libc::EOVERFLOW)
    );
    fs.catalog.redacted = None;
    fs.catalog.captured = u64::MAX;
    assert_eq!(
        raw_errno(fs.attr(1, None).unwrap_err()),
        Some(libc::EOVERFLOW)
    );
    fs.catalog.captured = i64::MAX as u64;
    fs.catalog.redaction_generation = 1_000_000_000;
    assert_eq!(
        raw_errno(fs.attr(1, None).unwrap_err()),
        Some(libc::EOVERFLOW)
    );
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_invalid_saved_date_returns_overflow_without_hiding_other_entries() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog.clone());
    let (_, mut entry) = view.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    entry.nanos = 1_000_000_000;
    view.db
        .0
        .execute(
            "UPDATE entries SET json=?1 WHERE scan='catalog' AND path=?2",
            params![serde_json::to_string(&entry).unwrap(), b"file.txt".as_slice()],
        )
        .unwrap();
    drop(view);
    let server = CatalogProcess::start(&fixture, &catalog);
    assert_eq!(
        fs::metadata(server.mount.join("file.txt"))
            .unwrap_err()
            .raw_os_error(),
        Some(libc::EOVERFLOW)
    );
    assert!(fs::metadata(&server.mount).unwrap().is_dir());
    assert!(
        fs::read_dir(&server.mount)
            .unwrap()
            .any(|row| row.unwrap().file_name() == "excluded")
    );
    drop(server);
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_preserves_signed_fractional_modification_times() {
    let fixture = fixture();
    let times = [
        (-2, 250_000_000),
        (-1, 0),
        (-1, 1),
        (-1, 999_999_999),
        (0, 1),
        (2_000_000_000, 123_456_789),
    ];
    for (index, (seconds, nanos)) in times.iter().enumerate() {
        let path = fixture.source.path.join(format!("date-{index}"));
        fs::write(&path, b"metadata date").unwrap();
        File::open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(backup::time(*seconds, *nanos).unwrap()))
            .unwrap();
        let observed = fs::metadata(path).unwrap();
        assert_eq!(
            (observed.mtime(), observed.mtime_nsec()),
            (*seconds, *nanos)
        );
    }
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let server = CatalogProcess::start(&fixture, &catalog);
    for (index, expected) in times.iter().enumerate() {
        let observed = fs::metadata(server.mount.join(format!("date-{index}"))).unwrap();
        assert_eq!(
            (observed.mtime(), observed.mtime_nsec()),
            *expected,
            "catalog date-{index}"
        );
    }
    drop(server);
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_large_reads_and_metadata_views_reject_mutations() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = fixture();
    fs::create_dir(fixture.source.path.join("empty")).unwrap();
    let bytes: Vec<_> = (0..3 * 1024 * 1024 + 17).map(|n| (n % 251) as u8).collect();
    fs::write(fixture.source.path.join("large.bin"), &bytes).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let backup = fixture.backup().unwrap();
    let server = CatalogProcess::start(&fixture, &catalog);
    let path = server.mount.join("large.bin");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let held = File::open(&path).unwrap();
    let mut slice = vec![0; 2 * 1024 * 1024 + 19];
    held.read_exact_at(&mut slice, 777).unwrap();
    assert_eq!(slice, bytes[777..777 + slice.len()]);
    let absent = server.mount.join("new-name");
    let attempts = [
        ("write", fs::write(&path, b"changed")),
        (
            "append",
            fs::OpenOptions::new().append(true).open(&path).map(drop),
        ),
        ("create", File::create(&absent).map(drop)),
        (
            "chmod",
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)),
        ),
        (
            "utimens",
            held.set_times(FileTimes::new().set_modified(SystemTime::UNIX_EPOCH)),
        ),
        ("mkdir", fs::create_dir(&absent)),
        ("unlink", fs::remove_file(&path)),
        ("rmdir", fs::remove_dir(server.mount.join("empty"))),
        ("rename", fs::rename(&path, &absent)),
        ("symlink", symlink("large.bin", &absent)),
        ("hardlink", fs::hard_link(&path, &absent)),
        (
            "setxattr",
            rustix::fs::setxattr(
                &path,
                "user.test",
                b"value",
                rustix::fs::XattrFlags::empty(),
            )
            .map_err(Into::into),
        ),
        (
            "removexattr",
            rustix::fs::removexattr(&path, "user.test").map_err(Into::into),
        ),
    ];
    for (label, result) in attempts {
        assert_eq!(
            result.unwrap_err().raw_os_error(),
            Some(libc::EROFS),
            "{label}"
        );
    }
    // A read-only descriptor is rejected before the VFS mount-write check.
    assert_eq!(
        held.set_len(0).unwrap_err().raw_os_error(),
        Some(libc::EINVAL)
    );
    assert_eq!(
        held.write_at(b"changed", 0).unwrap_err().raw_os_error(),
        Some(libc::EBADF)
    );
    assert!(!absent.exists());
    assert!(server.mount.join("empty").is_dir());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(backup.path.join("large.bin")).unwrap(), bytes);
    assert_eq!(
        fs::read(fixture.source.path.join("large.bin")).unwrap(),
        bytes
    );
    drop(held);
    drop(server);
}
