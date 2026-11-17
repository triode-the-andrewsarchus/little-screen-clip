//! Historical manifests remain optional, bounded and independently verified.
use super::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;

#[test]
fn historical_fingerprints_preserve_raw_file_names() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = reliable_fixture();
    let name = OsString::from_vec(b"raw-\xff.bin".to_vec());
    fs::write(fixture.source.path.join(&name), b"raw payload").unwrap();
    fixture.backup().unwrap();
    let current = fixture.backup().unwrap();
    corrupt(&current);
    let entry = fixture.captured_entry(Path::new(&name));
    assert_eq!(entry.path, Path::new(&name));
    assert_eq!(entry.size, b"raw payload".len() as u64);
    assert_eq!(
        entry.hash,
        fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(|| format!("{:x}", Sha256::digest(b"raw payload")))
    );
    assert_eq!(
        fs::read(fixture.source.path.join(&name)).unwrap(),
        b"raw payload"
    );
}

fn expected() -> Option<String> {
    Some(format!("{:x}", Sha256::digest(b"catalog payload")))
}
fn corrupt(version: &Backup) {
    Store::existing(version.manifest.as_ref().unwrap(), false)
        .unwrap()
        .put("unverified", "change", &true)
        .unwrap();
}

#[test]
fn unusable_current_candidates_do_not_hide_a_verified_older_manifest() {
    for fault in [
        "checksum",
        "missing",
        "source-root",
        "tracking",
        "malformed-row",
        "header",
        "registry",
        "registry-id",
        "pointer",
        "large-registry",
        "evidence",
        "kind",
    ] {
        let fixture = reliable_fixture();
        fs::write(fixture.source.path.join("z-broken"), b"last payload").unwrap();
        let older = fixture.backup().unwrap();
        let mut current = fixture.backup().unwrap();
        let current_id = current.id.clone();
        let old_bytes = fs::read(older.manifest.as_ref().unwrap()).unwrap();
        match fault {
            "checksum" => corrupt(&current),
            "missing" => {
                let path = current.manifest.as_ref().unwrap();
                fs::rename(path, path.with_extension("retained")).unwrap();
            }
            "source-root" => fixture.rewrite_manifest(&mut current, |db| {
                let mut root: Entry = db.get("source", "root").unwrap().unwrap();
                root.inode += 1;
                db.put("source", "root", &root).unwrap();
            }),
            "tracking" => fixture.rewrite_manifest(&mut current, |db| {
                db.remove("collection", "source_queried_change_tracking")
                    .unwrap();
            }),
            "malformed-row" => fixture.rewrite_manifest(&mut current, |db| {
                db.0.execute(
                    "UPDATE entries SET json='broken json' WHERE path=?1",
                    [b"z-broken".as_slice()],
                )
                .unwrap();
            }),
            "header" => fixture.rewrite_manifest(&mut current, |db| {
                let mut header: Value = db.get("backup", &current_id).unwrap().unwrap();
                header["future-field"] = "x".repeat(70_000).into();
                db.put("backup", &current_id, &header).unwrap();
            }),
            "registry" => {
                fixture
                    .db
                    .0
                    .execute(
                        "UPDATE entities SET json='broken json' WHERE kind='backup' AND id=?1",
                        [&current.id],
                    )
                    .unwrap();
            }
            "registry-id" => {
                let mut mismatch = current.clone();
                mismatch.id = uuid::Uuid::new_v4().to_string();
                fixture.db.put("backup", &current.id, &mismatch).unwrap();
            }
            "pointer" => {
                corrupt(&current);
                fixture
                    .db
                    .remove(
                        "backup-current",
                        &format!("{}:{:?}", fixture.drive.id, crate::BackupKind::Files),
                    )
                    .unwrap();
            }
            "large-registry" => {
                let mut value = serde_json::to_value(&current).unwrap();
                value["future-field"] = "x".repeat(70_000).into();
                fixture.db.put("backup", &current.id, &value).unwrap();
            }
            "evidence" => {
                current.evidence = "another-drive".into();
                fixture.db.put("backup", &current.id, &current).unwrap();
            }
            "kind" => {
                current.kind = crate::BackupKind::Image;
                fixture.db.put("backup", &current.id, &current).unwrap();
            }
            _ => unreachable!(),
        }
        assert_eq!(
            fixture.captured_entry(Path::new("file.txt")).hash,
            fixture
                .source
                .queried_change_tracking()
                .unwrap()
                .then(expected)
                .flatten(),
            "{fault}"
        );
        assert_eq!(
            fs::read(older.manifest.as_ref().unwrap()).unwrap(),
            old_bytes,
            "{fault}"
        );
    }
}

#[test]
fn current_pointer_has_priority_over_newer_history_and_wall_clock_values() {
    let fixture = reliable_fixture();
    let mut chosen = fixture.backup().unwrap();
    let mut newer = fixture.backup().unwrap();
    let id = format!("backup:{}", newer.id);
    fixture.rewrite_manifest(&mut newer, |db| {
        let mut row = scan::entry(db, &id, Path::new("file.txt"))
            .unwrap()
            .unwrap();
        row.hash = Some("0".repeat(64));
        scan::put(db, &id, &row).unwrap();
    });
    chosen.created = 0;
    fixture.db.put("backup", &chosen.id, &chosen).unwrap();
    fixture
        .db
        .put(
            "backup-current",
            &format!("{}:{:?}", fixture.drive.id, crate::BackupKind::Files),
            &chosen.id,
        )
        .unwrap();
    assert_eq!(
        fixture.captured_entry(Path::new("file.txt")).hash,
        fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(expected)
            .flatten()
    );
}

