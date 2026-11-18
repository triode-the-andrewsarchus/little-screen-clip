//! Worker settings govern live admission and retain independent capture completions.
use super::*;
use serde_json::Value;

const FILES: usize = 10_000;

#[test]
fn invalid_worker_counts_fail_before_startup_or_reload_changes_preferences() {
    let fixture = TestDrives::new(0);
    for workers in [0, little_clip_core::drives::MAX_CATALOG_WORKERS + 1] {
        let config = DriveConfig {
            catalog_workers: workers,
            ..Default::default()
        };
        assert!(
            Service::start_with_inventory(
                config,
                fixture.temp.path().join("invalid"),
                Arc::new(|| Ok(Vec::new()))
            )
            .is_err()
        );
        assert!(!fixture.temp.path().join("invalid").exists());
    }
    let service = fixture.start_authority(None, fixture.image_authority());
    for workers in [0, usize::MAX] {
        let mut config = status(&service).config;
        config.catalog_workers = workers;
        let operation = completed(&service, DriveRequest::Configure { config });
        assert_eq!(operation.state, "failed");
        assert!(operation.message.contains("Catalog workers"));
        assert_eq!(status(&service).config.catalog_workers, 1);
    }
    service.shutdown().unwrap();
}

#[test]
fn completed_configuration_exposes_new_limits_while_inventory_refresh_is_held() {
    let fixture = TestDrives::new(0);
    let mut held = status_refresh::Held::new(
        &fixture,
        DriveConfig {
            ram_min_available: 0,
            ..Default::default()
        },
    );
    let service = &held.service;
    let mut config = status(service).config;
    config.catalog_workers = 2;
    held.arm();
    let id = accept(service, DriveRequest::Configure { config });
    held.wait();
    let snapshot = status(service);
    assert_eq!(
        snapshot
            .operations
            .iter()
            .find(|op| op.id == id)
            .unwrap()
            .state,
        "complete"
    );
    assert_eq!(snapshot.config.catalog_workers, 2);
    let mut config = snapshot.config;
    config.catalog_workers = 3;
    finish(service, DriveRequest::Configure { config });
    assert_eq!(status(service).config.catalog_workers, 3);
    held.release();
}

