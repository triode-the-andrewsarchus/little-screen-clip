use super::*;
use crate::catalog::redaction::{self, Intent};
use std::{
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
    sync::Arc,
};

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

fn aliased_file() -> (Fixture, Catalog, Intent, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture();
    let relative = PathBuf::from(OsString::from_vec(b"saved_\xff".to_vec()));
    let path = fixture.source.path.join(&relative);
    std::fs::write(&path, b"metadata-only alias payload").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
    std::fs::write(fixture.source.path.join("zz-later"), b"ordinary sibling").unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    std::fs::hard_link(&path, fixture.source.path.join("new-alias")).unwrap();
    let intent = exclude(&fixture, Path::new("new-alias"));
    (fixture, catalog, intent, relative)
}

fn assert_single_redaction_replay(fixture: &Fixture, catalog: &Catalog, intent: &Intent) {
    let before: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(before.pending_redactions, 0);
    assert_eq!(before.redaction_generation, 1);
    assert!(apply(fixture, intent).unwrap());
    let after: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
}

#[test]
fn a_later_alias_redacts_saved_raw_names_using_metadata_without_opening_payloads() {
    let (fixture, catalog, intent, relative) = aliased_file();
    assert!(apply(&fixture, &intent).unwrap());
    let database = saved(&fixture, &catalog.id);
    let entry = scan::entry(&database, "catalog", &relative)
        .unwrap()
        .unwrap();
    assert_eq!(entry.kind, "excluded");
    assert!(entry.hash.is_none());
    assert!(
        scan::entry(&database, "catalog", Path::new("zz-later"))
            .unwrap()
            .is_some()
    );
    let metadata = std::fs::metadata(fixture.source.path.join(&relative)).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0);
    assert_eq!(metadata.len(), b"metadata-only alias payload".len() as u64);
    assert_single_redaction_replay(&fixture, &catalog, &intent);
}

#[test]
fn an_unavailable_original_keeps_alias_cleanup_pending_then_recovers_without_media_writes() {
    let (fixture, catalog, intent, relative) = aliased_file();
    let offline = fixture.temp.path().join("offline-source");
    std::fs::rename(&fixture.source.path, &offline).unwrap();
    let error = apply(&fixture, &intent).unwrap_err();
    assert!(
        error.to_string().contains("cleanup remains pending"),
        "{error}"
    );
    redaction::failure(&fixture.db, &intent, error.to_string()).unwrap();
    let pending: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(pending.pending_redactions, 1);
    assert_eq!(pending.redaction_generation, 0);
    assert!(pending.redacted.is_none());
    assert!(pending.redaction_error.is_some());
    assert_eq!(
        scan::entry(&saved(&fixture, &catalog.id), "catalog", &relative)
            .unwrap()
            .unwrap()
            .kind,
        "file"
    );
    std::fs::rename(&offline, &fixture.source.path).unwrap();
    assert!(apply(&fixture, &intent).unwrap());
    let completed: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(completed.pending_redactions, 0);
    assert!(completed.redaction_error.is_none());
    assert_eq!(
        scan::entry(&saved(&fixture, &catalog.id), "catalog", &relative)
            .unwrap()
            .unwrap()
            .kind,
        "excluded"
    );
}

#[test]
fn an_explicit_root_exclusion_can_finish_uncertain_alias_cleanup_offline() {
    let (fixture, catalog, intent, relative) = aliased_file();
    let offline = fixture.temp.path().join("offline-source");
    std::fs::rename(&fixture.source.path, &offline).unwrap();
    assert!(apply(&fixture, &intent).is_err());
    exclude(&fixture, Path::new(""));
    let root = fixture
        .db
        .list::<Intent>("catalog-redaction", 0, 10)
        .unwrap()
        .into_iter()
        .find(|intent| intent.relative.as_os_str().is_empty())
        .unwrap();
    assert!(apply(&fixture, &root).unwrap());
    assert!(apply(&fixture, &intent).unwrap());
    let completed: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(completed.entries, 0);
    assert_eq!(completed.pending_redactions, 0);
    assert!(completed.root_excluded);
    assert_eq!(
        std::fs::metadata(offline.join(relative)).unwrap().mode() & 0o777,
        0
    );
}

