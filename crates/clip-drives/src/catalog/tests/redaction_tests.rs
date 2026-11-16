use super::*;
use crate::catalog::redaction::{self, Intent};
use std::{os::unix::ffi::OsStringExt, sync::Arc};

fn exclude(fixture: &Fixture, relative: &Path) -> Intent {
    redaction::set_rule(
        &fixture.db,
        &DriveRule {
            id: format!(
                "test:{}",
                little_clip_core::path_address::display_path(relative)
            ),
            drive: fixture.drive.id.clone(),
            relative: relative.into(),
            catalog: false,
            index: false,
        },
    )
    .unwrap();
    fixture
        .db
        .list::<Intent>("catalog-redaction", 0, 1)
        .unwrap()
        .pop()
        .unwrap()
}
fn apply(fixture: &Fixture, intent: &Intent) -> Result<bool> {
    redaction::apply(
        Arc::new(Root::open(&fixture.state.path)?),
        intent,
        &AtomicBool::new(false),
    )
}
fn saved(fixture: &Fixture, id: &str) -> Store {
    Store::existing(
        &fixture
            .state
            .path
            .join("catalogs")
            .join(format!("{id}.sqlite")),
        true,
    )
    .unwrap()
}

#[test]
fn reader_leases_defer_raw_name_redaction_without_omitting_backup_payloads() {
    let fixture = fixture();
    let folder = PathBuf::from(OsString::from_vec(b"private_\xff".to_vec()));
    let sibling = PathBuf::from(OsString::from_vec(b"private_\xff_more".to_vec()));
    for relative in [&folder, &sibling] {
        std::fs::create_dir(fixture.source.path.join(relative)).unwrap();
        std::fs::write(
            fixture
                .source
                .path
                .join(relative)
                .join("unique-secret-filename"),
            b"retained source payload",
        )
        .unwrap();
    }
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let backup = fixture.backup().unwrap();
    let reader = filesystem(&fixture.state.path, catalog.clone());
    let intent = exclude(&fixture, &folder);
    assert!(!apply(&fixture, &intent).unwrap());
    assert!(
        reader
            .lookup_path(&folder.join("unique-secret-filename"))
            .unwrap()
            .is_some()
    );
    let pending: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(pending.pending_redactions, 1);
    assert_eq!(pending.redacted, None);
    drop(reader);
    assert!(apply(&fixture, &intent).unwrap());
    let db = saved(&fixture, &catalog.id);
    assert!(
        scan::entry(&db, "catalog", &folder.join("unique-secret-filename"))
            .unwrap()
            .is_none()
    );
    assert!(
        scan::entry(&db, "catalog", &sibling.join("unique-secret-filename"))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        scan::entry(&db, "catalog", &folder).unwrap().unwrap().kind,
        "excluded"
    );
    let done: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(done.captured, catalog.captured);
    assert!(done.redacted.is_some());
    assert_eq!(done.pending_redactions, 0);
    assert!(
        fixture
            .db
            .list::<Intent>("catalog-redaction", 0, 1)
            .unwrap()
            .is_empty()
    );
    for root in [&fixture.source.path, &backup.path] {
        assert_eq!(
            std::fs::read(root.join(&folder).join("unique-secret-filename")).unwrap(),
            b"retained source payload"
        );
    }
    // This only checks the removed subtree's unique descendant, not its retained sibling.
    assert!(
        !std::fs::read(
            fixture
                .state
                .path
                .join("catalogs")
                .join(format!("{}.sqlite", catalog.id))
        )
        .unwrap()
        .windows(b"private_\xff/unique-secret-filename".len())
        .any(|bytes| bytes == b"private_\xff/unique-secret-filename")
    );
}

