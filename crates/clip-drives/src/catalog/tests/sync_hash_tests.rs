//! Sync scans are optional hash sources only with their published lifetime evidence.
use super::*;
use crate::{
    SyncPair,
    sync::{self, fingerprints},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::FileTimes,
    os::unix::{ffi::OsStringExt, fs::MetadataExt},
};

fn saved(fixture: &Fixture, path: &Path) -> Entry {
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    assert!(catalog.complete, "{}", catalog.coverage);
    filesystem(&fixture.state.path, catalog)
        .lookup_path(path)
        .unwrap()
        .unwrap()
        .1
}
fn cache(pair: &SyncPair) -> String {
    format!("{}:portable", pair.id)
}
fn provenance(fixture: &Fixture, pair: &SyncPair) -> Option<Value> {
    fixture.db.get(fingerprints::KIND, &cache(pair)).unwrap()
}

fn changed(fixture: &Fixture, pair: &SyncPair, path: PathBuf) -> Result<sync::SyncReport> {
    let pin = fixture.source.directory(&pair.relative, false)?;
    sync::changed(
        &fixture.db,
        pair,
        &Root::open(&crate::root::anchor(&pin))?,
        &Root::open(&pair.local)?,
        &std::collections::HashSet::from([path]),
        &AtomicBool::new(false),
    )
}

#[test]
fn named_source_updates_refresh_only_freshly_observed_eligible_cache_rows() {
    let fixture = reliable_fixture();
    let pair = fixture.pair(Path::new(""));
    let source = fixture.source.path.join("file.txt");
    fs::write(&source, b"changed payload").unwrap();
    changed(&fixture, &pair, source.clone()).unwrap();
    assert_eq!(
        fs::read(pair.local.join("file.txt")).unwrap(),
        b"changed payload"
    );
    let eligible = fixture.source.queried_change_tracking().unwrap();
    assert_eq!(
        saved(&fixture, Path::new("file.txt")).hash.is_some(),
        eligible
    );

    // A hash observed before an incoming copy cannot be assigned to that new
    // destination revision without a fresh read/observation.
    fs::write(pair.local.join("file.txt"), b"locally changed").unwrap();
    changed(&fixture, &pair, pair.local.join("file.txt")).unwrap();
    assert_eq!(fs::read(&source).unwrap(), b"locally changed");
    assert!(saved(&fixture, Path::new("file.txt")).hash.is_none());
    fixture.synchronize(&pair, false).unwrap();
    let expected = eligible.then(|| format!("{:x}", Sha256::digest(b"locally changed")));
    assert_eq!(saved(&fixture, Path::new("file.txt")).hash, expected);

    fixture
        .db
        .remove(fingerprints::KIND, &cache(&pair))
        .unwrap();
    fs::write(&source, b"another version").unwrap();
    changed(&fixture, &pair, source).unwrap();
    assert!(provenance(&fixture, &pair).is_none());
    assert!(saved(&fixture, Path::new("file.txt")).hash.is_none());
}

#[test]
fn named_directory_fallbacks_do_not_reuse_a_whole_unobserved_scan() {
    let fixture = reliable_fixture();
    let pair = fixture.pair(Path::new(""));
    for relative in ["", "excluded"] {
        fixture.poison_cached_hash(&pair, Path::new("file.txt"));
        assert_eq!(
            changed(&fixture, &pair, fixture.source.path.join(relative))
                .unwrap()
                .files,
            0
        );
        assert_eq!(
            fs::read(pair.local.join("file.txt")).unwrap(),
            b"catalog payload"
        );
    }
}

#[test]
fn portable_sync_hashes_enrich_catalogs_and_changed_revisions_are_not_promoted() {
    let fixture = reliable_fixture();
    let pair = fixture.pair(Path::new(""));
    let entry = saved(&fixture, Path::new("file.txt"));
    if !fixture.source.queried_change_tracking().unwrap() {
        assert!(entry.hash.is_none());
        assert!(provenance(&fixture, &pair).is_none());
        return;
    }
    assert!(provenance(&fixture, &pair).is_some());
    assert_eq!(
        entry.hash,
        scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .hash
    );
    assert!(entry.hash.is_some());
    assert!(
        entry.hash_generation.is_none(),
        "catalogs do not persist operational scan markers"
    );
    let path = fixture.source.path.join("file.txt");
    let before = fs::metadata(&path).unwrap();
    fs::write(&path, b"changed payload").unwrap();
    File::open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(before.modified().unwrap()))
        .unwrap();
    let after = fs::metadata(&path).unwrap();
    assert_eq!(
        (before.len(), before.mtime(), before.mtime_nsec()),
        (after.len(), after.mtime(), after.mtime_nsec())
    );
    assert!(saved(&fixture, Path::new("file.txt")).hash.is_none());
    fixture.synchronize(&pair, false).unwrap();
    assert!(saved(&fixture, Path::new("file.txt")).hash.is_some());
    assert_eq!(fs::read(path).unwrap(), b"changed payload");
}

