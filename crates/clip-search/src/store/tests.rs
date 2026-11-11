use super::*;
use little_clip_core::file_inventory::read_directory;
use std::sync::atomic::AtomicBool;

fn observation(folder: &Path) -> Entry {
    read_directory(folder, &AtomicBool::new(false))
        .unwrap()
        .entries
        .remove(0)
}
fn segment() -> Segment {
    Segment {
        modality: "text".into(),
        location: "passage:0".into(),
        text: "the cat sat".into(),
        input_hash: vec![1; 32],
        vector: Some(vec![1.0; DIMENSION]),
    }
}
#[test]
fn capacity_counts_replacements_preserves_old_content_and_tracks_removal() {
    use little_clip_core::config::FileSearchLimits;
    let cache = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), "one").unwrap();
    let root = Root::open(source.path()).unwrap();
    let mut store = Store::open(cache.path()).unwrap();
    let root_id = store.enroll(&root).unwrap();
    let generation = store.begin_scan(root_id).unwrap();
    let entry = observation(source.path());
    let (id, _) = store
        .observe(
            root_id,
            generation,
            &entry,
            &std::fs::metadata(&entry.path).unwrap(),
            "model",
        )
        .unwrap();
    store
        .publish(id, entry.revision, "model", &[segment()], "complete")
        .unwrap();
    let limits = FileSearchLimits {
        max_mib: 16,
        max_vectors: 1,
    };
    assert!(
        store
            .capacity(None, &[], 1, 0, &limits)
            .unwrap()
            .unwrap()
            .contains("vector limit")
    );
    assert!(
        store
            .capacity(Some(id), &[segment()], 0, 0, &limits)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .capacity(Some(id), &[segment()], 1, 0, &limits)
            .unwrap()
            .is_some()
    );
    let mut large = segment();
    large.text = "x".repeat(2 * 1024 * 1024);
    assert!(
        store
            .capacity(Some(id), &[large], 0, 0, &limits)
            .unwrap()
            .unwrap()
            .contains("storage limit")
    );
    assert_eq!(count_vectors(&store.connection).unwrap(), 1);
    store
        .publish(id, entry.revision, "model", &[segment()], "complete")
        .unwrap();
    assert_eq!(store.vector_count, 1);
    drop(store);
    let mut store = Store::open(cache.path()).unwrap();
    assert_eq!(store.vector_count, 1);
    store.remove_root(root_id).unwrap();
    assert_eq!(store.vector_count, 0);
    assert!(store.capacity(None, &[], 1, 0, &limits).unwrap().is_none());
    assert!(
        store
            .capacity(None, &[], 0, 2 * 1024 * 1024, &limits)
            .unwrap()
            .unwrap()
            .contains("storage limit")
    );
}

#[test]
fn capacity_includes_wal_bytes_and_detects_sqlite_full() {
    let cache = tempfile::tempdir().unwrap();
    let mut store = Store::open(cache.path()).unwrap();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), "one").unwrap();
    let root_id = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
    let generation = store.begin_scan(root_id).unwrap();
    let entry = observation(source.path());
    let (id, _) = store
        .observe(
            root_id,
            generation,
            &entry,
            &std::fs::metadata(&entry.path).unwrap(),
            "model",
        )
        .unwrap();
    store
        .publish(id, entry.revision, "model", &[segment()], "complete")
        .unwrap();
    store.connection.execute_batch("CREATE TABLE capacity_fixture(payload BLOB); INSERT INTO capacity_fixture VALUES(zeroblob(2097152));").unwrap();
    let wal_bytes = std::fs::metadata(cache.path().join("index.sqlite-wal"))
        .unwrap()
        .len();
    assert!(wal_bytes > 2_000_000);
    assert!(store.storage_bytes().unwrap() >= wal_bytes);
    let pages: u64 = store
        .connection
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap();
    store
        .connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    assert!(
        store
            .connection
            .execute("INSERT INTO capacity_fixture VALUES(zeroblob(2097152))", [])
            .is_err()
    );
    assert!(store.storage_failed());
    let mut large = segment();
    large.text = "large ".repeat(400_000);
    assert!(
        store
            .publish(id, entry.revision, "model", &[large], "complete")
            .is_err()
    );
    assert!(
        store.storage_failed(),
        "Transaction cleanup must preserve the disk-full failure"
    );
    let old: String = store
        .connection
        .query_row("SELECT text FROM segments WHERE file_id=?1", [id], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(old, "the cat sat");
    assert_eq!(store.vector_count, 1);
    assert!(
        store.storage_failed(),
        "Status reads must not erase the recorded failure"
    );
    store.reset_storage_fault().unwrap();
    assert!(!store.storage_failed());
}

