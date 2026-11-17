//! Real retirement work must not own the coordinator's event loop.
use super::*;
use serde_json::Value;
use std::os::unix::ffi::OsStrExt;

struct Fixture {
    drives: TestDrives,
    _media: Option<tempfile::TempDir>,
    db: Store,
    old: DriveBackup,
    current: DriveBackup,
    legacy: PathBuf,
}
impl Fixture {
    fn new(files: usize) -> Self {
        let drives = TestDrives::new(0);
        let media = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").map(|base| {
            crate::require_disposable_guest();
            let media = tempfile::tempdir_in(base).unwrap();
            let mut selected = drives.drives.lock().unwrap();
            selected[1].mount = Some(media.path().into());
            selected[1].filesystem = std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
            media
        });
        let drive = drives.drives.lock().unwrap()[1].clone();
        let source = Root::open(drive.mount.as_ref().unwrap()).unwrap();
        for index in 0..files {
            let directory = source.path.join(format!("group-{:03}", index / 100));
            fs::create_dir_all(&directory).unwrap();
            File::create(directory.join(format!("entry-{index:05}"))).unwrap();
        }
        let state = Root::create(&drives.temp.path().join("state")).unwrap();
        let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
        let legacy = Root::create(
            &drives
                .temp
                .path()
                .join(std::ffi::OsStr::from_bytes(b"legacy-backups-\xff\\\n")),
        )
        .unwrap();
        let old = backup::files(
            &db,
            &drive,
            &source,
            &legacy,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        for index in 0..files.div_ceil(100) {
            fs::remove_dir_all(source.path.join(format!("group-{index:03}"))).unwrap();
        }
        fs::write(source.path.join("current"), b"keep this current version").unwrap();
        let target = Root::create(&drives.temp.path().join("current-backups")).unwrap();
        let current = backup::files(
            &db,
            &drive,
            &source,
            &target,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        // Model releases that predate persistent destination enrollment.
        db.0.execute("DELETE FROM entities WHERE kind='backup-root'", [])
            .unwrap();
        // Retention and recovery must also work with the source disconnected.
        drives.drives.lock().unwrap().retain(|d| d.id != drive.id);
        Self {
            drives,
            _media: media,
            db,
            old,
            current,
            legacy: legacy.path,
        }
    }

    fn start(&self) -> Service {
        self.drives.start_configured(
            DriveConfig {
                backup_keep_versions: Some(1),
                sync_interval_seconds: 3600,
                ..Default::default()
            },
            None,
            self.drives.image_authority(),
        )
    }

    fn retire(&self, paused: bool) {
        let parent = Root::open(self.old.path.parent().unwrap()).unwrap();
        let transaction = self.db.0.unchecked_transaction().unwrap();
        self.db
            .put(
                "backup-prune",
                &self.old.id,
                &serde_json::json!({
                    "version": 1, "backup": &self.old, "parent": parent.storage_identity().unwrap(),
                    "quarantined": false, "paused": paused,
                }),
            )
            .unwrap();
        self.db.remove("backup", &self.old.id).unwrap();
        transaction.commit().unwrap();
    }

    fn wait_quarantine(&self) {
        let started = Instant::now();
        loop {
            let intent = self.db.get::<Value>("backup-prune", &self.old.id).unwrap();
            if intent.as_ref().is_some_and(|v| v["quarantined"] == true) {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "No live retirement reached quarantine: {intent:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_live(&self) {
        assert!(
            self.db
                .get::<Value>("backup-prune", &self.old.id)
                .unwrap()
                .is_some(),
            "The fixture must still be doing real cleanup while controls complete"
        );
        assert!(!self.old.path.exists());
        assert!(
            self.db
                .get::<Value>("recovery", &self.old.id)
                .unwrap()
                .is_none(),
            "A failed retained intent is not live work"
        );
        // The manifest is checked and retired after the payload tree. Either
        // remaining artifact proves more than a tiny final journal-update window.
        assert!(
            self.old
                .path
                .parent()
                .unwrap()
                .join(format!(".pruning-{}", self.old.id))
                .exists()
                || self.old.manifest.as_ref().unwrap().exists()
        );
    }

    fn independent_controls(&self, service: &Service, pair: &str, local: &Path) {
        let mut config = status(service).config;
        config.catalog_workers = 2;
        finish(service, DriveRequest::Configure { config });
        self.assert_live();
        fs::write(
            self.drives.temp.path().join("small/edit"),
            b"sync during retirement",
        )
        .unwrap();
        finish(service, DriveRequest::Sync { pair: pair.into() });
        assert_eq!(
            fs::read(local.join("edit")).unwrap(),
            b"sync during retirement"
        );
        self.assert_live();
    }

    fn pair(&self, service: &Service) -> (String, PathBuf) {
        let local = self.drives.pair(service, "small", "");
        let pair = status(service)
            .pairs
            .into_iter()
            .find(|p| p.drive == "part:small")
            .unwrap();
        (pair.id, local)
    }

    fn assert_legacy_policy(&self, service: &Service) {
        assert!(published_policy(service).excluded.contains(&self.legacy));
        failed(
            service,
            DriveRequest::Pair {
                drive: "part:small".into(),
                relative: PathBuf::new(),
                local: self.legacy.clone(),
            },
            "managed",
        );
    }
}

#[test]
fn paused_legacy_retirement_keeps_its_root_excluded_and_rejects_sync_enrollment() {
    let fixture = Fixture::new(1);
    fixture.retire(true);
    let service = fixture.start();
    fixture.assert_legacy_policy(&service);
    assert!(
        fixture
            .db
            .get::<Value>("backup-prune", &fixture.old.id)
            .unwrap()
            .unwrap()["paused"]
            == true
    );
    assert!(fixture.old.path.exists());
    service.shutdown().unwrap();
}

#[test]
fn legacy_retirement_preserves_its_raw_destination_policy_atomically_after_restart() {
    let fixture = Fixture::new(1);
    fixture.retire(false);
    fixture.db.0.execute_batch("CREATE TRIGGER fail_root BEFORE INSERT ON entities WHEN NEW.kind='backup-root' BEGIN SELECT RAISE(FAIL,'root enrollment fault'); END").unwrap();
    assert!(
        backup::recover_record(
            &fixture.db,
            "backup-prune",
            &fixture.old.id,
            &AtomicBool::new(false),
        )
        .is_err()
    );
    assert!(!fixture.old.path.exists());
    assert!(!fixture.old.manifest.as_ref().unwrap().exists());
    assert!(
        fixture
            .db
            .get::<Value>("backup-prune", &fixture.old.id)
            .unwrap()
            .is_some()
    );
    assert!(
        preflight::managed_backup_paths(&fixture.db)
            .unwrap()
            .contains(&fixture.legacy)
    );
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER fail_root")
        .unwrap();
    assert!(
        backup::recover_record(
            &fixture.db,
            "backup-prune",
            &fixture.old.id,
            &AtomicBool::new(false),
        )
        .unwrap()
    );
    assert!(
        fixture
            .db
            .get::<Value>("backup-prune", &fixture.old.id)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .db
            .get::<Value>("backup", &fixture.old.id)
            .unwrap()
            .is_none()
    );
    // The old bucket still contains its reference, although its last version is gone.
    assert!(
        fixture
            .old
            .path
            .parent()
            .unwrap()
            .join("current-files.json")
            .exists()
    );
    for _ in 0..2 {
        let service = fixture.start();
        assert!(status(&service).backup_directory.is_none());
        fixture.assert_legacy_policy(&service);
        service.shutdown().unwrap();
    }
}

#[test]
#[ignore = "creates 10000 owned files to verify live retirement responsiveness"]
fn real_prune_keeps_controls_and_an_independent_pair_live_until_cleanup_completes() {
    let fixture = Fixture::new(10_000);
    let service = fixture.start();
    let (pair, local) = fixture.pair(&service);
    let id = accept(
        &service,
        DriveRequest::PruneBackups {
            drive: fixture.old.drive.clone(),
        },
    );
    fixture.wait_quarantine();
    fixture.independent_controls(&service, &pair, &local);
    service
        .request(DriveRequest::Cancel {
            operation: id.clone(),
        })
        .unwrap();
    let operation = wait_operation(&service, &id, |operation| operation.finished.is_some());
    assert_eq!(operation.state, "cancelled", "{}", operation.message);
    assert_eq!(
        fixture
            .db
            .get::<Value>("backup-prune", &fixture.old.id)
            .unwrap()
            .unwrap()["paused"],
        true
    );
    let retry = completed(
        &service,
        DriveRequest::PruneBackups {
            drive: fixture.old.drive.clone(),
        },
    );
    assert_eq!(retry.state, "complete", "{}", retry.message);
    assert!(retry.message.contains("Pruned 1"));
    assert_eq!(
        fs::read(fixture.current.path.join("current")).unwrap(),
        b"keep this current version"
    );
    service.shutdown().unwrap();
}

#[test]
#[ignore = "creates 10000 owned files to verify live startup recovery responsiveness"]
fn real_startup_retirement_recovery_runs_while_controls_and_an_independent_pair_progress() {
    let fixture = Fixture::new(10_000);
    fixture.retire(false);
    let service = fixture.start();
    fixture.wait_quarantine();
    let (pair, local) = fixture.pair(&service);
    fixture.assert_live();
    fixture.independent_controls(&service, &pair, &local);
    service.shutdown().unwrap();
    assert_eq!(
        fixture
            .db
            .get::<Value>("backup-prune", &fixture.old.id)
            .unwrap()
            .unwrap()["paused"],
        false
    );
    let restarted = fixture.start();
    let started = Instant::now();
    while fixture
        .db
        .get::<Value>("backup-prune", &fixture.old.id)
        .unwrap()
        .is_some()
    {
        assert!(started.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read(fixture.current.path.join("current")).unwrap(),
        b"keep this current version"
    );
    restarted.shutdown().unwrap();
}
