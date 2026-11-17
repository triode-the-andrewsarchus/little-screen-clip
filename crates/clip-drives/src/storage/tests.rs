use super::*;
use rusqlite::params;
use std::{fs, sync::atomic::AtomicBool};

fn fixture() -> (tempfile::TempDir, Arc<Root>, Store) {
    let temp = tempfile::tempdir().unwrap();
    let state = Arc::new(Root::create(&temp.path().join("state")).unwrap());
    let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
    fs::create_dir(state.path.join("catalogs")).unwrap();
    (temp, state, db)
}

#[test]
fn accounting_preserves_raw_byte_lengths_and_paginates_optional_catalogs_without_decoding_them() {
    let (_temp, state, db) = fixture();
    let pair = "é";
    let path = b"raw-\xff";
    let hash = "fingerprint";
    db.0.execute(
        "INSERT INTO baseline VALUES(?1,?2,?3)",
        params![pair, path.as_slice(), hash],
    )
    .unwrap();
    let scan = "é:portable";
    let json = r#"{"kind":"file"}"#;
    db.0.execute(
        "INSERT INTO entries VALUES(?1,?2,?3,?4,?5)",
        params![scan, path.as_slice(), b"".as_slice(), path.as_slice(), json],
    )
    .unwrap();
    db.0.execute(
        "INSERT INTO sync_dirs VALUES(?1,?2)",
        params![pair, path.as_slice()],
    )
    .unwrap();
    for index in 0..130 {
        let id = uuid::Uuid::new_v4().to_string();
        db.put("catalog", &id, &()).unwrap();
        if index == 0 {
            // An invalid SQLite payload must still be sized without opening or decoding it.
            fs::write(
                state.path.join("catalogs").join(format!("{id}.sqlite")),
                [0x77; 123],
            )
            .unwrap();
        }
    }
    let version: u64 =
        db.0.query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap();
    let report = measure(state, Arc::new(AtomicBool::new(false))).unwrap();
    assert!(
        report.contains(&format!(
            "Sync baselines: 1 paths · {} logical bytes",
            pair.len() + path.len() + hash.len()
        )),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "Sync/RAM inventories: 1 entries · {} logical bytes",
            scan.len() + path.len() * 2 + json.len()
        )),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "Pending paths and sync bookkeeping: 1 records · {} logical bytes",
            pair.len() + path.len()
        )),
        "{report}"
    );
    assert!(
        report.contains(
            "Saved catalogs (optional): 130 snapshots · 123 available file bytes · 129 unavailable"
        ),
        "{report}"
    );
    let after: u64 =
        db.0.query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap();
    assert_eq!(
        after, version,
        "Measurement must not commit changes through its separate connection"
    );
}

#[test]
fn catalog_measurement_refuses_replaced_types_and_symlinks() {
    let (_temp, state, db) = fixture();
    let id = uuid::Uuid::new_v4().to_string();
    db.put("catalog", &id, &()).unwrap();
    let path = state.path.join("catalogs").join(format!("{id}.sqlite"));
    fs::create_dir(&path).unwrap();
    let error = measure(state.clone(), Arc::new(AtomicBool::new(false))).unwrap_err();
    assert!(error.to_string().contains("file type"), "{error}");
    fs::remove_dir(&path).unwrap();
    std::os::unix::fs::symlink(state.path.join("drives.sqlite"), path).unwrap();
    assert!(measure(state, Arc::new(AtomicBool::new(false))).is_err());
}

#[test]
fn measurement_does_not_switch_to_a_replaced_state_directory() {
    let (temp, state, _) = fixture();
    fs::rename(&state.path, temp.path().join("retained-state")).unwrap();
    fs::create_dir(&state.path).unwrap();
    let error = measure(state, Arc::new(AtomicBool::new(false))).unwrap_err();
    assert!(error.to_string().contains("replaced or removed"), "{error}");
}

#[test]
fn database_and_working_file_aliases_are_refused_before_sqlite_opens_them() {
    for name in [
        "drives.sqlite",
        "drives.sqlite-wal",
        "drives.sqlite-shm",
        "index-policy.json",
    ] {
        let (_temp, state, db) = fixture();
        drop(db);
        let file = state.path.join(name);
        if file.exists() {
            fs::remove_file(&file).unwrap();
        }
        // The SQLite database itself is a valid ordinary target: only following
        // the unexpected alias would make this measurement appear successful.
        let target = state.path.join("retained.sqlite");
        Store::open(&target).unwrap();
        std::os::unix::fs::symlink(target, file).unwrap();
        assert!(
            measure(state, Arc::new(AtomicBool::new(false))).is_err(),
            "{name}"
        );
    }
}

#[test]
#[ignore = "spends the real 30-second measurement budget on bounded-memory SQL"]
fn real_measurement_budget_interrupts_a_long_sql_query() {
    let (_temp, state, db) = fixture();
    db.0.execute_batch("DROP TABLE baseline;
        CREATE VIEW baseline AS WITH RECURSIVE ticks(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ticks WHERE n<1000000000)
        SELECT 'pair' pair,x'61' path,'fingerprint' hash FROM ticks;").unwrap();
    let started = Instant::now();
    let error = measure(state, Arc::new(AtomicBool::new(false))).unwrap_err();
    assert!(error.to_string().contains("30-second budget"), "{error}");
    assert!(started.elapsed() >= LIMIT);
    assert!(started.elapsed() < LIMIT + Duration::from_secs(5));
}