#[test]
fn root_redaction_is_not_an_empty_snapshot_or_a_new_capture() {
    let fixture = fixture();
    let catalog = fixture
        .capture(&DriveConfig {
            catalog_max_entries: 1,
            ..DriveConfig::default()
        })
        .unwrap();
    assert!(!catalog.complete);
    let reader = filesystem(&fixture.state.path, catalog.clone());
    let before = reader.attr(1, None);
    drop(reader);
    let intent = exclude(&fixture, Path::new(""));
    assert!(apply(&fixture, &intent).unwrap());
    let updated: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(updated.entries, 0);
    assert_eq!(updated.captured, catalog.captured);
    assert!(updated.coverage.contains(&catalog.coverage));
    assert!(updated.root_excluded);
    let fs = filesystem(&fixture.state.path, updated);
    assert_eq!(fs.attr(1, None).mtime, before.mtime);
    assert!(fs.attr(1, None).ctime > before.ctime);
    assert!(fs.parent_path(1).is_err());
    assert_eq!(
        fs.coverage(1).unwrap(),
        Some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued)
    );
    assert_eq!(
        std::fs::read(fixture.source.path.join("file.txt")).unwrap(),
        b"catalog payload"
    );
}

#[test]
fn interrupted_registration_retries_without_rewriting_the_redaction_date_or_losing_new_intents() {
    let fixture = fixture();
    std::fs::create_dir(fixture.source.path.join("private")).unwrap();
    std::fs::write(fixture.source.path.join("private/secret"), b"source").unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let first = exclude(&fixture, Path::new("private"));
    fixture.db.0.execute_batch("CREATE TRIGGER reject_completion BEFORE DELETE ON entities WHEN OLD.kind='catalog-redaction' BEGIN SELECT RAISE(ABORT,'injected registry completion failure'); END;").unwrap();
    assert!(
        apply(&fixture, &first)
            .unwrap_err()
            .to_string()
            .contains("injected registry")
    );
    let db = saved(&fixture, &catalog.id);
    assert!(
        scan::entry(&db, "catalog", Path::new("private/secret"))
            .unwrap()
            .is_none()
    );
    let edited: Catalog = db.get("catalog", &catalog.id).unwrap().unwrap();
    drop(db);
    let second = exclude(&fixture, Path::new("file.txt"));
    assert_ne!(first.id, second.id);
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER reject_completion")
        .unwrap();
    assert!(apply(&fixture, &first).unwrap());
    let intermediate: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(intermediate.redacted, edited.redacted);
    assert_eq!(intermediate.pending_redactions, 1);
    assert!(apply(&fixture, &second).unwrap());
    let done: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(done.pending_redactions, 0);
    assert_eq!(done.captured, catalog.captured);
}