#[test]
fn local_index_storage_rejects_network_fuse_and_unknown_filesystems() {
    for magic in [
        0x9123_683e,
        0xef53,
        0x5846_5342,
        0x2fc1_2fc1,
        0xf2f5_2010,
        0xca45_1a4e,
        0x0102_1994,
        0x794c_7630,
    ] {
        assert!(local_index_filesystem(magic).is_ok(), "{magic:#x}");
    }
    for magic in [
        0x6969,
        0xff53_4d42,
        0xfe53_4d42,
        0x0102_1997,
        0x5346_414f,
        0x7373_7245,
        0x6573_5546,
        0x00c3_6400,
        0x0bd0_0bd0,
        0,
        u64::MAX,
    ] {
        let error = local_index_filesystem(magic).unwrap_err();
        assert!(error.contains("local XDG_DATA_HOME"), "{error}");
        assert!(
            error.contains("source folders may still be remote"),
            "{error}"
        );
    }
}

#[test]
fn no_default_enrollment_and_single_writer_ownership() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let store = Store::open(folder.path()).unwrap();
    for (path, mode) in [
        (folder.path().to_path_buf(), 0o700),
        (folder.path().join("index.sqlite"), 0o600),
        (folder.path().join("index.lock"), 0o600),
    ] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            mode
        );
    }
    assert!(store.roots().unwrap().is_empty());
    assert!(Store::open(folder.path()).is_err());
    drop(store);
    assert!(Store::open(folder.path()).is_ok());
}
#[test]
fn changed_revision_rejects_late_vectors_and_fts_is_transactional() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), "one").unwrap();
    let root = Root::open(source.path()).unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let root_id = store.enroll(&root).unwrap();
    let generation = store.begin_scan(root_id).unwrap();
    let mut entry = observation(source.path());
    let metadata = std::fs::metadata(&entry.path).unwrap();
    let (id, pending) = store
        .observe(root_id, generation, &entry, &metadata, "pipeline1")
        .unwrap();
    assert!(pending);
    assert!(
        store
            .publish(id, entry.revision, "pipeline1", &[segment()], "complete")
            .unwrap()
    );
    let count: usize = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
    let (_, pending) = store
        .observe(root_id, generation, &entry, &metadata, "pipeline1")
        .unwrap();
    assert!(!pending);
    let old = entry.revision;
    entry.revision = old.wrapping_add(1);
    store
        .observe(root_id, generation, &entry, &metadata, "pipeline1")
        .unwrap();
    assert!(
        !store
            .publish(id, old, "pipeline1", &[segment()], "complete")
            .unwrap()
    );
    assert!(
        store
            .publish(id, entry.revision, "pipeline1", &[], "unsupported")
            .unwrap()
    );
    let count: usize = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}
#[test]
fn a_new_extractor_reprepares_unchanged_files_without_invalidating_exact_vectors() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), "the cat sat").unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
    let generation = store.begin_scan(root).unwrap();
    let entry = observation(source.path());
    let metadata = std::fs::metadata(&entry.path).unwrap();
    let (id, _) = store
        .observe(root, generation, &entry, &metadata, "model")
        .unwrap();
    store
        .publish(id, entry.revision, "model", &[segment()], "complete")
        .unwrap();
    store
        .connection
        .execute("UPDATE files SET extraction_version=0", [])
        .unwrap();
    let (_, pending) = store
        .observe(root, generation, &entry, &metadata, "model")
        .unwrap();
    assert!(pending);
    assert!(
        store
            .reusable_vector("model", "text", &[1; 32])
            .unwrap()
            .is_some()
    );
    store
        .publish(id, entry.revision, "model", &[segment()], "complete")
        .unwrap();
    assert!(
        !store
            .observe(root, generation, &entry, &metadata, "model")
            .unwrap()
            .1
    );
}