#[test]
fn later_manifest_failure_cannot_undo_prior_enrichment_and_older_rows_can_fill_gaps() {
    let fixture = reliable_fixture();
    fs::write(fixture.source.path.join("z-broken"), b"last payload").unwrap();
    fixture.backup().unwrap();
    let mut middle = fixture.backup().unwrap();
    let mut current = fixture.backup().unwrap();
    fixture.rewrite_manifest(&mut middle, |db| {
        db.0.execute(
            "UPDATE entries SET json='broken json' WHERE path=?1",
            [b"z-broken".as_slice()],
        )
        .unwrap();
    });
    fixture.rewrite_manifest(&mut current, |db| {
        db.0.execute(
            "DELETE FROM entries WHERE path=?1",
            [b"z-broken".as_slice()],
        )
        .unwrap();
    });
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    assert!(catalog.complete);
    let view = filesystem(&fixture.state.path, catalog);
    for (path, bytes) in [
        ("file.txt", b"catalog payload".as_slice()),
        ("z-broken", b"last payload".as_slice()),
    ] {
        let hash = view.lookup_path(Path::new(path)).unwrap().unwrap().1.hash;
        let wanted = fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(|| format!("{:x}", Sha256::digest(bytes)));
        assert_eq!(hash, wanted, "{path}");
    }
}

#[test]
fn verification_allowance_is_shared_and_larger_capture_budgets_allow_older_reuse() {
    let fixture = reliable_fixture();
    let older = fixture.backup().unwrap();
    let current = fixture.backup().unwrap();
    corrupt(&current);
    let old_len = fs::metadata(older.manifest.as_ref().unwrap())
        .unwrap()
        .len();
    let new_len = fs::metadata(current.manifest.as_ref().unwrap())
        .unwrap()
        .len();
    let mut bytes = new_len + old_len;
    assert!(
        backup::read_manifest(
            &current,
            current.manifest.as_ref().unwrap(),
            &AtomicBool::new(false),
            &mut bytes,
            Duration::MAX,
            |_| Ok(())
        )
        .is_err()
    );
    assert_eq!(
        bytes, old_len,
        "failed checks still consume their reservation"
    );
    backup::read_manifest(
        &older,
        older.manifest.as_ref().unwrap(),
        &AtomicBool::new(false),
        &mut bytes,
        Duration::MAX,
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(bytes, 0);
    let mut config = DriveConfig {
        catalog_max_bytes: old_len.max(new_len).max(128 * 1024),
        ..Default::default()
    };
    assert!(config.catalog_max_bytes < old_len + new_len);
    let limited = fixture.capture(&config).unwrap();
    assert!(limited.complete);
    assert!(
        filesystem(&fixture.state.path, limited)
            .lookup_path(Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .1
            .hash
            .is_none()
    );
    config.catalog_max_bytes += old_len + new_len;
    let larger = fixture.capture(&config).unwrap();
    assert!(larger.complete);
    assert_eq!(
        filesystem(&fixture.state.path, larger)
            .lookup_path(Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .1
            .hash,
        fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(expected)
            .flatten()
    );
}

#[test]
fn indexed_history_reaches_older_candidates_beyond_status_pages_without_a_sort() {
    let fixture = reliable_fixture();
    let old = fixture.backup().unwrap();
    let current = fixture.backup().unwrap();
    corrupt(&current);
    let transaction = fixture.db.0.unchecked_transaction().unwrap();
    for _ in 0..1100 {
        let mut unrelated = old.clone();
        unrelated.id = uuid::Uuid::new_v4().to_string();
        unrelated.drive = "unrelated".into();
        fixture.db.put("backup", &unrelated.id, &unrelated).unwrap();
    }
    transaction.commit().unwrap();
    let mut query = fixture.db.0.prepare("EXPLAIN QUERY PLAN SELECT rowid FROM entities INDEXED BY entity_kind WHERE kind='backup' ORDER BY rowid DESC").unwrap();
    let plan: Vec<String> = query
        .query_map([], |row| row.get(3))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    assert!(
        plan.iter().all(|step| !step.contains("TEMP B-TREE")),
        "{plan:?}"
    );
    assert_eq!(
        fixture.captured_entry(Path::new("file.txt")).hash,
        fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(expected)
            .flatten()
    );
}

#[test]
fn old_state_index_migration_preserves_current_reuse_and_enables_history() {
    let fixture = reliable_fixture();
    fixture.backup().unwrap();
    let current = fixture.backup().unwrap();
    fixture
        .db
        .0
        .execute_batch("DROP INDEX entity_kind")
        .unwrap();
    assert_eq!(
        fixture.captured_entry(Path::new("file.txt")).hash,
        fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(expected)
            .flatten()
    );
    let upgraded = Store::open(&fixture.state.path.join("drives.sqlite")).unwrap();
    drop(upgraded);
    corrupt(&current);
    assert_eq!(
        fixture.captured_entry(Path::new("file.txt")).hash,
        fixture
            .source
            .queried_change_tracking()
            .unwrap()
            .then(expected)
            .flatten()
    );
}