#[test]
fn publication_enrolls_mid_scan_exclusions_but_already_excluded_scans_need_no_cleanup() {
    let fixture = fixture();
    std::fs::create_dir(fixture.source.path.join("private")).unwrap();
    std::fs::write(fixture.source.path.join("private/secret"), b"source").unwrap();
    let mut changed = false;
    let catalog = capture(
        &fixture.db,
        &fixture.state,
        &fixture.drive,
        &fixture.source,
        &DriveConfig::default(),
        &AtomicBool::new(false),
        |_, _| {
            if !changed {
                redaction::set_rule(
                    &fixture.db,
                    &DriveRule {
                        id: "mid-scan".into(),
                        drive: fixture.drive.id.clone(),
                        relative: "private".into(),
                        catalog: false,
                        index: false,
                    },
                )
                .unwrap();
                changed = true;
            }
        },
    )
    .unwrap();
    assert!(changed);
    let pending: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(pending.pending_redactions, 1);
    let fresh = fixture.capture(&DriveConfig::default()).unwrap();
    let registered: Catalog = fixture.db.get("catalog", &fresh.id).unwrap().unwrap();
    assert_eq!(registered.pending_redactions, 0);
    assert_eq!(registered.redacted, None);
    assert!(
        scan::entry(
            &saved(&fixture, &fresh.id),
            "catalog",
            Path::new("private/secret")
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn replaced_catalog_paths_are_rejected_without_touching_the_replacement() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let intent = exclude(&fixture, Path::new("private"));
    let path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{}.sqlite", catalog.id));
    let victim = fixture.temp.path().join("ordinary-file");
    std::fs::write(&victim, b"untouched ordinary bytes").unwrap();
    std::fs::rename(&path, path.with_extension("retained")).unwrap();
    std::os::unix::fs::symlink(&victim, &path).unwrap();
    assert!(apply(&fixture, &intent).is_err());
    assert_eq!(std::fs::read(&victim).unwrap(), b"untouched ordinary bytes");
    assert_eq!(
        fixture
            .db
            .list::<Intent>("catalog-redaction", 0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
#[ignore = "requires /dev/fuse and fusermount3"]
fn mounted_legacy_helpers_without_file_leases_still_block_redaction() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let mount = fixture.temp.path().join("legacy-view");
    std::fs::create_dir(&mount).unwrap();
    let server = filesystem(&fixture.state.path, catalog.clone());
    server._lease.release_for_legacy_reader_test();
    let session = fuser::spawn_mount2(server, &mount, &mount_options(&catalog.id)).unwrap();
    assert!(mount.join("file.txt").is_file());
    let intent = exclude(&fixture, Path::new(""));
    assert!(!apply(&fixture, &intent).unwrap());
    assert!(mount.join("file.txt").is_file());
    drop(session);
    assert!(apply(&fixture, &intent).unwrap());
}

#[test]
fn older_exclusions_migrate_once_and_repeated_pending_rules_share_the_same_intent() {
    let fixture = fixture();
    fixture.db.remove("rule", "exclude").unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let reader = filesystem(&fixture.state.path, catalog.clone());
    let rule = DriveRule {
        id: "old-policy".into(),
        drive: fixture.drive.id.clone(),
        relative: "excluded".into(),
        catalog: false,
        index: false,
    };
    // This is the old implementation's policy write, before cleanup existed.
    fixture.db.put("rule", &rule.id, &rule).unwrap();
    redaction::migrate(&fixture.db).unwrap();
    redaction::migrate(&fixture.db).unwrap();
    redaction::set_rule(&fixture.db, &rule).unwrap();
    let pending = fixture
        .db
        .list::<Intent>("catalog-redaction", 0, 10)
        .unwrap();
    assert_eq!(pending.len(), 1);
    let header: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(header.pending_redactions, 1);
    assert!(!apply(&fixture, &pending[0]).unwrap());
    assert!(
        reader
            .lookup_path(Path::new("excluded/hidden"))
            .unwrap()
            .is_some()
    );
    drop(reader);
    assert!(apply(&fixture, &pending[0]).unwrap());
    assert!(
        scan::entry(
            &saved(&fixture, &catalog.id),
            "catalog",
            Path::new("excluded/hidden")
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn exclusions_queue_all_historical_catalogs_beyond_one_database_page() {
    let fixture = fixture();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let transaction = fixture.db.0.unchecked_transaction().unwrap();
    for _ in 0..1025 {
        let mut historical = catalog.clone();
        historical.id = uuid::Uuid::new_v4().to_string();
        fixture
            .db
            .put("catalog", &historical.id, &historical)
            .unwrap();
    }
    transaction.commit().unwrap();
    exclude(&fixture, Path::new("private"));
    let queued: u64 = fixture
        .db
        .0
        .query_row(
            "SELECT count(*) FROM entities WHERE kind='catalog-redaction'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let enrolled: u64 = fixture.db.0.query_row("SELECT sum(json_extract(json,'$.pending_redactions')) FROM entities WHERE kind='catalog'", [], |row| row.get(0)).unwrap();
    assert_eq!(queued, 1026);
    assert_eq!(enrolled, queued);
}

#[test]
fn redaction_compacts_large_metadata_trees_without_removing_source_files() {
    let fixture = fixture();
    let private = fixture.source.path.join("private");
    std::fs::create_dir(&private).unwrap();
    for index in 0..4096 {
        std::fs::write(private.join(format!("record-{index:04}")), b"").unwrap();
    }
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{}.sqlite", catalog.id));
    let before = std::fs::metadata(&path).unwrap().len();
    let intent = exclude(&fixture, Path::new("private"));
    assert!(apply(&fixture, &intent).unwrap());
    let after = std::fs::metadata(&path).unwrap().len();
    assert!(
        after < before / 4,
        "catalog compaction: {before} -> {after} bytes"
    );
    assert_eq!(std::fs::read_dir(&private).unwrap().count(), 4096);
    eprintln!("catalog compaction: {before} -> {after} bytes; all 4096 source files retained");
}