#[test]
fn moved_input_survives_source_retirement_and_reuse_keeps_original_pipeline() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let before = source.path().join("a");
    let after = source.path().join("z");
    std::fs::create_dir(&before).unwrap();
    std::fs::create_dir(&after).unwrap();
    std::fs::write(before.join("one.txt"), "the cat sat").unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
    let generation = store.begin_scan(root).unwrap();
    let entry = observation(&before);
    let (id, _) = store
        .observe(
            root,
            generation,
            &entry,
            &std::fs::metadata(&entry.path).unwrap(),
            "old-pipeline",
        )
        .unwrap();
    store
        .publish(id, entry.revision, "old-pipeline", &[segment()], "complete")
        .unwrap();
    std::fs::rename(&entry.path, after.join("two.txt")).unwrap();
    // The old parent can be reconciled before the destination parent.
    store.retire_path(root, &entry.path).unwrap();
    store.directory_complete(root, generation, &before).unwrap();
    let moved = observation(&after);
    let (moved_id, pending) = store
        .observe(
            root,
            generation,
            &moved,
            &std::fs::metadata(&moved.path).unwrap(),
            "new-pipeline",
        )
        .unwrap();
    assert_eq!(id, moved_id);
    assert!(pending);
    assert!(
        store
            .reusable_vector("old-pipeline", "text", &[1; 32])
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .reusable_vector("new-pipeline", "text", &[1; 32])
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .reusable_vector("old-pipeline", "image", &[1; 32])
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .reusable_vector("old-pipeline", "text", &[2; 32])
            .unwrap()
            .is_none()
    );
    store.prune_unreferenced().unwrap();
    assert!(
        store
            .reusable_vector("old-pipeline", "text", &[1; 32])
            .unwrap()
            .is_some()
    );
    store.remove_root(root).unwrap();
    assert!(
        store
            .reusable_vector("old-pipeline", "text", &[1; 32])
            .unwrap()
            .is_none()
    );
}

#[test]
fn enrollment_does_not_silently_adopt_a_replacement_root() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let root = Root::open(source.path()).unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let id = store.enroll(&root).unwrap();
    store.begin_scan(id).unwrap();
    let replacement = crate::source::SourceRoot {
        path: root.path.clone(),
        device: root.device,
        inode: root.inode.wrapping_add(1),
        mount: Some(root.mount),
        session: None,
    };
    assert!(store.enroll_source(&replacement).is_err());
    assert!(store.enroll_new_source(&replacement).is_err());
    assert_eq!(store.roots().unwrap().len(), 1);
    assert_eq!(store.source_roots().unwrap().len(), 1);
    assert_eq!(
        store.unavailable(&root.path, "Root replaced").unwrap(),
        Some(id)
    );
    assert_eq!(store.roots().unwrap()[0].state, "offline");
    assert!(store.source_roots().unwrap().is_empty());
    assert!(store.enroll(&root).is_ok());
    store.disable_unconfigured(&[]).unwrap();
    assert!(store.enroll_source(&replacement).is_err());
    store.enroll_new_source(&replacement).unwrap();
    let inode: i64 = store
        .connection
        .query_row("SELECT inode FROM roots", [], |row| row.get(0))
        .unwrap();
    assert_eq!(inode as u64, replacement.inode);
}