#[test]
fn unresolved_saved_names_roll_back_earlier_alias_edits_before_retry() {
    let (fixture, catalog, intent, relative) = aliased_file();
    std::fs::remove_file(fixture.source.path.join("zz-later")).unwrap();
    let error = apply(&fixture, &intent).unwrap_err();
    assert!(
        error.to_string().contains("no longer resolvable"),
        "{error}"
    );
    let database = saved(&fixture, &catalog.id);
    assert_eq!(
        scan::entry(&database, "catalog", &relative)
            .unwrap()
            .unwrap()
            .kind,
        "file"
    );
    assert!(
        database
            .get::<u64>("redaction", &intent.id)
            .unwrap()
            .is_none()
    );
    drop(database);
    std::fs::write(fixture.source.path.join("zz-later"), b"replacement sibling").unwrap();
    assert!(apply(&fixture, &intent).unwrap());
    assert_eq!(
        scan::entry(&saved(&fixture, &catalog.id), "catalog", &relative)
            .unwrap()
            .unwrap()
            .kind,
        "excluded"
    );
}

#[test]
fn missing_or_stale_source_context_cannot_authorize_alias_cleanup() {
    for invalid in ["missing", "unique_mount", "boot", "inode"] {
        let (fixture, catalog, intent, relative) = aliased_file();
        let database = Store::existing(
            &fixture
                .state
                .path
                .join("catalogs")
                .join(format!("{}.sqlite", catalog.id)),
            false,
        )
        .unwrap();
        if invalid == "missing" {
            database.remove("source", "redaction-context").unwrap();
        } else {
            let mut value: serde_json::Value = database
                .get("source", "redaction-context")
                .unwrap()
                .unwrap();
            match invalid {
                "unique_mount" => value["lifetime"]["unique_mount"] = 0.into(),
                "boot" => value["lifetime"]["boot"] = uuid::Uuid::new_v4().to_string().into(),
                "inode" => value["lifetime"]["identity"]["inode"] = 0.into(),
                _ => unreachable!(),
            }
            database.put("source", "redaction-context", &value).unwrap();
        }
        drop(database);
        assert!(apply(&fixture, &intent).is_err(), "{invalid}");
        assert_eq!(
            scan::entry(&saved(&fixture, &catalog.id), "catalog", &relative)
                .unwrap()
                .unwrap()
                .kind,
            "file"
        );
        let pending: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
        assert_eq!(pending.pending_redactions, 1);
    }
}

#[test]
fn replacing_a_source_with_a_symlink_cannot_redirect_alias_cleanup() {
    let (fixture, catalog, intent, relative) = aliased_file();
    let original = fixture.temp.path().join("moved-source");
    std::fs::rename(&fixture.source.path, &original).unwrap();
    std::os::unix::fs::symlink(&original, &fixture.source.path).unwrap();
    assert!(apply(&fixture, &intent).is_err());
    assert_eq!(
        scan::entry(&saved(&fixture, &catalog.id), "catalog", &relative)
            .unwrap()
            .unwrap()
            .kind,
        "file"
    );
    std::fs::remove_file(&fixture.source.path).unwrap();
    std::fs::rename(&original, &fixture.source.path).unwrap();
    assert!(apply(&fixture, &intent).unwrap());
}

#[test]
fn alias_resolution_rejects_retargeting_and_observes_cancellation_after_a_match() {
    for retarget in [true, false] {
        let (fixture, catalog, intent, _) = aliased_file();
        let database = saved(&fixture, &catalog.id);
        let cancel = AtomicBool::new(false);
        let mut matched = 0;
        let result = redaction::boundaries::visit(&database, &intent.relative, &cancel, |_| {
            matched += 1;
            if retarget {
                let alias = fixture.source.path.join("new-alias");
                std::fs::remove_file(&alias).unwrap();
                std::fs::write(alias, b"different inode").unwrap();
            } else {
                cancel.store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(())
        });
        assert_eq!(matched, 1);
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if retarget {
                "changed during"
            } else {
                "Cancelled"
            }),
            "{error}"
        );
    }
}

