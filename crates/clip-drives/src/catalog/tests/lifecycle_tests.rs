//! Capture lifetime failures must not accumulate unpublished metadata.
use super::*;
use little_clip_core::process::CommandRequest;
use serde_json::Value;
use std::sync::Arc;
use std::{fs, sync::atomic::Ordering};

fn source_fixture() -> (Fixture, Option<tempfile::TempDir>) {
    let mut fixture = fixture();
    let native = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").map(|_| {
        let (native, _, drive) = crate::sync_edge_tests::native_fs::fixture();
        fixture.drive = drive;
        fixture.source = Root::open(native.path()).unwrap();
        fs::write(native.path().join("file.txt"), b"catalog payload").unwrap();
        fs::create_dir(native.path().join("excluded")).unwrap();
        fs::write(native.path().join("excluded/hidden"), b"never catalogued").unwrap();
        let mut rule: DriveRule = fixture.db.get("rule", "exclude").unwrap().unwrap();
        rule.drive = fixture.drive.id.clone();
        fixture.db.put("rule", "exclude", &rule).unwrap();
        native
    });
    (fixture, native)
}
fn pending(fixture: &Fixture) -> Value {
    let mut pending = fixture.db.list::<Value>(staging::KIND, 0, 10).unwrap();
    assert_eq!(pending.len(), 1);
    pending.pop().unwrap()
}
fn recover(fixture: &Fixture, intent: &Value) -> Result<bool> {
    staging::apply(
        Arc::new(Root::open(&fixture.state.path)?),
        intent["id"].as_str().unwrap(),
        &AtomicBool::new(false),
    )
}
fn stage_path(fixture: &Fixture, intent: &Value) -> PathBuf {
    fixture
        .state
        .path
        .join("catalogs")
        .join(intent["stage"].as_str().unwrap())
}
fn crash_after_batch(fixture: &Fixture) -> Value {
    for index in 0..300 {
        fs::write(
            fixture.source.path.join(format!("crash-{index:03}")),
            b"source payload",
        )
        .unwrap();
    }
    run_probe(fixture, "BATCH", 77);
    let intent = pending(fixture);
    assert!(intent["ready"].is_null());
    let db = Store::existing(&stage_path(fixture, &intent).join("metadata.sqlite"), true).unwrap();
    assert!(scan::entries(&db, "catalog", b"", 256).unwrap().len() >= 128);
    intent
}
fn run_probe(fixture: &Fixture, point: &str, code: i32) {
    let token = uuid::Uuid::new_v4().to_string();
    fs::write(fixture.temp.path().join(".catalog-lifecycle-owned"), &token).unwrap();
    let mut root_env = OsString::from("LITTLE_CLIP_CAPTURE_CRASH_ROOT=");
    root_env.push(fixture.temp.path());
    let output = CommandRequest::new("/usr/bin/env")
        .arg(root_env)
        .arg(format!("LITTLE_CLIP_CAPTURE_CRASH_TOKEN={token}"))
        .arg(format!("LITTLE_CLIP_CAPTURE_CRASH_POINT={point}"))
        .arg(std::env::current_exe().unwrap())
        .args([
            "catalog::tests::lifecycle_tests::capture_crash_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .timeout(Duration::from_secs(30))
        .own_process_group()
        .die_with_parent()
        .run()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("CAPTURE_EXIT_AFTER_{point}"))
    );
}

fn seal_snapshot(db: &Store, state: &Root, drive: &crate::Drive, saved: &Catalog) {
    let mut staged = staging::Capture::new(db, state, drive).unwrap();
    fs::copy(
        state
            .path
            .join("catalogs")
            .join(format!("{}.sqlite", saved.id)),
        staged.path(),
    )
    .unwrap();
    let metadata = Store::existing(&staged.path(), false).unwrap();
    let mut catalog = saved.clone();
    catalog.id = staged.id().to_owned();
    metadata.remove("catalog", &saved.id).unwrap();
    metadata.put("catalog", &catalog.id, &catalog).unwrap();
    staged
        .seal(
            metadata,
            &catalog,
            &[PathBuf::from("excluded")],
            &AtomicBool::new(false),
        )
        .unwrap();
}