#[test]
fn v1_migration_preserves_vectors_but_does_not_guess_pending_pipeline() {
    let folder = tempfile::tempdir().unwrap();
    let connection = Connection::open(folder.path().join("index.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE roots(id INTEGER PRIMARY KEY,path BLOB,device INTEGER,inode INTEGER);
        CREATE TABLE files(id INTEGER PRIMARY KEY,device INTEGER,inode INTEGER,revision INTEGER DEFAULT 0,pipeline TEXT,metadata TEXT DEFAULT '{}',state TEXT,coverage TEXT,error TEXT);
        CREATE TABLE paths(path BLOB PRIMARY KEY,parent BLOB,file_id INTEGER); CREATE TABLE memberships(root_id INTEGER,path BLOB,verified INTEGER);
        INSERT INTO roots(id,path,device,inode) VALUES(1,CAST('/ordinary/legacy' AS BLOB),1,1);
        CREATE TABLE segments(id INTEGER PRIMARY KEY,file_id INTEGER,modality TEXT,text TEXT DEFAULT '',input_hash BLOB,vector BLOB);
        INSERT INTO files(id,device,inode,pipeline,state) VALUES(1,1,1,'original','ready'),(2,1,2,'changed','pending');
        PRAGMA user_version=1;",
        )
        .unwrap();
    for id in [1, 2] {
        connection
            .execute(
                "INSERT INTO segments(file_id,modality,text,input_hash,vector) VALUES(?1,'text','persisted café text',?2,?3)",
                params![
                    id,
                    vec![id as u8; 32],
                    encode_vector(&vec![1.0; DIMENSION]).unwrap()
                ],
            )
            .unwrap();
    }
    drop(connection);
    let store = Store::open(folder.path()).unwrap();
    let root: (i64, i64, String) = store
        .connection
        .query_row(
            "SELECT available,generation,state FROM roots WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(root, (0, 0, "checking".into()));
    store
        .connection
        .prepare("SELECT generation FROM memberships")
        .unwrap();
    assert_eq!(
        store
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cafe'",
                [],
                |row| row.get::<_, usize>(0)
            )
            .unwrap(),
        2
    );
    assert!(
        store
            .reusable_vector("original", "text", &[1; 32])
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .reusable_vector("changed", "text", &[2; 32])
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM segments", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        2
    );
}

#[test]
fn incomplete_scans_keep_inventory_and_hard_link_memberships_are_independent() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let a = source.path().join("a");
    let b = source.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("one.txt"), "one").unwrap();
    std::fs::hard_link(a.join("one.txt"), b.join("two.txt")).unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let roots: Vec<_> = [&a, &b]
        .iter()
        .map(|path| store.enroll(&Root::open(path).unwrap()).unwrap())
        .collect();
    for (path, id) in [&a, &b].iter().zip(&roots) {
        let generation = store.begin_scan(*id).unwrap();
        let entry = observation(path);
        let metadata = std::fs::metadata(&entry.path).unwrap();
        store
            .observe(*id, generation, &entry, &metadata, "pipeline")
            .unwrap();
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, usize>(0))
            .unwrap(),
        1
    );
    // The same path can independently belong to an enclosing root too.
    let parent = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
    let generation = store.begin_scan(parent).unwrap();
    for directory in [&a, &b] {
        let entry = observation(directory);
        let (file, _) = store
            .observe_object(
                parent,
                generation,
                &entry,
                entry.identity.unwrap().object(),
                "pipeline",
            )
            .unwrap();
        store
            .publish(file, entry.revision, "pipeline", &[segment()], "complete")
            .unwrap();
    }
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM memberships", [], |r| r
                .get::<_, usize>(0))
            .unwrap(),
        4
    );
    let summaries = store.roots().unwrap();
    let enclosing = summaries.iter().find(|root| root.id == parent).unwrap();
    assert_eq!((enclosing.files, enclosing.vectors), (2, 1));
    assert!(summaries.iter().all(|root| root.vectors == 1));
    store.begin_scan(parent).unwrap();
    store
        .finish_scan(parent, Some("cancelled or incomplete inventory"))
        .unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM paths", [], |r| r.get::<_, usize>(0))
            .unwrap(),
        2
    );
    store.retire_path(parent, &a).unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM memberships", [], |r| r
                .get::<_, usize>(0))
            .unwrap(),
        3
    );
    store.remove_root(parent).unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM memberships", [], |r| r
                .get::<_, usize>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        store
            .connection
            .query_row("SELECT count(*) FROM paths", [], |r| r.get::<_, usize>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        store.vector_count, 1,
        "Retiring one owner cannot prune a shared vector"
    );
    store.begin_scan(roots[0]).unwrap();
    store
        .finish_scan(roots[0], Some("directory unavailable"))
        .unwrap();
    assert_eq!(store.roots().unwrap()[0].files, 1);
    store.remove_root(roots[0]).unwrap();
    assert_eq!(store.roots().unwrap()[0].files, 1);
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, usize>(0))
            .unwrap(),
        1
    );
}
#[test]
fn delta_invalidation_preserves_other_directories_and_pending_coverage_stays_partial() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
    let generation = store.begin_scan(root).unwrap();
    for name in ["a", "b"] {
        let directory = source.path().join(name);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("one.txt"), "text").unwrap();
        let entry = observation(&directory);
        let (id, _) = store
            .observe(
                root,
                generation,
                &entry,
                &std::fs::metadata(&entry.path).unwrap(),
                "pipeline",
            )
            .unwrap();
        store
            .publish(id, entry.revision, "pipeline", &[segment()], "complete")
            .unwrap();
        store
            .directory_complete(root, generation, &directory)
            .unwrap();
    }
    store
        .invalidate_directory(root, Some(&source.path().join("a")))
        .unwrap();
    let next = store.begin_delta(root).unwrap();
    assert!(next > generation);
    let verified: usize = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM memberships WHERE verified=1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(verified, 1);
    store.finish_scan(root, None).unwrap();
    assert_eq!(store.roots().unwrap()[0].state, "partial");
    assert!(
        store
            .known_directory(root, &source.path().join("b"))
            .unwrap()
    );
}

