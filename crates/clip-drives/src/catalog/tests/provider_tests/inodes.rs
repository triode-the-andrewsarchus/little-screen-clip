use super::*;
use std::os::unix::fs::{DirEntryExt, MetadataExt};

#[test]
fn reserved_and_out_of_range_inode_numbers_cannot_alias_sqlite_rows_or_panic() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    for ino in [0, 1, 2, 1 << 63, (1 << 63) + 1, u64::MAX] {
        assert!(view.row(ino).unwrap().is_none(), "inode {ino}");
    }
}

#[test]
fn directory_inode_lookup_does_not_invent_a_parent_for_missing_or_nondirectory_rows() {
    let fixture = fixture();
    fs::create_dir_all(fixture.source.path.join("a/b")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    assert_eq!(view.directory_inode(Path::new("")).unwrap(), 1);
    let (ino, _) = view.lookup_path(Path::new("a")).unwrap().unwrap();
    assert_eq!(view.directory_inode(Path::new("a")).unwrap(), ino);
    for path in ["missing", "file.txt", "excluded"] {
        assert!(
            matches!(view.directory_inode(Path::new(path)), Err(crate::Error::Io(error)) if error.raw_os_error() == Some(libc::EIO))
        );
    }
    view.db
        .0
        .execute(
            "DELETE FROM entries WHERE scan='catalog' AND path=?1",
            [b"a".as_slice()],
        )
        .unwrap();
    assert!(
        matches!(view.directory_inode(Path::new("a")), Err(crate::Error::Io(error)) if error.raw_os_error() == Some(libc::EIO))
    );
}

#[test]
fn catalog_rowids_must_fit_nonreserved_inodes_and_signed_directory_cookies() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    for rowid in [i64::MIN, -2, -1, 0, i64::MAX - 1, i64::MAX] {
        view.db
            .0
            .execute(
                "UPDATE entries SET rowid=?1 WHERE scan='catalog' AND path=?2",
                params![rowid, b"file.txt".as_slice()],
            )
            .unwrap();
        assert!(
            matches!(view.lookup_path(Path::new("file.txt")), Err(crate::Error::Io(error)) if error.raw_os_error() == Some(libc::EOVERFLOW)),
            "rowid {rowid}"
        );
    }
    view.db
        .0
        .execute(
            "UPDATE entries SET rowid=?1 WHERE scan='catalog' AND path=?2",
            params![i64::MAX - 2, b"file.txt".as_slice()],
        )
        .unwrap();
    let (ino, entry) = view.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    assert_eq!(ino, i64::MAX as u64);
    assert_eq!(view.row(ino).unwrap().unwrap().path, entry.path);
    let page = view.directory_page(1, i64::MAX - 1).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].0, ino);
    assert!(view.directory_page(1, i64::MAX).unwrap().is_empty());
    assert!(
        matches!(view.directory_page(1, -1), Err(crate::Error::Io(error)) if error.raw_os_error() == Some(libc::EINVAL))
    );
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_dot_entries_report_the_actual_directory_and_parent_inodes() {
    let fixture = fixture();
    fs::create_dir_all(fixture.source.path.join("a/b/c")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let server = CatalogProcess::start(&fixture, &catalog);
    for (relative, parent) in [("", ""), ("a", ""), ("a/b", "a"), ("a/b/c", "a/b")] {
        let path = server.mount.join(relative);
        let file = File::open(&path).unwrap();
        let entries: Vec<_> = rustix::fs::Dir::read_from(&file)
            .unwrap()
            .map(|row| {
                let row = row.unwrap();
                (row.file_name().to_bytes().to_vec(), row.ino())
            })
            .collect();
        let dot = entries.iter().find(|row| row.0 == b".").unwrap().1;
        let dotdot = entries.iter().find(|row| row.0 == b"..").unwrap().1;
        assert_eq!(dot, fs::metadata(&path).unwrap().ino(), "dot in {relative}");
        assert_eq!(
            dotdot,
            fs::metadata(server.mount.join(parent)).unwrap().ino(),
            "parent of {relative}"
        );
    }
    drop(server);
}

#[test]
#[ignore = "requires /dev/fuse, fusermount3 and LITTLE_CLIP_DRIVE_MOUNT_HELPER"]
fn real_fuse_signed_cookie_boundary_resumes_and_bad_rowids_do_not_poison_the_server() {
    for rowid in [i64::MAX - 2, i64::MAX] {
        let fixture = fixture();
        let catalog = fixture.capture(&DriveConfig::default()).unwrap();
        let view = filesystem(&fixture.state.path, catalog.clone());
        view.db
            .0
            .execute(
                "UPDATE entries SET rowid=?1 WHERE scan='catalog' AND path=?2",
                params![rowid, b"file.txt".as_slice()],
            )
            .unwrap();
        drop(view);
        let server = CatalogProcess::start(&fixture, &catalog);
        let metadata = fs::metadata(server.mount.join("file.txt"));
        let entries =
            fs::read_dir(&server.mount).and_then(|rows| rows.collect::<std::io::Result<Vec<_>>>());
        if rowid == i64::MAX - 2 {
            assert_eq!(metadata.unwrap().ino(), i64::MAX as u64);
            let entries = entries.unwrap();
            assert_eq!(entries.len(), 2);
            let file = entries
                .iter()
                .find(|row| row.file_name() == "file.txt")
                .unwrap();
            assert_eq!(file.ino(), i64::MAX as u64);
        } else {
            assert_eq!(metadata.unwrap_err().raw_os_error(), Some(libc::EOVERFLOW));
            assert_eq!(entries.unwrap_err().raw_os_error(), Some(libc::EOVERFLOW));
            assert!(server.mount.join("excluded").is_dir());
        }
        drop(server);
    }
}
