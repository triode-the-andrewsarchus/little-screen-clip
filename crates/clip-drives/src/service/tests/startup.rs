//! Initial discovery and RAM recovery leave local controls available.
use super::*;

#[test]
fn initial_discovery_keeps_controls_sync_and_shutdown_responsive() {
    for outcome in ["blocked", "published", "shutdown"] {
        let fixture = TestDrives::new(0);
        let (entered, release, observe) =
            worker_observer("drive-status-", Arc::new(AtomicBool::new(true)));
        let (pinning, resume, observe_sync) = worker_observer(
            "drive-sync-",
            Arc::new(AtomicBool::new(outcome == "published")),
        );
        let drives = fixture.drives.clone();
        let started = Instant::now();
        let service = Service::start_with_authority(
            DriveConfig {
                ram_min_available: u64::MAX,
                sync_interval_seconds: 3600,
                ..Default::default()
            },
            fixture.temp.path().join("state"),
            Arc::new(move || {
                observe();
                observe_sync();
                Ok(drives.lock().unwrap().clone())
            }),
            None,
            Arc::new(fixture.image_authority()),
            Default::default(),
        )
        .unwrap();
        let mut closing = None;
        let wait_discovery = || {
            let started = Instant::now();
            while status(&service).discovery_error.as_deref() == Some(refresh::INITIAL_DISCOVERY) {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(status(&service).drives.len(), 2);
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert!(started.elapsed() < Duration::from_secs(2));
            assert_eq!(
                status(&service).discovery_error.as_deref(),
                Some(refresh::INITIAL_DISCOVERY)
            );
            assert!(status(&service).drives.is_empty());
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_configuration_responsive(&service);
            let local = fixture.temp.path().join("local");
            let operation = accept(
                &service,
                DriveRequest::Pair {
                    drive: "part:small".into(),
                    relative: PathBuf::new(),
                    local: local.clone(),
                },
            );
            if outcome == "published" {
                pinning.recv_timeout(Duration::from_secs(5)).unwrap();
                release.send(()).unwrap();
                wait_discovery();
                resume.send(()).unwrap();
            }
            let completed = wait_operation(&service, &operation, |op| op.finished.is_some());
            assert_eq!(completed.state, "complete", "{}", completed.message);
            fs::write(local.join("independent"), b"sync during initial discovery").unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during initial discovery",
            );
            if outcome == "shutdown" {
                closing = Some(shutdown_while_held(&service));
            }
        }));
        let _ = release.send(());
        let _ = resume.send(());
        if let Some(closing) = closing {
            finish_held_shutdown(closing);
        }
        if result.is_ok() && outcome != "shutdown" {
            wait_discovery();
        }
        service.shutdown().unwrap();
        result.unwrap();
    }
}

#[test]
fn unpublished_allocation_paths_stay_protected_during_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let state = Root::create(&temp.path().join("state")).unwrap();
    let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
    let private = Root::create(&temp.path().join("unpublished")).unwrap();
    let ordinary = Root::create(&temp.path().join("ordinary")).unwrap();
    let identity = private.storage_identity().unwrap();
    let backing = RamBacking {
        directory: private.path.clone(),
        directory_identity: identity.clone(),
        image_identity: identity,
        capacity: 4096,
        inodes: 128,
        mounted: false,
        initialized: false,
    };
    db.put(
        "ram-allocation",
        "owned",
        &serde_json::json!({
            "session": "owned", "operation": "preparing", "backing": backing,
        }),
    )
    .unwrap();
    let storage = policy::Storage::capture(&db, &state.path, None).unwrap();
    assert!(
        storage
            .validate_source(&private)
            .unwrap_err()
            .to_string()
            .contains("private RAM backing")
    );
    assert!(
        policy::validate_backup_storage(&db, &state.path, &private)
            .unwrap_err()
            .to_string()
            .contains("private RAM backing")
    );
    storage.validate_source(&ordinary).unwrap();
    db.remove("ram-allocation", "owned").unwrap();
    policy::Storage::capture(&db, &state.path, None)
        .unwrap()
        .validate_source(&private)
        .unwrap();
}