#[test]
fn only_completed_directory_membership_can_retire_an_unseen_path() {
    let folder = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("one.txt"), "one").unwrap();
    let mut store = Store::open(folder.path()).unwrap();
    let id = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
    let generation = store.begin_scan(id).unwrap();
    let entry = observation(source.path());
    store
        .observe(
            id,
            generation,
            &entry,
            &std::fs::metadata(&entry.path).unwrap(),
            "p",
        )
        .unwrap();
    let generation = store.begin_scan(id).unwrap();
    assert_eq!(store.roots().unwrap()[0].files, 1);
    std::fs::remove_file(&entry.path).unwrap();
    store
        .directory_complete(id, generation, source.path())
        .unwrap();
    assert_eq!(
        store.roots().unwrap()[0].files,
        1,
        "Enumeration alone cannot prove absence"
    );
    assert_eq!(
        store
            .unseen_children(id, generation, source.path())
            .unwrap(),
        [entry.path.clone()]
    );
    store.retire_path(id, &entry.path).unwrap();
    store.finish_scan(id, None).unwrap();
    let root = &store.roots().unwrap()[0];
    assert_eq!(root.files, 0);
    assert_eq!(root.state, "ready");
}
#[test]
fn batched_half_decoding_matches_scalar_normalization() {
    for chunk in (0..=u16::MAX)
        .filter(|bits| half::f16::from_bits(*bits).is_finite())
        .collect::<Vec<_>>()
        .chunks(DIMENSION)
    {
        let mut halves = chunk
            .iter()
            .map(|&bits| half::f16::from_bits(bits))
            .collect::<Vec<_>>();
        halves.resize(DIMENSION, half::f16::ONE);
        let bytes = halves
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        let mut reference = halves
            .iter()
            .map(|value| value.to_f32())
            .collect::<Vec<_>>();
        let norm = reference
            .iter()
            .map(|&value| f64::from(value).powi(2))
            .sum::<f64>()
            .sqrt();
        for value in &mut reference {
            *value = (f64::from(*value) / norm) as f32;
        }
        let actual = decode_vector(&bytes).unwrap();
        assert!(
            actual
                .iter()
                .zip(reference)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
    }
    for value in [
        half::f16::NAN,
        half::f16::INFINITY,
        half::f16::NEG_INFINITY,
        half::f16::ZERO,
    ] {
        assert!(decode_vector(&value.to_le_bytes().repeat(DIMENSION)).is_err());
    }
}

#[test]
fn reopening_current_schema_recreates_derived_indexes_without_rebuilding_records() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    store.connection.execute_batch("DROP INDEX path_file_lookup; DROP INDEX path_parent; DROP INDEX file_search; DROP INDEX segment_identity; DROP INDEX segment_admission; DROP INDEX segment_vector_file; DROP INDEX membership_search;").unwrap();
    drop(store);
    let reopened = Store::open(folder.path()).unwrap();
    let count: usize = reopened.connection.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name IN ('path_file_lookup','path_parent','file_search','segment_identity','segment_admission','segment_vector_file','membership_search')",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 7);
    assert_eq!(
        reopened
            .connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        SCHEMA_VERSION
    );
}

