//! Public jobs retain confirmed effects independently from scan progress and retry.
use super::*;

#[test]
fn failed_public_sync_keeps_receipts_and_retry_confirms_only_new_writes() {
    let fixture = TestDrives::new(0);
    let _native = fixture.guest_source();
    let source = fixture.drives.lock().unwrap()[1]
        .mount
        .as_ref()
        .unwrap()
        .join("selected");
    fs::create_dir(&source).unwrap();
    for name in ["a", "b", "c"] {
        fs::write(source.join(name), b"receipt payload").unwrap();
    }
    let local = fixture.temp.path().join("receipt-local");
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    db.0.execute_batch(
        "CREATE TRIGGER receipt_failure BEFORE INSERT ON sync_receipts
        WHEN NEW.path=x'62' BEGIN SELECT RAISE(FAIL,'controlled receipt failure'); END;",
    )
    .unwrap();
    let failed = failed(
        &service,
        DriveRequest::Pair {
            drive: "part:large".into(),
            relative: "selected".into(),
            local: local.clone(),
        },
        "controlled receipt failure",
    );
    let expected = crate::SyncCompletion {
        installed: 1,
        removed: 0,
        bytes: 15,
    };
    assert_eq!(failed.sync_completion, Some(expected.clone()));
    let pair = status(&service).pairs.pop().unwrap();
    assert!(pair.paused && pair.last_success.is_none());
    assert_eq!(fs::read(local.join("a")).unwrap(), b"receipt payload");
    assert_eq!(fs::read(local.join("b")).unwrap(), b"receipt payload");
    assert!(!local.join("c").exists());
    let installed = File::open(local.join("a")).unwrap();
    let revision = little_clip_core::file_source::FileSource::capture_file(&installed).unwrap();
    service.shutdown().unwrap();
    db.0.execute_batch("DROP TRIGGER receipt_failure").unwrap();
    let restarted = fixture.start_observed_reads(Arc::new(|| {}));
    let original = status(&restarted)
        .operations
        .into_iter()
        .find(|op| op.id == failed.id)
        .unwrap();
    assert_eq!(original.state, "failed");
    assert_eq!(original.sync_completion, Some(expected.clone()));
    let retried = finish(&restarted, DriveRequest::Sync { pair: pair.id });
    assert_eq!(retried.sync_completion, Some(expected));
    revision.check_file(&installed).unwrap();
    assert_eq!(fs::read(local.join("c")).unwrap(), b"receipt payload");
    for (id, path) in [(&failed.id, b"a"), (&retried.id, b"c")] {
        let paths: Vec<Vec<u8>> =
            db.0.prepare("SELECT path FROM sync_receipts WHERE operation=?1 ORDER BY id")
                .unwrap()
                .query_map([id], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
        assert_eq!(paths, vec![path.to_vec()]);
    }
    restarted.shutdown().unwrap();
}
