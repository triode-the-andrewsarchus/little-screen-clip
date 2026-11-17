use super::*;

#[test]
fn excluded_catalog_and_index_policy_keeps_measurable_sync_baselines_without_media() {
    let fixture = TestDrives::new(0);
    let source = fixture.temp.path().join("small");
    fs::write(source.join("edit"), b"sync includes excluded content").unwrap();
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: PathBuf::new(),
            catalog: false,
            index: false,
        },
    );
    let local = fixture.pair(&service, "small", "");
    assert_eq!(
        fs::read(local.join("edit")).unwrap(),
        b"sync includes excluded content"
    );
    fixture.drives.lock().unwrap().clear();
    fs::rename(&source, fixture.temp.path().join("removed-small")).unwrap();
    let operation = completed(&service, DriveRequest::MeasureStorage);
    assert_eq!(operation.state, "complete", "{}", operation.message);
    assert!(
        operation.message.contains("Sync baselines: 1 paths"),
        "{}",
        operation.message
    );
    assert!(
        operation
            .message
            .contains("Saved catalogs (optional): 0 snapshots"),
        "{}",
        operation.message
    );
    let DriveReply::Rule(Some(rule)) = service
        .request(DriveRequest::GetRule {
            drive: "part:small".into(),
            relative: PathBuf::new(),
        })
        .unwrap()
    else {
        panic!("Missing exclusion rule")
    };
    assert!(!rule.catalog && !rule.index);
    service.shutdown().unwrap();
    let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
    let retained: DriveOperation = db.get("operation", &operation.id).unwrap().unwrap();
    assert_eq!(retained.message, operation.message);
    assert_eq!(retained.finished, operation.finished);
    let count: u64 =
        db.0.query_row("SELECT count(*) FROM baseline", [], |row| row.get(0))
            .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn long_metadata_measurement_keeps_controls_responsive_and_cancels_inside_sql() {
    let fixture = TestDrives::new(0);
    fixture.drives.lock().unwrap().clear();
    let service = fixture.start_authority(None, fixture.image_authority());
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    // A streamed recursive view makes a genuinely long read without a huge
    // database or payload tree. Ordinary status never evaluates this view.
    db.0.execute_batch("DROP TABLE baseline;
        CREATE VIEW baseline AS WITH RECURSIVE ticks(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM ticks WHERE n<100000000)
        SELECT 'pair' pair,x'61' path,'fingerprint' hash FROM ticks;").unwrap();
    let id = accept(&service, DriveRequest::MeasureStorage);
    wait_operation(&service, &id, |operation| operation.state == "running");
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    finish(
        &service,
        DriveRequest::Configure {
            config: DriveConfig {
                catalog_workers: 2,
                ..Default::default()
            },
        },
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(
        status(&service)
            .operations
            .iter()
            .find(|operation| operation.id == id)
            .unwrap()
            .finished
            .is_none()
    );
    service
        .request(DriveRequest::Cancel {
            operation: id.clone(),
        })
        .unwrap();
    let cancelled = wait_operation(&service, &id, |operation| operation.finished.is_some());
    assert_eq!(cancelled.state, "cancelled", "{}", cancelled.message);
    assert!(!cancelled.message.contains("Sync baselines:"));
    db.0.execute_batch("DROP VIEW baseline; CREATE TABLE baseline(pair TEXT NOT NULL,path BLOB NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(pair,path));").unwrap();
    let retry = completed(&service, DriveRequest::MeasureStorage);
    assert_eq!(retry.state, "complete", "{}", retry.message);
    assert!(retry.message.contains("Sync baselines: 0 paths"));
    service.shutdown().unwrap();
}