#[test]
fn rtf_extractor_retries_only_legacy_name_only_outcomes() {
    for (suffix, coverage, retry) in [
        ("rtf", "name-only: extractor unavailable", true),
        ("RTF", "name-only: extractor unavailable", true),
        ("unknown", "name-only: extractor unavailable", false),
        ("rtf", "complete", false),
        (
            "rtf",
            "name-only: document exceeds 64 MiB extraction budget",
            false,
        ),
    ] {
        let mut fixture = test_support::Fixture::new();
        let path = fixture.entry.path.with_extension(suffix);
        std::fs::rename(&fixture.entry.path, &path).unwrap();
        let entry = observation(path.parent().unwrap());
        let root = fixture.store.roots().unwrap()[0].id;
        let generation = fixture.store.begin_scan(root).unwrap();
        let object = entry.identity.unwrap().object();
        let (file, _) = fixture
            .store
            .observe_object(root, generation, &entry, object, "model")
            .unwrap();
        fixture
            .store
            .publish(file, entry.revision, "model", &[], coverage)
            .unwrap();
        let (same, changed) = fixture
            .store
            .observe_object(root, generation, &entry, object, "model")
            .unwrap();
        assert_eq!(same, file);
        assert_eq!(changed, retry, "{suffix}: {coverage}");
        if retry {
            fixture
                .store
                .publish(file, entry.revision, "model", &[segment()], "complete")
                .unwrap();
            assert!(
                !fixture
                    .store
                    .observe_object(root, generation, &entry, object, "model")
                    .unwrap()
                    .1
            );
        }
    }
}