#[test]
#[ignore = "requires the disposable native-filesystem guest; historical catalog name-equivalence gate"]
fn native_historical_redaction_uses_source_name_equivalence() {
    for (rule, spelling) in [
        ("Future", "future"),
        ("Résumé", "résumé"),
        ("Ångström", "ångström"),
        ("Future/Inner", "future/inner"),
    ] {
        let (_native, fixture) = native_fixture();
        let actual = fixture.source.path.join(spelling);
        std::fs::create_dir_all(&actual).unwrap();
        std::fs::write(actual.join("unique-secret"), b"retained payload").unwrap();
        let equivalent = fixture.source.metadata(Path::new(rule)).unwrap().is_some();
        let catalog = fixture.capture(&DriveConfig::default()).unwrap();
        if equivalent {
            // The pinned canonical-directory path must avoid walking unrelated
            // historical names or any contents below the excluded boundary.
            std::fs::remove_file(fixture.source.path.join("file.txt")).unwrap();
        }
        let intent = exclude(&fixture, Path::new(rule));
        let replaced = crate::sync_edge_tests::native_fs::GuestMount::attach(
            &fixture.source.path,
            &fixture.source.path,
        );
        assert!(
            apply(&fixture, &intent).is_err(),
            "A replacement bind cannot reuse the captured source lifetime"
        );
        drop(replaced);
        assert!(apply(&fixture, &intent).unwrap());
        let database = saved(&fixture, &catalog.id);
        let descendant = scan::entry(
            &database,
            "catalog",
            &Path::new(spelling).join("unique-secret"),
        )
        .unwrap();
        eprintln!("Historical catalog {rule:?} / {spelling:?}: {equivalent}");
        assert_eq!(descendant.is_none(), equivalent, "{rule}");
        let boundary = scan::entry(&database, "catalog", Path::new(spelling))
            .unwrap()
            .unwrap();
        assert_eq!(boundary.kind == "excluded", equivalent, "{rule}");
        assert_eq!(
            std::fs::read(actual.join("unique-secret")).unwrap(),
            b"retained payload"
        );
    }
}

#[test]
#[ignore = "requires the disposable native-filesystem guest; renamed historical boundary gate"]
fn native_historical_case_rename_resolves_the_saved_spelling_without_inode_reuse() {
    let (_native, fixture) = native_fixture();
    let source = &fixture.source.path;
    std::fs::create_dir(source.join("future")).unwrap();
    std::fs::write(
        source.join("future/secret"),
        b"retained after a case rename",
    )
    .unwrap();
    if fixture
        .source
        .metadata(Path::new("Future"))
        .unwrap()
        .is_none()
    {
        eprintln!("Native filesystem keeps Future/future distinct; no case-alias rename applies");
        return;
    }
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    std::fs::rename(source.join("future"), source.join("intermediate-name")).unwrap();
    std::fs::rename(source.join("intermediate-name"), source.join("FUTURE")).unwrap();
    let pin = fixture
        .source
        .directory(Path::new("Future"), false)
        .unwrap();
    assert_eq!(
        std::fs::read_link(crate::root::anchor(&pin)).unwrap(),
        source.join("FUTURE")
    );
    drop(pin);
    let intent = exclude(&fixture, Path::new("Future"));
    assert!(apply(&fixture, &intent).unwrap());
    crate::catalog::test_assertions::redacted_leaf(
        &fixture.state.path,
        &catalog.id,
        Path::new("future/secret"),
        source,
        b"retained after a case rename",
    );
}

