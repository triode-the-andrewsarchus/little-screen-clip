use super::*;
use little_clip_core::catalog_metadata::{CatalogEntryStatus, CatalogScanReason, CatalogScanStage};
use std::{
    fs,
    os::unix::{
        ffi::OsStringExt,
        fs::{MetadataExt, PermissionsExt},
    },
};

fn details(view: &CatalogFs, ino: u64) -> serde_json::Value {
    serde_json::from_slice(&view.details(ino).unwrap().unwrap()).unwrap()
}
pub(super) fn limited_subtree(fixture: &Fixture) -> Catalog {
    fs::create_dir(fixture.source.path.join("limited")).unwrap();
    fs::write(fixture.source.path.join("limited/unseen"), b"not empty").unwrap();
    fixture
        .capture(&DriveConfig {
            catalog_max_depth: 1,
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn directory_completion_and_raw_timestamps_are_recorded_without_trusting_drive_labels() {
    let fixture = fixture();
    fixture.db.remove("rule", "exclude").unwrap();
    fs::create_dir(fixture.source.path.join("empty")).unwrap();
    let original = fs::metadata(fixture.source.path.join("file.txt")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    assert!(catalog.complete);
    let source = catalog.details.as_ref().unwrap();
    assert!(source.directory.listing_complete);
    assert!(source.directory.subtree_complete);
    assert_eq!(
        source.namespace,
        little_clip_core::kernel_interface::Namespace::current()
    );
    assert_eq!(
        source.source_mount,
        little_clip_core::kernel_interface::MountIdentity::pinned(
            &fixture.source.directory(Path::new(""), false).unwrap()
        )
        .unwrap()
    );
    // The fixture's Drive label claims vfat; the ordinary host source is not vfat.
    assert_ne!(source.filesystem.as_deref(), Some("vfat"));
    assert_eq!(source.mtime_resolution_ns, None);
    let view = filesystem(&fixture.state.path, catalog);
    let version: u32 = view
        .db
        .0
        .query_row("SELECT value FROM version WHERE id=1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, 2,
        "older version-1 helpers must reject new catalog semantics"
    );
    assert_eq!(view.coverage(1).unwrap(), None);
    let (ino, empty) = view.lookup_path(Path::new("empty")).unwrap().unwrap();
    assert!(empty.directory_coverage.unwrap().subtree_complete);
    assert!(view.listing_complete(ino).unwrap());
    let (_, entry) = view.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    assert_eq!(
        (entry.modified, entry.nanos),
        (original.mtime(), original.mtime_nsec())
    );
    assert!(entry.hash.is_none());
    let captured: crate::Drive = view.db.get("source", "drive").unwrap().unwrap();
    assert_eq!(captured, fixture.drive);
}

#[test]
fn depth_and_entry_limits_disclose_incomplete_folders_without_claiming_policy_exclusions() {
    let fixture = fixture();
    let catalog = limited_subtree(&fixture);
    assert!(!catalog.complete);
    assert!(catalog.details.as_ref().unwrap().directory.listing_complete);
    let view = filesystem(&fixture.state.path, catalog);
    let (ino, entry) = view.lookup_path(Path::new("limited")).unwrap().unwrap();
    assert_eq!(entry.kind, "directory");
    let coverage = entry.directory_coverage.unwrap();
    assert!(!coverage.listing_complete);
    assert_eq!(coverage.issue.unwrap().reason, CatalogScanReason::Depth);
    assert_eq!(
        view.coverage(ino).unwrap(),
        Some(CatalogEntryStatus::Incomplete)
    );
    assert!(!view.listing_complete(ino).unwrap());
    assert_eq!(details(&view, ino)["directory"]["issue"]["reason"], "Depth");
    let (excluded, _) = view.lookup_path(Path::new("excluded")).unwrap().unwrap();
    assert_eq!(
        view.coverage(excluded).unwrap(),
        Some(CatalogEntryStatus::NotCatalogued)
    );
    drop(view);
    let catalog = fixture
        .capture(&DriveConfig {
            catalog_max_entries: 1,
            ..Default::default()
        })
        .unwrap();
    let source = catalog.details.as_ref().unwrap();
    assert!(!source.directory.listing_complete);
    assert_eq!(source.stopped.unwrap().reason, CatalogScanReason::Entries);
    let view = filesystem(&fixture.state.path, catalog);
    assert!(!view.listing_complete(1).unwrap());
    assert_eq!(details(&view, 1)["directory"]["issue"]["reason"], "Entries");
}

#[test]
fn unreadable_directory_and_root_keep_typed_errno_without_fabricating_empty_coverage() {
    // Root bypasses DAC; the real permission fixture is exercised as an ordinary user.
    if rustix::process::geteuid().is_root() {
        return;
    }
    let fixture = fixture();
    let blocked = fixture.source.path.join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::write(blocked.join("private-child"), b"not catalogued").unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o0)).unwrap();
    let captured = fixture.capture(&DriveConfig::default());
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    let catalog = captured.unwrap();
    assert!(!catalog.complete);
    assert!(!catalog.coverage.contains("private-child"));
    let view = filesystem(&fixture.state.path, catalog);
    let (ino, entry) = view.lookup_path(Path::new("blocked")).unwrap().unwrap();
    let failure = entry.directory_coverage.unwrap().issue.unwrap();
    assert_eq!(failure.stage, CatalogScanStage::OpenDirectory);
    assert_eq!(failure.reason, CatalogScanReason::Io);
    assert_eq!(failure.errno, Some(libc::EACCES));
    assert!(!view.listing_complete(ino).unwrap());
    drop(view);
    fs::set_permissions(&fixture.source.path, fs::Permissions::from_mode(0o0)).unwrap();
    let captured = fixture.capture(&DriveConfig::default());
    fs::set_permissions(&fixture.source.path, fs::Permissions::from_mode(0o700)).unwrap();
    let catalog = captured.unwrap();
    assert_eq!(catalog.entries, 0);
    assert!(!catalog.complete);
    let coverage = &catalog.details.as_ref().unwrap().directory;
    assert!(!coverage.listing_complete);
    assert_eq!(coverage.issue.unwrap().errno, Some(libc::EACCES));
}

#[test]
fn metadata_failures_round_trip_raw_paths_and_legacy_records_remain_unknown() {
    let fixture = fixture();
    let relative = PathBuf::from(OsString::from_vec(b"missing-\xff".to_vec()));
    let error = std::io::Error::from_raw_os_error(libc::ENOENT).into();
    let entry = scan::unavailable(relative.clone(), &error, true);
    scan::put(&fixture.db, "typed", &entry).unwrap();
    let entry = scan::entry(&fixture.db, "typed", &relative)
        .unwrap()
        .unwrap();
    assert_eq!(entry.path, relative);
    assert_eq!(entry.failure.unwrap().errno, Some(libc::ENOENT));
    let mut legacy = serde_json::to_value(entry).unwrap();
    legacy.as_object_mut().unwrap().remove("failure");
    legacy.as_object_mut().unwrap().remove("directory_coverage");
    let legacy: Entry = serde_json::from_value(legacy).unwrap();
    assert!(legacy.failure.is_none());
    assert!(legacy.directory_coverage.is_none());
    let mut catalog = fixture.capture(&DriveConfig::default()).unwrap();
    catalog.details = None;
    let view = filesystem(&fixture.state.path, catalog);
    assert_eq!(view.details(1).unwrap(), None);
}

#[test]
fn directory_pages_preserve_raw_names_stable_inodes_and_offsets_across_multiple_batches() {
    let fixture = fixture();
    for index in 0..1025 {
        File::create(fixture.source.path.join(format!("page-{index:04}"))).unwrap();
    }
    let raw = OsString::from_vec(b"page-\xff".to_vec());
    File::create(fixture.source.path.join(&raw)).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    let mut offset = 2;
    let plans = view.db.0.prepare("EXPLAIN QUERY PLAN SELECT rowid,name,json FROM entries WHERE scan='catalog' AND parent=?1 AND rowid>?2 ORDER BY rowid LIMIT 512").unwrap()
        .query_map(params![b"".as_slice(), 0_i64], |row| row.get::<_, String>(3)).unwrap()
        .collect::<rusqlite::Result<Vec<_>>>().unwrap();
    assert!(
        plans.iter().any(|plan| plan.contains("rowid>")),
        "{plans:?}"
    );
    assert!(
        !plans.iter().any(|plan| plan.contains("TEMP B-TREE")),
        "{plans:?}"
    );
    let mut names = std::collections::HashSet::new();
    let mut inodes = std::collections::HashSet::new();
    loop {
        let page = view.directory_page(1, offset).unwrap();
        assert!(page.len() <= 512);
        if page.is_empty() {
            break;
        }
        assert_eq!(
            view.directory_page(1, offset)
                .unwrap()
                .iter()
                .map(|row| row.0)
                .collect::<Vec<_>>(),
            page.iter().map(|row| row.0).collect::<Vec<_>>()
        );
        for (ino, name, entry) in page {
            assert!(ino > offset as u64);
            assert!(names.insert(name.clone()));
            assert!(inodes.insert(ino));
            assert_eq!(view.lookup_path(&entry.path).unwrap().unwrap().0, ino);
            offset = ino as i64;
        }
    }
    assert_eq!(names.len(), 1028);
    assert!(names.contains(&raw));
    assert_eq!(
        view.payload_opens
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn replacing_an_open_directory_cannot_publish_complete_coverage_for_its_replacement() {
    let fixture = fixture();
    let nested = fixture.source.path.join("replace-me");
    fs::create_dir(&nested).unwrap();
    for index in 0..256 {
        File::create(nested.join(format!("file-{index:04}"))).unwrap();
    }
    let mut replaced = false;
    let catalog = capture(
        &fixture.db,
        &fixture.state,
        &fixture.drive,
        &fixture.source,
        &DriveConfig::default(),
        &AtomicBool::new(false),
        |count, _| {
            if !replaced && count >= 128 {
                fs::rename(&nested, fixture.temp.path().join("retained-original")).unwrap();
                fs::create_dir(&nested).unwrap();
                replaced = true;
            }
        },
    )
    .unwrap();
    assert!(replaced);
    assert!(!catalog.complete);
    let view = filesystem(&fixture.state.path, catalog);
    let (_, entry) = view.lookup_path(Path::new("replace-me")).unwrap().unwrap();
    let coverage = entry.directory_coverage.unwrap();
    assert!(!coverage.listing_complete);
    assert_eq!(coverage.issue.unwrap().reason, CatalogScanReason::Changed);
}
