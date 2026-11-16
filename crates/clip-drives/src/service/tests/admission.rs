//! Retry identities never replay writes or consume another backup selection.
use super::*;

fn submitted(service: &Service, id: uuid::Uuid, request: DriveRequest) -> String {
    match service.submit(id, request).unwrap() {
        DriveReply::Accepted(operation) => operation,
        _ => panic!("expected a retained operation"),
    }
}

#[test]
fn concurrent_retries_of_a_consumed_review_share_the_pending_and_terminal_operation() {
    let fixture = TestDrives::new(0);
    let (entered, release, gate) = worker_gate();
    let mut authority = fixture.image_authority();
    let unmount = authority.unmount.clone();
    authority.unmount = Arc::new(move |drive| {
        gate();
        unmount(drive)
    });
    let service = fixture.start_authority(None, authority);
    let destination = backup_review::destination(&fixture);
    let review = backup_review::preview(&service, BackupKind::Image, &destination);
    let request = backup_review::start(&review, false);
    let key = uuid::Uuid::new_v4();
    std::thread::scope(|scope| {
        let retries = (0..12)
            .map(|_| scope.spawn(|| submitted(&service, key, request.clone())))
            .collect::<Vec<_>>();
        for retry in retries {
            assert_eq!(retry.join().unwrap(), key.to_string());
        }
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(status(&service).operations.len(), 1);
    assert!(!service.previews.lock().unwrap().contains_key(&review.token));
    assert!(
        service
            .submit(key, backup_review::start(&review, true))
            .unwrap_err()
            .to_string()
            .contains("different parameters")
    );
    release.send(()).unwrap();
    let failed = wait_operation(&service, &key.to_string(), |op| op.finished.is_some());
    assert_eq!(failed.state, "failed");
    assert!(
        failed
            .message
            .contains("owned image authorization declined")
    );
    assert_eq!(submitted(&service, key, request), failed.id);
    assert_eq!(status(&service).operations.len(), 1);
    service.shutdown().unwrap();
}

#[test]
fn completed_backup_retries_survive_restart_without_needing_the_old_review() {
    let (fixture, service, _destination, review) = backup_review::reviewed_files();
    let request = backup_review::start(&review, false);
    let key = uuid::Uuid::new_v4();
    let id = submitted(&service, key, request.clone());
    let complete = wait_operation(&service, &id, |op| op.finished.is_some());
    assert_eq!(complete.state, "complete", "{}", complete.message);
    let version = status(&service).backups.pop().unwrap();
    service.shutdown().unwrap();
    let restarted = fixture.start_authority(None, fixture.image_authority());
    assert_eq!(submitted(&restarted, key, request), id);
    let state = status(&restarted);
    assert_eq!(state.operations.len(), 1);
    assert_eq!(state.backups.len(), 1);
    assert_eq!(state.backups[0].id, version.id);
    assert_eq!(state.operations[0].finished, complete.finished);
    restarted.shutdown().unwrap();
}

#[test]
fn rejected_queue_and_atomic_database_failure_restore_the_review_and_allow_retry() {
    for database_failure in [false, true] {
        let (fixture, service, destination, review) = backup_review::reviewed_files();
        let key = uuid::Uuid::new_v4();
        let request = backup_review::start(&review, false);
        let db = Store::existing(&fixture.temp.path().join("state/drives.sqlite"), false).unwrap();
        let mut rejected = service.clone();
        let (sender, receiver) = mpsc::sync_channel(0);
        if database_failure {
            db.0.execute_batch("CREATE TRIGGER reject_owned_submission BEFORE INSERT ON entities WHEN NEW.kind='submission' BEGIN SELECT RAISE(ABORT, 'owned admission failure'); END").unwrap();
        } else {
            rejected.sender = sender;
        }
        assert!(rejected.submit(key, request.clone()).is_err());
        assert!(
            db.get::<DriveOperation>("operation", &key.to_string())
                .unwrap()
                .is_none()
        );
        let bindings: i64 =
            db.0.query_row(
                "SELECT count(*) FROM entities WHERE kind='submission'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bindings, 0);
        assert!(service.previews.lock().unwrap().contains_key(&review.token));
        assert!(fs::read_dir(destination).unwrap().next().is_none());
        if database_failure {
            db.0.execute_batch("DROP TRIGGER reject_owned_submission")
                .unwrap();
        }
        let id = submitted(&service, key, request);
        let complete = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(complete.state, "complete", "{}", complete.message);
        assert_eq!(status(&service).backups.len(), 1);
        drop(receiver);
        service.shutdown().unwrap();
    }
}

#[test]
fn lost_operation_ownership_is_interrupted_on_retry_before_and_after_daemon_restart() {
    for restart in [false, true] {
        let fixture = TestDrives::new(0);
        let service = fixture.start_authority(None, fixture.image_authority());
        let destination = fixture.temp.path().join("must-not-be-created");
        let request = DriveRequest::BackupDirectory {
            path: destination.clone(),
        };
        let key = uuid::Uuid::new_v4();
        let mut queued = service.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        queued.sender = sender;
        let id = submitted(&queued, key, request.clone());
        // Durable records survive, but the envelope never reaches its owner.
        drop(receiver);
        let caller = if restart {
            service.shutdown().unwrap();
            fixture.start_authority(None, fixture.image_authority())
        } else {
            service.runtime.tokens.lock().unwrap().remove(&id);
            service.clone()
        };
        assert_eq!(submitted(&caller, key, request), id);
        let operation = status(&caller).operations.pop().unwrap();
        assert_eq!(operation.state, "interrupted");
        assert!(operation.finished.is_some());
        assert!(operation.message.contains(if restart {
            "Daemon restarted"
        } else {
            "no live owner"
        }));
        assert!(!destination.exists());
        caller.shutdown().unwrap();
    }
}

#[test]
fn failed_queue_cleanup_keeps_a_terminal_result_and_never_replays_the_write() {
    let (fixture, service, destination, review) = backup_review::reviewed_files();
    let db = Store::existing(&fixture.temp.path().join("state/drives.sqlite"), false).unwrap();
    db.0.execute_batch("CREATE TRIGGER reject_owned_cleanup BEFORE DELETE ON entities WHEN OLD.kind='submission' BEGIN SELECT RAISE(ABORT, 'owned cleanup failure'); END").unwrap();
    let mut rejected = service.clone();
    let (sender, _receiver) = mpsc::sync_channel(0);
    rejected.sender = sender;
    let key = uuid::Uuid::new_v4();
    let request = backup_review::start(&review, false);
    let error = rejected.submit(key, request.clone()).unwrap_err();
    assert!(
        error.to_string().contains("failed result is retained"),
        "{error}"
    );
    let id = submitted(&service, key, request);
    let failure = status(&service).operations.pop().unwrap();
    assert_eq!(failure.id, id);
    assert_eq!(failure.state, "failed");
    assert!(failure.finished.is_some());
    assert!(failure.message.contains("before execution"));
    assert!(status(&service).backups.is_empty());
    assert!(fs::read_dir(destination).unwrap().next().is_none());
    assert!(service.previews.lock().unwrap().contains_key(&review.token));
    service.shutdown().unwrap();
}

#[test]
fn identity_submission_rejects_controls_and_cannot_take_over_a_legacy_operation() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_authority(None, fixture.image_authority());
    assert!(
        service
            .submit(uuid::Uuid::new_v4(), DriveRequest::Status { offset: 0 })
            .is_err()
    );
    let request = DriveRequest::BackupDirectory {
        path: fixture.temp.path().join("legacy"),
    };
    let legacy = finish(&service, request.clone());
    // Model an operation written before request bindings were introduced.
    let db = Store::existing(&fixture.temp.path().join("state/drives.sqlite"), false).unwrap();
    db.remove("submission", &legacy.id).unwrap();
    assert!(
        service
            .submit(uuid::Uuid::parse_str(&legacy.id).unwrap(), request)
            .unwrap_err()
            .to_string()
            .contains("without its request binding")
    );
    assert_eq!(status(&service).operations.len(), 1);
    service.shutdown().unwrap();
}