struct Captures {
    fixture: TestDrives,
    _media: Option<tempfile::TempDir>,
    service: Service,
    db: Store,
}
impl Captures {
    fn new() -> Self {
        let fixture = TestDrives::new(0);
        for label in ["third", "fourth"] {
            let source = fixture.temp.path().join(label);
            fs::create_dir(&source).unwrap();
            fixture
                .drives
                .lock()
                .unwrap()
                .push(fixture_drive(&source, label));
        }
        let media = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR")
            .map(|base| tempfile::tempdir_in(base).unwrap());
        if let Some(media) = &media {
            for drive in fixture.drives.lock().unwrap().iter_mut() {
                let source = media.path().join(&drive.label);
                fs::create_dir(&source).unwrap();
                drive.mount = Some(source);
            }
        }
        // Enough real metadata work to observe live admission, rather than a
        // synthetic task advertised as a running capture. No payload hashing.
        for drive in fixture.drives.lock().unwrap().iter() {
            let root = drive.mount.as_ref().unwrap();
            for group in 0..FILES / 100 {
                let directory = root.join(format!("group-{group:03}"));
                fs::create_dir(&directory).unwrap();
                for index in 0..100 {
                    // Metadata workload must fit even exFAT's large default
                    // allocation units on the bounded guest volume.
                    File::create(directory.join(format!("entry-{index:03}"))).unwrap();
                }
            }
            eprintln!("CATALOG_WORKER_SOURCE_READY={}", drive.id);
        }
        let service = fixture.start_authority(None, fixture.image_authority());
        let db = Store::existing(&fixture.temp.path().join("state/drives.sqlite"), true).unwrap();
        Self {
            fixture,
            _media: media,
            service,
            db,
        }
    }
    fn start(&self, drive: &str) -> String {
        accept(
            &self.service,
            DriveRequest::Catalog {
                drive: format!("part:{drive}"),
            },
        )
    }
    fn pending(&self) -> Vec<Value> {
        self.db.list(catalog::staging::KIND, 0, 10).unwrap()
    }
    fn wait_pending(&self, count: usize) {
        let started = Instant::now();
        loop {
            let pending = self.pending();
            assert!(pending.len() <= count, "capture admission exceeded {count}");
            if pending.len() == count {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "expected {count} active captures, got {pending:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn workers(&self, workers: usize) {
        let mut config = status(&self.service).config;
        config.catalog_workers = workers;
        finish(&self.service, DriveRequest::Configure { config });
        assert_eq!(status(&self.service).config.catalog_workers, workers);
    }
    fn drain(&self, ids: &[String], max: usize, blocked: Option<&str>) {
        let started = Instant::now();
        loop {
            let pending = self.pending();
            assert!(pending.len() <= max, "too many active captures");
            if !pending.is_empty()
                && let Some(id) = blocked
            {
                let op: DriveOperation = self.db.get("operation", id).unwrap().unwrap();
                assert_eq!(op.state, "queued", "backup ran during active captures");
            }
            let snapshot = status(&self.service);
            let done = ids.iter().all(|id| {
                snapshot
                    .operations
                    .iter()
                    .any(|op| &op.id == id && op.finished.is_some())
            });
            if done {
                for id in ids {
                    let op = snapshot.operations.iter().find(|op| &op.id == id).unwrap();
                    assert_eq!(op.state, "complete", "{}", op.message);
                }
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "captures did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
#[ignore = "explicit owned 40,000-file live scheduler acceptance; supports native guest sources"]
fn live_limits_raise_and_lower_without_cancelling_active_captures_or_overlapping_sources() {
    let captures = Captures::new();
    let first = captures.start("small");
    captures.wait_pending(1);
    let second = captures.start("large");
    // Observe default admission while a real capture owns its database.
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(50) {
        assert_eq!(captures.pending().len(), 1);
        let op = captures
            .db
            .get::<DriveOperation>("operation", &second)
            .unwrap()
            .unwrap();
        assert_eq!(op.state, "queued");
        std::thread::sleep(Duration::from_millis(5));
    }
    captures.workers(2);
    captures.wait_pending(2);
    // Both tasks reached private SQLite allocation. There are two real workers,
    // rather than one running task and an unbounded executor queue.
    captures.workers(1);
    let third = captures.start("small");
    let mut ids = vec![first, second];
    captures.drain(&ids, 2, None);
    ids.push(third);
    captures.drain(&ids, 1, None);
    let saved = status(&captures.service).catalogs;
    assert_eq!(saved.len(), 3);
    assert!(
        saved
            .iter()
            .all(|catalog| catalog.complete && catalog.entries >= FILES as u64)
    );
    assert!(captures.pending().is_empty());

    // Raising to the supported maximum must still serialize the same physical
    // source reached through distinct enrolled drive identifiers.
    captures.workers(little_clip_core::drives::MAX_CATALOG_WORKERS);
    let mut alias = captures.fixture.drives.lock().unwrap()[0].clone();
    alias.id = "part:alias".into();
    alias.part_uuid = Some("alias".into());
    alias.evidence = "owned alias".into();
    captures.fixture.drives.lock().unwrap().push(alias);
    let first = captures.start("small");
    captures.wait_pending(1);
    let alias = captures.start("alias");
    let repeated = captures.start("small");
    captures.drain(&[first, alias, repeated], 1, None);
    assert_eq!(status(&captures.service).catalogs.len(), 6);
    assert!(captures.pending().is_empty());
    let ids = ["small", "large", "third", "fourth"].map(|drive| captures.start(drive));
    captures.wait_pending(little_clip_core::drives::MAX_CATALOG_WORKERS);
    let backup_source = captures.fixture.temp.path().join("backup-source");
    fs::create_dir(&backup_source).unwrap();
    fs::write(backup_source.join("payload"), b"exclusive backup").unwrap();
    captures
        .fixture
        .drives
        .lock()
        .unwrap()
        .push(fixture_drive(&backup_source, "backup-source"));
    let backup = accept(
        &captures.service,
        DriveRequest::Backup {
            drive: "part:backup-source".into(),
            kind: BackupKind::Files,
            restore_mounted: false,
        },
    );
    captures.drain(
        &ids,
        little_clip_core::drives::MAX_CATALOG_WORKERS,
        Some(&backup),
    );
    let operation = wait_operation(&captures.service, &backup, |op| op.finished.is_some());
    assert_eq!(operation.state, "complete", "{}", operation.message);
    let version = status(&captures.service).backups.pop().unwrap();
    assert_eq!(
        fs::read(version.path.join("payload")).unwrap(),
        b"exclusive backup"
    );
    let saved = status(&captures.service).catalogs;
    assert_eq!(saved.len(), 10);
    assert!(
        saved
            .iter()
            .all(|catalog| catalog.complete && catalog.entries >= FILES as u64)
    );
    assert!(captures.pending().is_empty());
    captures.service.shutdown().unwrap();
}

#[test]
#[ignore = "explicit owned 40,000-file live shutdown acceptance; supports native guest sources"]
fn shutdown_retains_all_concurrent_capture_results_and_recovers_without_source_media() {
    let captures = Captures::new();
    captures.workers(2);
    let ids = [captures.start("small"), captures.start("large")];
    captures.wait_pending(2);
    captures.service.shutdown().unwrap();
    for id in ids {
        let op: DriveOperation = captures.db.get("operation", &id).unwrap().unwrap();
        assert!(op.finished.is_some());
        assert!(
            matches!(op.state.as_str(), "complete" | "cancelled"),
            "{}",
            op.message
        );
    }
    assert!(captures.pending().is_empty());
    let restarted = Service::start_with_inventory(
        DriveConfig::default(),
        captures.fixture.temp.path().join("state"),
        Arc::new(|| Ok(Vec::new())),
    )
    .unwrap();
    assert!(
        status(&restarted)
            .operations
            .iter()
            .all(|op| op.finished.is_some())
    );
    assert!(captures.pending().is_empty());
    restarted.shutdown().unwrap();
}