#[test]
fn subtree_and_raw_names_map_to_portable_scan_paths_without_enriching_other_folders() {
    let fixture = reliable_fixture();
    let relative = PathBuf::from(OsString::from_vec(b"sub-\xff".to_vec()));
    let leaf = relative.join(OsString::from_vec(b"leaf-\xfe".to_vec()));
    fs::create_dir(fixture.source.path.join(&relative)).unwrap();
    fs::write(fixture.source.path.join(&leaf), b"raw payload").unwrap();
    let pair = fixture.pair(&relative);
    let entry = saved(&fixture, &leaf);
    assert_eq!(
        entry.hash.is_some(),
        fixture.source.queried_change_tracking().unwrap()
    );
    assert_eq!(
        fs::read(pair.local.join(leaf.file_name().unwrap())).unwrap(),
        b"raw payload"
    );
    assert!(saved(&fixture, Path::new("file.txt")).hash.is_none());
    assert_eq!(entry.path, leaf);
}

#[test]
fn legacy_or_mismatched_row_generations_are_rehashed_before_new_provenance_is_published() {
    for fault in ["provenance", "legacy-row", "wrong-generation"] {
        let fixture = reliable_fixture();
        let pair = fixture.pair(Path::new(""));
        let Some(before) = provenance(&fixture, &pair) else {
            return;
        };
        let mut entry = scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
            .unwrap()
            .unwrap();
        if fault == "provenance" {
            fixture
                .db
                .remove(fingerprints::KIND, &cache(&pair))
                .unwrap();
        } else {
            entry.hash_generation = (fault == "wrong-generation").then(uuid::Uuid::new_v4);
        }
        // A poisoned digest proves a fresh source read, rather than merely
        // adding a new marker to a legacy cached value.
        entry.hash = Some("0".repeat(64));
        scan::put(&fixture.db, &cache(&pair), &entry).unwrap();
        fixture.assert_metadata_only(&["file.txt"]);
        assert_eq!(fixture.synchronize(&pair, true).unwrap().files, 0);
        let expected = Some(format!("{:x}", Sha256::digest(b"catalog payload")));
        assert_eq!(saved(&fixture, Path::new("file.txt")).hash, expected);
        let after = provenance(&fixture, &pair).unwrap();
        assert_ne!(before["generation"], after["generation"]);
    }
}

#[test]
fn missing_or_changed_fingerprint_context_declines_hashes_without_losing_metadata() {
    for field in [
        "missing",
        "generation",
        "version",
        "drive",
        "evidence",
        "birth",
        "boot",
        "namespace",
        "unique_mount",
        "root",
    ] {
        let fixture = reliable_fixture();
        let pair = fixture.pair(Path::new(""));
        let Some(mut record) = provenance(&fixture, &pair) else {
            continue;
        };
        match field {
            "missing" => {
                fixture
                    .db
                    .remove(fingerprints::KIND, &cache(&pair))
                    .unwrap();
            }
            "generation" => record["generation"] = serde_json::to_value(uuid::Uuid::nil()).unwrap(),
            "version" => record["version"] = 999.into(),
            "drive" | "evidence" => record[field] = "another-source".into(),
            "birth" => record["root"]["birth"] = Value::Null,
            "boot" => {
                record["lifetime"]["boot"] = serde_json::to_value(uuid::Uuid::new_v4()).unwrap()
            }
            "namespace" => record["lifetime"]["namespace"]["mount"] = Value::Null,
            "unique_mount" => record["lifetime"]["unique_mount"] = 0.into(),
            "root" => record["root"]["inode"] = 0.into(),
            _ => unreachable!(),
        }
        if field != "missing" {
            fixture
                .db
                .put(fingerprints::KIND, &cache(&pair), &record)
                .unwrap();
        }
        let entry = saved(&fixture, Path::new("file.txt"));
        assert!(entry.hash.is_none(), "{field}");
        assert_eq!(entry.size, b"catalog payload".len() as u64);
    }
}