#[test]
fn legacy_spelling_intents_redact_offline_and_keep_their_original_replay_identity() {
    let fixture = fixture();
    let folder = PathBuf::from(OsString::from_vec(b"private_\xff/nested".to_vec()));
    std::fs::create_dir_all(fixture.source.path.join(&folder)).unwrap();
    std::fs::write(
        fixture.source.path.join(&folder).join("secret"),
        b"retained payload",
    )
    .unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let spelling = PathBuf::from(OsString::from_vec(b"private_\xff//./nested/".to_vec()));
    let intent = exclude(&fixture, &spelling);
    assert_eq!(intent.relative.as_os_str(), spelling.as_os_str());
    let offline = fixture.temp.path().join("offline-source");
    std::fs::rename(&fixture.source.path, &offline).unwrap();
    assert!(apply(&fixture, &intent).unwrap());
    let database = saved(&fixture, &catalog.id);
    assert!(
        scan::entry(&database, "catalog", &folder.join("secret"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        scan::entry(&database, "catalog", &folder)
            .unwrap()
            .unwrap()
            .kind,
        "excluded"
    );
    assert!(
        database
            .get::<u64>("redaction", &intent.id)
            .unwrap()
            .is_some()
    );
    drop(database);
    assert_single_redaction_replay(&fixture, &catalog, &intent);
    assert_eq!(
        std::fs::read(offline.join(&folder).join("secret")).unwrap(),
        b"retained payload"
    );
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
    let before = reader.attr(1, None).unwrap();
    drop(reader);
    let intent = exclude(&fixture, Path::new(""));
    assert!(apply(&fixture, &intent).unwrap());
    let updated: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert_eq!(updated.entries, 0);
    assert_eq!(updated.captured, catalog.captured);
    assert!(updated.coverage.contains(&catalog.coverage));
    assert!(updated.root_excluded);
    let fs = filesystem(&fixture.state.path, updated);
    assert_eq!(fs.attr(1, None).unwrap().mtime, before.mtime);
    assert!(fs.attr(1, None).unwrap().ctime > before.ctime);
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
fn redaction_retires_descendant_diagnostics_and_updates_retained_ancestor_coverage() {
    let fixture = fixture();
    std::fs::create_dir_all(fixture.source.path.join("parent/private/hidden")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let before = filesystem(&fixture.state.path, catalog.clone());
    let (parent, _) = before.lookup_path(Path::new("parent")).unwrap().unwrap();
    assert!(
        before
            .row(parent)
            .unwrap()
            .unwrap()
            .directory_coverage
            .unwrap()
            .subtree_complete
    );
    drop(before);
    let intent = exclude(&fixture, Path::new("parent/private"));
    assert!(apply(&fixture, &intent).unwrap());
    let updated: Catalog = fixture.db.get("catalog", &catalog.id).unwrap().unwrap();
    assert!(!updated.details.as_ref().unwrap().directory.subtree_complete);
    let view = filesystem(&fixture.state.path, updated);
    let (parent, _) = view.lookup_path(Path::new("parent")).unwrap().unwrap();
    assert!(
        !view
            .row(parent)
            .unwrap()
            .unwrap()
            .directory_coverage
            .unwrap()
            .subtree_complete
    );
    let (_, boundary) = view
        .lookup_path(Path::new("parent/private"))
        .unwrap()
        .unwrap();
    assert!(boundary.directory_coverage.is_none());
    assert!(boundary.failure.is_none());
    assert!(
        view.lookup_path(Path::new("parent/private/hidden"))
            .unwrap()
            .is_none()
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
    let session = server::spawn(server, &mount, &catalog.id).unwrap();
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
fn catalog_capture_honors_old_subtree_and_root_rules_beyond_one_database_page() {
    for relative in [Path::new("excluded"), Path::new("")] {
        let fixture = fixture();
        let transaction = fixture.db.0.unchecked_transaction().unwrap();
        fixture
            .db
            .put(
                "rule",
                "exclude",
                &DriveRule {
                    id: "exclude".into(),
                    drive: fixture.drive.id.clone(),
                    relative: relative.into(),
                    catalog: false,
                    index: true,
                },
            )
            .unwrap();
        for index in 0..1025 {
            let rule = DriveRule {
                id: format!("other-{index}"),
                drive: "other-drive".into(),
                relative: PathBuf::new(),
                catalog: true,
                index: true,
            };
            fixture.db.put("rule", &rule.id, &rule).unwrap();
        }
        transaction.commit().unwrap();
        let result = fixture.capture(&DriveConfig::default());
        if relative.as_os_str().is_empty() {
            assert!(result.unwrap_err().to_string().contains("disabled"));
        } else {
            let catalog = result.unwrap();
            assert!(catalog.complete);
            assert_eq!(catalog.entries, 2);
            let saved = saved(&fixture, &catalog.id);
            assert!(
                scan::entry(&saved, "catalog", Path::new("excluded/hidden"))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                scan::entry(&saved, "catalog", relative)
                    .unwrap()
                    .unwrap()
                    .kind,
                "excluded"
            );
        }
    }
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