#[test]
fn mixed_format_hardlinks_keep_independent_publications_and_compatible_aliases_share() {
    let mut fixture = test_support::Fixture::new();
    let directory = fixture.entry.path.parent().unwrap().to_path_buf();
    for name in ["alias.TXT", "page.html", "unknown.data"] {
        std::fs::hard_link(&fixture.entry.path, directory.join(name)).unwrap();
    }
    let entries =
        little_clip_core::file_inventory::read_directory(&directory, &AtomicBool::new(false))
            .unwrap()
            .entries;
    let root = fixture.store.roots().unwrap()[0].id;
    let generation = fixture.store.begin_scan(root).unwrap();
    let mut observed = Vec::new();
    for entry in entries {
        let (file, _) = fixture
            .store
            .observe_object(
                root,
                generation,
                &entry,
                entry.identity.unwrap().object(),
                "model",
            )
            .unwrap();
        observed.push((entry, file));
    }
    let find = |name: &str| {
        observed
            .iter()
            .find(|(entry, _)| entry.name == name)
            .unwrap()
    };
    let (original, text) = find("note.txt");
    let (_, compatible) = find("alias.TXT");
    let (html, decoded) = find("page.html");
    let (unknown, unsupported) = find("unknown.data");
    assert_eq!(text, compatible);
    assert_ne!(text, decoded);
    assert_ne!(text, unsupported);
    assert_ne!(decoded, unsupported);
    fixture
        .store
        .publish(*text, original.revision, "model", &[segment()], "complete")
        .unwrap();
    fixture
        .store
        .publish(
            *decoded,
            html.revision,
            "model",
            &[
                segment(),
                Segment {
                    vector: None,
                    ..segment()
                },
            ],
            "complete",
        )
        .unwrap();
    fixture
        .store
        .publish(
            *unsupported,
            unknown.revision,
            "model",
            &[],
            super::super::source_format::NAME_ONLY_UNSUPPORTED,
        )
        .unwrap();
    let state = |file| {
        fixture.store.connection.query_row(
        "SELECT state,(SELECT count(*) FROM segments WHERE file_id=files.id AND vector IS NOT NULL) FROM files WHERE id=?1",
        [file], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    ).unwrap()
    };
    assert_eq!(state(*text), ("ready".into(), 1));
    assert_eq!(state(*decoded), ("ready".into(), 1));
    assert_eq!(state(*unsupported), ("unsupported".into(), 0));
    let summary = fixture.store.roots().unwrap().remove(0);
    assert_eq!((summary.files, summary.vectors), (4, 2));
    fixture
        .store
        .invalidate_directory(root, Some(&directory))
        .unwrap();
    let unverified = fixture.store.roots().unwrap().remove(0);
    assert_eq!((unverified.files, unverified.vectors), (4, 0));
    let alias = &find("alias.TXT").0;
    fixture
        .store
        .observe_object(
            root,
            generation,
            alias,
            alias.identity.unwrap().object(),
            "model",
        )
        .unwrap();
    let reverified = fixture.store.roots().unwrap().remove(0);
    assert_eq!((reverified.files, reverified.vectors), (4, 1));
    // Removing an alias must not retire the remaining compatible pathname.
    fixture.store.retire_path(root, &original.path).unwrap();
    assert_eq!(
        fixture
            .store
            .connection
            .query_row(
                "SELECT file_id FROM paths WHERE path=?1",
                [find("alias.TXT").0.path.as_os_str().as_bytes()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        *text
    );
}

#[test]
fn extensionless_content_routes_do_not_share_unknown_name_only_state() {
    let mut entry = test_support::Fixture::new().entry;
    entry.path.set_file_name("README");
    assert!(super::super::source_format::supports_content(&entry));
    let content = source_format(&entry);
    entry.path.set_file_name("unknown");
    assert!(!super::super::source_format::supports_content(&entry));
    assert_ne!(content, source_format(&entry));
}

#[test]
fn changed_subtype_reprepares_only_the_affected_file_without_losing_canonical_vectors() {
    let mut fixture = test_support::Fixture::new();
    let path = fixture.entry.path.with_file_name("module.ts");
    std::fs::rename(&fixture.entry.path, &path).unwrap();
    let entry = observation(path.parent().unwrap());
    assert!(entry.typescript);
    let mut legacy = entry.clone();
    legacy.typescript = false;
    let root = fixture.store.roots().unwrap()[0].id;
    let generation = fixture.store.begin_scan(root).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let (file, _) = fixture
        .store
        .observe(root, generation, &legacy, &metadata, "model")
        .unwrap();
    let prepared = segment();
    let input = prepared.input_hash.clone();
    fixture
        .store
        .publish(file, entry.revision, "model", &[prepared], "complete")
        .unwrap();
    assert!(
        !fixture
            .store
            .observe(root, generation, &legacy, &metadata, "model")
            .unwrap()
            .1
    );
    let (decoded_file, changed) = fixture
        .store
        .observe(root, generation, &entry, &metadata, "model")
        .unwrap();
    assert_ne!(decoded_file, file);
    assert!(
        changed,
        "same source revision but a different decoder must not reuse ready state"
    );
    let state: String = fixture
        .store
        .connection
        .query_row(
            "SELECT state FROM files WHERE id=?1",
            [decoded_file],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "pending");
    assert!(
        fixture
            .store
            .reusable_vector("model", "text", &input)
            .unwrap()
            .is_some()
    );
}

#[test]
fn full_source_reuse_requires_exact_digest_format_pipeline_and_complete_publication() {
    let mut fixture = test_support::Fixture::new();
    let store = &mut fixture.store;
    let entry = &fixture.entry;
    let id = fixture.file;
    let root_id = store.enroll(&fixture.root).unwrap();
    let generation = store.begin_scan(root_id).unwrap();
    let digest = [7; 32];
    store
        .remember_source(id, entry.revision.wrapping_add(1), "model", entry, &digest)
        .unwrap();
    assert!(
        store
            .reusable_source(entry, "model", &digest)
            .unwrap()
            .is_none()
    );
    store
        .remember_source(id, entry.revision, "model", entry, &digest)
        .unwrap();
    let (reused, coverage) = store
        .reusable_source(entry, "model", &digest)
        .unwrap()
        .unwrap();
    assert_eq!(coverage, "complete");
    assert_eq!(reused.len(), 1);
    assert_eq!(reused[0].text, "original content");
    assert_eq!(reused[0].input_hash, segment().input_hash);
    assert_eq!(
        encode_vector(reused[0].vector.as_ref().unwrap()).unwrap(),
        encode_vector(segment().vector.as_ref().unwrap()).unwrap()
    );
    assert!(
        store
            .reusable_source(entry, "different-model", &digest)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .reusable_source(entry, "model", &[8; 32])
            .unwrap()
            .is_none()
    );
    let mut different_format = entry.clone();
    different_format.path.set_extension("pdf");
    assert!(
        store
            .reusable_source(&different_format, "model", &digest)
            .unwrap()
            .is_none()
    );
    let mut typed = entry.clone();
    typed.path.set_extension("ts");
    typed.typescript = false;
    store
        .remember_source(id, entry.revision, "model", &typed, &digest)
        .unwrap();
    assert!(
        store
            .reusable_source(&typed, "model", &digest)
            .unwrap()
            .is_some()
    );
    typed.typescript = true;
    assert!(
        store
            .reusable_source(&typed, "model", &digest)
            .unwrap()
            .is_none()
    );
    store
        .remember_source(id, entry.revision, "model", entry, &digest)
        .unwrap();
    // A revision change keeps the old extraction eligible only by its full
    // digest; it is never an unchanged-metadata authorization.
    let mut changed = entry.clone();
    changed.revision = changed.revision.wrapping_add(1);
    assert!(
        store
            .observe(
                root_id,
                generation,
                &changed,
                &std::fs::metadata(&entry.path).unwrap(),
                "model"
            )
            .unwrap()
            .1
    );
    assert!(
        store
            .reusable_source(&changed, "model", &digest)
            .unwrap()
            .is_some()
    );
    store
        .publish(
            id,
            changed.revision,
            "model",
            &[segment()],
            "partial: interrupted",
        )
        .unwrap();
    store
        .remember_source(id, changed.revision, "model", &changed, &digest)
        .unwrap();
    assert!(
        store
            .reusable_source(&changed, "model", &digest)
            .unwrap()
            .is_none()
    );
    store
        .publish(id, changed.revision, "model", &[segment()], "complete")
        .unwrap();
    store
        .remember_source(id, changed.revision, "model", &changed, &digest)
        .unwrap();
    store
        .connection
        .execute("DELETE FROM files WHERE id=?1", [id])
        .unwrap();
    assert_eq!(
        store
            .connection
            .query_row("SELECT COUNT(*) FROM source_digests", [], |row| row
                .get::<_, usize>(0))
            .unwrap(),
        0
    );
}

#[test]
fn compact_vectors_are_normalized_and_invalid_payloads_cannot_publish() {
    let encoded = encode_vector(&vec![0.1; DIMENSION]).unwrap();
    assert_eq!(encoded.len(), 1536);
    let values = decode_vector(&encoded).unwrap();
    assert!((values.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(encode_vector(&vec![f32::NAN; DIMENSION]).is_err());
    assert!(encode_vector(&vec![0.0; DIMENSION]).is_err());
    assert!(decode_vector(&encoded[..1500]).is_err());
}

#[test]
fn failed_schema_upgrade_rolls_back_every_prior_step() {
    let connection = Connection::open_in_memory().unwrap();
    // A malformed legacy schema lets the pipeline step start, then rejects
    // the next step. None of that partially upgraded schema may survive.
    connection.execute_batch("CREATE TABLE files(id INTEGER PRIMARY KEY,pipeline TEXT,state TEXT,extraction_version INTEGER); CREATE TABLE segments(id INTEGER PRIMARY KEY,file_id INTEGER,modality TEXT,input_hash BLOB); PRAGMA user_version=1;").unwrap();
    assert!(migrate(&connection, 1).is_err());
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let columns = connection
        .prepare("SELECT name FROM pragma_table_info('segments')")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!columns.iter().any(|name| name == "pipeline"));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='segment_input'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