#[test]
fn scan_and_provenance_publication_fail_atomically_after_completed_file_effects() {
    for named in [false, true] {
        let fixture = reliable_fixture();
        let pair = fixture.pair(Path::new(""));
        let Some(original) = provenance(&fixture, &pair) else {
            continue;
        };
        let old = scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
            .unwrap()
            .unwrap();
        fs::write(fixture.source.path.join("file.txt"), b"new content").unwrap();
        let trigger = if named {
            "CREATE TRIGGER fingerprint_publication_fault BEFORE INSERT ON entries WHEN NEW.scan='owned-sync:portable' BEGIN SELECT RAISE(ABORT,'owned named fingerprint publication fault'); END"
        } else {
            "CREATE TRIGGER fingerprint_publication_fault BEFORE INSERT ON entities WHEN NEW.kind='sync-fingerprints' BEGIN SELECT RAISE(ABORT,'owned fingerprint publication fault'); END"
        };
        fixture.db.0.execute_batch(trigger).unwrap();
        let result = if named {
            changed(&fixture, &pair, fixture.source.path.join("file.txt"))
        } else {
            fixture.synchronize(&pair, false)
        };
        assert!(result.is_err());
        assert_eq!(
            fs::read(pair.local.join("file.txt")).unwrap(),
            b"new content"
        );
        assert_eq!(provenance(&fixture, &pair).unwrap(), original);
        assert_eq!(
            scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
                .unwrap()
                .unwrap()
                .hash,
            old.hash
        );
        assert!(saved(&fixture, Path::new("file.txt")).hash.is_none());
        fixture
            .db
            .0
            .execute_batch("DROP TRIGGER fingerprint_publication_fault")
            .unwrap();
        assert_eq!(fixture.synchronize(&pair, false).unwrap().files, 0);
        assert!(saved(&fixture, Path::new("file.txt")).hash.is_some());
    }
}

#[test]
fn a_late_malformed_sync_row_rolls_back_optional_imports() {
    let fixture = reliable_fixture();
    fs::write(fixture.source.path.join("z-broken"), b"source payload").unwrap();
    let pair = fixture.pair(Path::new(""));
    fixture
        .db
        .0
        .execute(
            "UPDATE entries SET json='invalid json' WHERE scan=?1 AND path=?2",
            rusqlite::params![cache(&pair), b"z-broken".as_slice()],
        )
        .unwrap();
    fixture.assert_metadata_only(&["file.txt", "z-broken"]);
}

#[test]
fn oversized_optional_rows_and_registries_keep_catalogs_metadata_only() {
    for fault in ["pair", "provenance", "entry", "many-pairs"] {
        let fixture = reliable_fixture();
        let pair = fixture.pair(Path::new(""));
        match fault {
            "pair" => {
                let mut large = pair.clone();
                large.status = "x".repeat(20_000);
                fixture.db.put("pair", &pair.id, &large).unwrap();
            }
            "provenance" => {
                if let Some(mut record) = provenance(&fixture, &pair) {
                    record["evidence"] = "x".repeat(20_000).into();
                    fixture
                        .db
                        .put(fingerprints::KIND, &cache(&pair), &record)
                        .unwrap();
                }
            }
            "entry" => {
                let mut entry = scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
                    .unwrap()
                    .unwrap();
                entry.warning = Some("x".repeat(70_000));
                scan::put(&fixture.db, &cache(&pair), &entry).unwrap();
            }
            "many-pairs" => {
                for index in 0..513 {
                    let mut extra = pair.clone();
                    extra.id = format!("extra-{index}");
                    fixture.db.put("pair", &extra.id, &extra).unwrap();
                }
            }
            _ => unreachable!(),
        }
        assert!(
            saved(&fixture, Path::new("file.txt")).hash.is_none(),
            "{fault}"
        );
    }
}

#[test]
fn a_sync_digest_cannot_replace_a_verified_completed_backup_digest() {
    let fixture = reliable_fixture();
    fixture.backup().unwrap();
    let pair = fixture.pair(Path::new(""));
    let before = saved(&fixture, Path::new("file.txt")).hash;
    let mut entry = scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
        .unwrap()
        .unwrap();
    entry.hash = Some("0".repeat(64));
    scan::put(&fixture.db, &cache(&pair), &entry).unwrap();
    assert_eq!(saved(&fixture, Path::new("file.txt")).hash, before);
}

#[test]
#[ignore = "requires isolated native FAT32/exFAT/NTFS sources"]
fn native_coarse_sources_do_not_promote_sync_hashes_without_tracking_provenance() {
    let mut fixture = fixture();
    let (native, _, drive) = crate::sync_edge_tests::native_fs::fixture();
    fs::write(native.path().join("file.txt"), b"catalog payload").unwrap();
    fixture.source = Root::open(native.path()).unwrap();
    fixture.drive = drive;
    let pair = fixture.pair(Path::new(""));
    assert!(
        scan::entry(&fixture.db, &cache(&pair), Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .hash
            .is_some()
    );
    fixture.poison_cached_hash(&pair, Path::new("file.txt"));
    assert_eq!(
        fixture.synchronize(&pair, true).unwrap().files,
        0,
        "coarse source caches require fresh reads"
    );
    assert!(!fixture.source.queried_change_tracking().unwrap());
    assert!(provenance(&fixture, &pair).is_none());
    let entry = saved(&fixture, Path::new("file.txt"));
    assert!(entry.hash.is_none());
    assert_eq!(entry.size, b"catalog payload".len() as u64);
}