#[test]
#[ignore = "disposable child of the owned catalog lifecycle fixture"]
fn capture_crash_probe() {
    let root = PathBuf::from(std::env::var_os("LITTLE_CLIP_CAPTURE_CRASH_ROOT").unwrap());
    let token = std::env::var("LITTLE_CLIP_CAPTURE_CRASH_TOKEN").unwrap();
    assert_eq!(
        fs::read_to_string(root.join(".catalog-lifecycle-owned")).unwrap(),
        token
    );
    let state = Root::open(&root.join("state")).unwrap();
    let db = Store::existing(&state.path.join("drives.sqlite"), false).unwrap();
    let saved = db.list::<Catalog>("catalog", 0, 1).unwrap().pop().unwrap();
    let metadata = Store::existing(
        &state
            .path
            .join("catalogs")
            .join(format!("{}.sqlite", saved.id)),
        true,
    )
    .unwrap();
    let drive: crate::Drive = metadata.get("source", "drive").unwrap().unwrap();
    if std::env::var("LITTLE_CLIP_CAPTURE_CRASH_POINT").unwrap() == "SEAL" {
        seal_snapshot(&db, &state, &drive, &saved);
        println!("CAPTURE_EXIT_AFTER_SEAL");
        std::process::exit(79);
    }
    let source = Root::open(drive.mount.as_ref().unwrap()).unwrap();
    let _ = capture(
        &db,
        &state,
        &drive,
        &source,
        &DriveConfig::default(),
        &AtomicBool::new(false),
        |files, _| {
            if files >= 128 {
                println!("CAPTURE_EXIT_AFTER_BATCH");
                std::process::exit(77);
            }
        },
    );
    panic!("capture never reached the process-exit barrier");
}

#[test]
fn cancelled_scan_retires_unpublished_metadata_and_preserves_completed_catalogs() {
    let (fixture, _native) = source_fixture();
    let retained = fixture.capture(&DriveConfig::default()).unwrap();
    let old_path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{}.sqlite", retained.id));
    let old_bytes = fs::read(&old_path).unwrap();
    for index in 0..300 {
        fs::write(
            fixture.source.path.join(format!("entry-{index:03}")),
            b"source payload",
        )
        .unwrap();
    }
    let cancel = AtomicBool::new(false);
    let mut interrupted = false;
    let result = capture(
        &fixture.db,
        &fixture.state,
        &fixture.drive,
        &fixture.source,
        &DriveConfig::default(),
        &cancel,
        |files, _| {
            if files >= 128 {
                interrupted = true;
                cancel.store(true, Ordering::Release);
            }
        },
    );
    assert!(
        interrupted,
        "cancellation must follow a committed inventory batch"
    );
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .to_lowercase()
            .contains("cancel")
    );
    assert_eq!(
        fixture.db.list::<Catalog>("catalog", 0, 10).unwrap().len(),
        1
    );
    assert_eq!(fs::read(old_path).unwrap(), old_bytes);
    let names: Vec<_> = fs::read_dir(fixture.state.path.join("catalogs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from(format!("{}.sqlite", retained.id))]
    );
    assert_eq!(
        fs::read(fixture.source.path.join("entry-299")).unwrap(),
        b"source payload"
    );
    assert!(
        fixture
            .db
            .list::<Value>(staging::KIND, 0, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn restart_retires_an_abruptly_interrupted_capture_without_touching_completed_metadata() {
    let (fixture, _native) = source_fixture();
    let retained = fixture.capture(&DriveConfig::default()).unwrap();
    let path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{}.sqlite", retained.id));
    let bytes = fs::read(&path).unwrap();
    let intent = crash_after_batch(&fixture);
    assert!(recover(&fixture, &intent).unwrap());
    assert!(!stage_path(&fixture, &intent).exists());
    assert!(
        fixture
            .db
            .list::<Value>(staging::KIND, 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(!recover(&fixture, &intent).unwrap());
    assert_eq!(fs::read(path).unwrap(), bytes);
    assert_eq!(
        fixture.db.list::<Catalog>("catalog", 0, 10).unwrap().len(),
        1
    );
}

#[test]
fn process_exit_after_sealing_before_installation_keeps_a_recoverable_complete_catalog() {
    let (fixture, _native) = source_fixture();
    let original = fixture.capture(&DriveConfig::default()).unwrap();
    run_probe(&fixture, "SEAL", 79);
    let intent = pending(&fixture);
    assert!(intent["ready"].is_object());
    assert!(
        stage_path(&fixture, &intent)
            .join("metadata.sqlite")
            .is_file()
    );
    let id = intent["id"].as_str().unwrap();
    let final_path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{id}.sqlite"));
    assert!(!final_path.exists());
    assert!(recover(&fixture, &intent).unwrap());
    let completed: Catalog = fixture.db.get("catalog", id).unwrap().unwrap();
    assert_eq!(completed.entries, original.entries);
    assert_eq!(completed.complete, original.complete);
    let metadata = Store::existing(&final_path, true).unwrap();
    assert_eq!(
        scan::entries(&metadata, "catalog", b"", 128).unwrap().len() as u64,
        completed.entries
    );
    assert!(!stage_path(&fixture, &intent).exists());
}

fn sealed_after_publication_fault(fixture: &Fixture) -> Value {
    fixture.db.0.execute_batch("CREATE TRIGGER capture_commit_fault BEFORE DELETE ON entities WHEN OLD.kind='catalog-capture' BEGIN SELECT RAISE(ABORT,'catalog publication fault'); END;").unwrap();
    let error = fixture.capture(&DriveConfig::default()).unwrap_err();
    assert!(
        error.to_string().contains("catalog publication fault"),
        "{error}"
    );
    let intent = pending(fixture);
    assert!(intent["ready"].is_object());
    assert!(!stage_path(fixture, &intent).exists());
    assert!(
        fixture
            .db
            .get::<Catalog>("catalog", intent["id"].as_str().unwrap())
            .unwrap()
            .is_none()
    );
    intent
}
fn ready_artifact(fixture: &Fixture) -> (Value, PathBuf, Vec<u8>) {
    let intent = sealed_after_publication_fault(fixture);
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER capture_commit_fault")
        .unwrap();
    let path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{}.sqlite", intent["id"].as_str().unwrap()));
    let bytes = fs::read(&path).unwrap();
    (intent, path, bytes)
}

#[test]
fn a_sealed_catalog_survives_a_failed_registry_transaction_and_recovers_with_its_drive_offline() {
    let (fixture, _native) = source_fixture();
    let retained = fixture.capture(&DriveConfig::default()).unwrap();
    let intent = sealed_after_publication_fault(&fixture);
    let final_path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{}.sqlite", intent["id"].as_str().unwrap()));
    let bytes = fs::read(&final_path).unwrap();
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER capture_commit_fault")
        .unwrap();
    let offline = fixture.source.path.with_extension("offline");
    fs::rename(&fixture.source.path, &offline).unwrap();
    let service = crate::Service::start_with_inventory(
        DriveConfig::default(),
        fixture.state.path.clone(),
        Arc::new(|| Ok(Vec::new())),
    )
    .unwrap();
    let started = std::time::Instant::now();
    while fixture
        .db
        .get::<Catalog>("catalog", intent["id"].as_str().unwrap())
        .unwrap()
        .is_none()
    {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "startup metadata maintenance did not recover the sealed capture"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    service.shutdown().unwrap();
    fs::rename(offline, &fixture.source.path).unwrap();
    assert_eq!(fs::read(final_path).unwrap(), bytes);
    assert!(
        fixture
            .db
            .get::<Catalog>("catalog", &retained.id)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .db
            .list::<Value>(staging::KIND, 0, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn replaced_capture_parents_and_directories_are_retained_until_the_original_owner_returns() {
    for replace_parent in [false, true] {
        let (fixture, _native) = source_fixture();
        fixture.capture(&DriveConfig::default()).unwrap();
        let intent = crash_after_batch(&fixture);
        let original = if replace_parent {
            fixture.state.path.join("catalogs")
        } else {
            stage_path(&fixture, &intent)
        };
        let moved = original.with_extension("retained");
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        fs::write(original.join("do-not-remove"), b"replacement data").unwrap();
        let error = recover(&fixture, &intent).unwrap_err();
        assert!(error.to_string().contains("replaced"), "{error}");
        assert_eq!(
            fs::read(original.join("do-not-remove")).unwrap(),
            b"replacement data"
        );
        assert!(moved.is_dir());
        assert_eq!(
            fixture
                .db
                .list::<Value>(staging::KIND, 0, 10)
                .unwrap()
                .len(),
            1
        );
        fs::remove_file(original.join("do-not-remove")).unwrap();
        fs::remove_dir(&original).unwrap();
        fs::rename(moved, original).unwrap();
        assert!(recover(&fixture, &intent).unwrap());
        assert!(
            fixture
                .db
                .list::<crate::DriveRecovery>("recovery", 0, 10)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn unexpected_entries_and_replaced_database_files_cannot_authorize_stage_cleanup() {
    for replace_file in [false, true] {
        let (fixture, _native) = source_fixture();
        fixture.capture(&DriveConfig::default()).unwrap();
        let intent = crash_after_batch(&fixture);
        let stage = stage_path(&fixture, &intent);
        let database = stage.join("metadata.sqlite");
        let original = fs::read(&database).unwrap();
        let path = if replace_file {
            fs::rename(&database, fixture.temp.path().join("retained.sqlite")).unwrap();
            database.clone()
        } else {
            stage.join("unexpected")
        };
        fs::write(&path, b"keep this data").unwrap();
        assert!(recover(&fixture, &intent).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep this data");
        if replace_file {
            fs::remove_file(path).unwrap();
            fs::rename(fixture.temp.path().join("retained.sqlite"), &database).unwrap();
        } else {
            assert_eq!(fs::read(database).unwrap(), original);
            fs::remove_file(path).unwrap();
        }
        assert!(recover(&fixture, &intent).unwrap());
        assert!(!stage.exists());
    }
}

#[test]
fn changed_sealed_bytes_are_not_published_even_when_file_identity_is_unchanged() {
    use std::{
        fs::FileTimes,
        io::{Seek, SeekFrom, Write},
    };
    let (fixture, _native) = source_fixture();
    let (intent, path, original) = ready_artifact(&fixture);
    let mut file = File::options().read(true).write(true).open(&path).unwrap();
    let identity = crate::StorageIdentity::pinned(&file).unwrap();
    let time = file.metadata().unwrap().modified().unwrap();
    file.seek(SeekFrom::Start(128)).unwrap();
    file.write_all(&[original[128] ^ 0x80]).unwrap();
    file.set_times(FileTimes::new().set_modified(time)).unwrap();
    file.sync_all().unwrap();
    assert_eq!(crate::StorageIdentity::pinned(&file).unwrap(), identity);
    let error = recover(&fixture, &intent).unwrap_err();
    assert!(error.to_string().contains("checksum"), "{error}");
    assert!(
        fixture
            .db
            .list::<Catalog>("catalog", 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(path.exists());
    fs::write(&path, &original).unwrap();
    assert!(recover(&fixture, &intent).unwrap());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn a_grown_sealed_database_is_rejected_by_size_before_checksum_work() {
    let (fixture, _native) = source_fixture();
    let (intent, path, original) = ready_artifact(&fixture);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(original.len() as u64 + 4096)
        .unwrap();
    let error = recover(&fixture, &intent).unwrap_err();
    assert!(error.to_string().contains("size differs"), "{error}");
    assert!(
        fixture
            .db
            .list::<Catalog>("catalog", 0, 10)
            .unwrap()
            .is_empty()
    );
    fs::write(path, original).unwrap();
    assert!(recover(&fixture, &intent).unwrap());
}

fn private_rule(fixture: &Fixture) {
    redaction::set_rule(
        &fixture.db,
        &DriveRule {
            id: "late-private".into(),
            drive: fixture.drive.id.clone(),
            relative: "private".into(),
            catalog: false,
            index: false,
        },
    )
    .unwrap();
}
fn redact_private(fixture: &Fixture) {
    let intent = fixture
        .db
        .list::<redaction::Intent>("catalog-redaction", 0, 10)
        .unwrap()
        .pop()
        .unwrap();
    assert!(
        redaction::apply(
            Arc::new(Root::open(&fixture.state.path).unwrap()),
            &intent,
            &AtomicBool::new(false)
        )
        .unwrap()
    );
}
fn sealed_private(fixture: &Fixture) -> Value {
    fs::create_dir(fixture.source.path.join("private")).unwrap();
    fs::write(
        fixture.source.path.join("private/secret"),
        b"source is retained",
    )
    .unwrap();
    let intent = sealed_after_publication_fault(fixture);
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER capture_commit_fault")
        .unwrap();
    intent
}

#[test]
fn recovery_enrolls_late_exclusions_atomically_before_advertising_a_catalog() {
    let (fixture, _native) = source_fixture();
    let intent = sealed_private(&fixture);
    private_rule(&fixture);
    assert!(
        fixture
            .db
            .list::<redaction::Intent>("catalog-redaction", 0, 10)
            .unwrap()
            .is_empty()
    );
    assert!(recover(&fixture, &intent).unwrap());
    let id = intent["id"].as_str().unwrap();
    let catalog: Catalog = fixture.db.get("catalog", id).unwrap().unwrap();
    assert_eq!(catalog.pending_redactions, 1);
    assert!(
        fixture
            .db
            .get::<Value>(staging::KIND, id)
            .unwrap()
            .is_none()
    );
    redact_private(&fixture);
    let catalog: Catalog = fixture.db.get("catalog", id).unwrap().unwrap();
    assert_eq!(catalog.pending_redactions, 0);
    test_assertions::redacted_leaf(
        &fixture.state.path,
        id,
        Path::new("private/secret"),
        &fixture.source.path,
        b"source is retained",
    );
}

#[test]
fn an_old_completed_publication_intent_cannot_undo_a_later_catalog_redaction() {
    let (fixture, _native) = source_fixture();
    let intent = sealed_private(&fixture);
    assert!(recover(&fixture, &intent).unwrap());
    private_rule(&fixture);
    redact_private(&fixture);
    let id = intent["id"].as_str().unwrap();
    let catalog: Catalog = fixture.db.get("catalog", id).unwrap().unwrap();
    let path = fixture
        .state
        .path
        .join("catalogs")
        .join(format!("{id}.sqlite"));
    let redacted = fs::read(&path).unwrap();
    fixture.db.put(staging::KIND, id, &intent).unwrap();
    assert!(recover(&fixture, &intent).unwrap());
    let current: Catalog = fixture.db.get("catalog", id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_vec(&current).unwrap(),
        serde_json::to_vec(&catalog).unwrap()
    );
    assert_eq!(fs::read(path).unwrap(), redacted);
}
