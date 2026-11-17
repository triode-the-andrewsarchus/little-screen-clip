//! Deterministic interleavings use the real worker's existing per-file progress hook.
use super::*;
use crate::service::tests::TestDrives;
use std::{fs::FileTimes, os::unix::fs::MetadataExt};

struct Population {
    db: Store,
    plan: Plan,
    intent: Intent,
    backing: RamBacking,
    mounted: Option<crate::ram::Mounted>,
    _drives: TestDrives,
    _media: Option<tempfile::TempDir>,
    _temp: tempfile::TempDir,
}
impl Population {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let media = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").map(|path| {
            crate::require_disposable_guest();
            assert_eq!(Path::new(&path), Path::new("/work/media"));
            tempfile::tempdir_in(path).unwrap()
        });
        let portable = Arc::new(
            Root::create(
                media
                    .as_ref()
                    .map_or_else(|| temp.path().join("source"), |media| media.path().into())
                    .as_path(),
            )
            .unwrap(),
        );
        fs::write(portable.path.join("a"), b"source-before").unwrap();
        fs::write(portable.path.join("b"), b"second-original").unwrap();
        let drives = TestDrives::new(0);
        let (mut backing, id, mounted, path) = drives.native_source("");
        fs::remove_file(path.join("edit")).unwrap();
        backing.mounted = true;
        backing.initialized = true;
        let local = Arc::new(Root::open(&path).unwrap());
        let pair = SyncPair {
            id: id.clone(),
            drive: "population-fixture".into(),
            evidence: "owned".into(),
            relative: PathBuf::new(),
            local: path.clone(),
            local_identity: Some(local.storage_identity().unwrap()),
            paused: true,
            deletions: false,
            ram: true,
            last_success: None,
            status: "Initial RAM copy incomplete".into(),
        };
        let session = RamSession {
            id: id.clone(),
            drive: pair.drive.clone(),
            path: path.clone(),
            identity: kernel_interface::MountIdentity::directory(&path).unwrap(),
            ready: false,
            connected: true,
            status: pair.status.clone(),
            backing: Some(backing.clone()),
            last_saved: None,
            save_status: None,
            last_flushed: None,
            pending: None,
        };
        let db = Store::open(&temp.path().join("population.sqlite")).unwrap();
        db.put("pair", &id, &pair).unwrap();
        db.put("ram", &id, &session).unwrap();
        {
            let transaction = db.0.unchecked_transaction().unwrap();
            super::super::ram_pending::record(&db, &id, std::iter::empty(), true).unwrap();
            transaction.commit().unwrap();
        }
        let plan = Plan {
            pair,
            portable,
            local,
            cached: false,
            observed: false,
        };
        let mut intent = Intent {
            queued: Instant::now(),
            pair: id,
            operation: Some("owned-population".into()),
            cancel: Arc::new(AtomicBool::new(false)),
            paths: None,
            initialize: true,
            save: None,
            writeback: None,
        };
        intent.writeback = plan.writeback_checkpoint(&db, &intent).unwrap();
        Self {
            db,
            plan,
            intent,
            backing,
            mounted: Some(mounted),
            _drives: drives,
            _media: media,
            _temp: temp,
        }
    }
    fn session(&self) -> RamSession {
        self.db.get("ram", &self.intent.pair).unwrap().unwrap()
    }
    fn run(&self, progress: &mut impl FnMut(u64, u64)) -> Result<String> {
        self.plan
            .run_with_cancel(&self.db, &self.intent, &self.intent.cancel, progress)
    }
    fn after_first_install(&self, edit: impl FnOnce(&Self)) -> Result<String> {
        let mut edit = Some(edit);
        let result = self.run(&mut |_, _| {
            if self.plan.local.path.join("a").exists()
                && let Some(edit) = edit.take()
            {
                assert!(!self.session().browsable());
                assert!(!self.plan.local.path.join("b").exists());
                assert_eq!(
                    fs::read(self.plan.local.path.join("a")).unwrap(),
                    b"source-before"
                );
                edit(self);
            }
        });
        assert!(
            edit.is_none(),
            "Progress must reach the actual first installation"
        );
        result
    }
    fn assert_converged(&self, names: &[&str]) {
        for name in names {
            assert_eq!(
                fs::read(self.plan.local.path.join(name)).unwrap(),
                fs::read(self.plan.portable.path.join(name)).unwrap()
            );
        }
    }
    fn publish_complete(&self, result: &Result<String>) {
        assert!(result.is_ok(), "{result:?}");
        persist_result(&self.db, &self.intent, result).unwrap();
        assert!(self.session().browsable());
        assert!(self.session().last_flushed.is_some());
        assert!(
            !super::super::ram_pending::snapshot(&self.db, &self.intent.pair)
                .unwrap()
                .full_check_required
        );
        let pair: SyncPair = self.db.get("pair", &self.intent.pair).unwrap().unwrap();
        assert!(!pair.paused && pair.last_success.is_some());
    }
    fn changed_source(&self, name: &str, bytes: &[u8]) {
        let path = self.plan.portable.path.join(name);
        let original = fs::metadata(&path).unwrap();
        fs::write(&path, bytes).unwrap();
        File::open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        let changed = fs::metadata(path).unwrap();
        assert_eq!(changed.len(), original.len());
        assert_eq!(
            (changed.mtime(), changed.mtime_nsec()),
            (original.mtime(), original.mtime_nsec())
        );
    }
}
impl Drop for Population {
    fn drop(&mut self) {
        drop(self.mounted.take());
        if let Err(error) = crate::ram::discard(&self.backing, &self.intent.pair) {
            if !std::thread::panicking() {
                panic!("Population fixture cleanup failed: {error}");
            }
            eprintln!("Population fixture retained after failed cleanup: {error}");
        }
    }
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; optionally uses owned guest media"]
fn initial_population_catches_preserved_timestamp_edits_and_new_paths_before_ready() {
    for initialize in [false, true] {
        let mut fixture = Population::new();
        fixture.intent.initialize = initialize;
        let result = fixture.after_first_install(|fixture| {
            fixture.changed_source("a", b"source-after!");
            fs::write(
                fixture.plan.portable.path.join("c"),
                b"new during population",
            )
            .unwrap();
        });
        assert!(result.is_ok(), "{result:?}");
        assert!(
            !fixture.session().browsable(),
            "Only result publication can enable browsing"
        );
        if initialize {
            fixture.assert_converged(&["a", "b", "c"]);
            fixture.publish_complete(&result);
        } else {
            // A one-pass control must miss these changes, proving that the
            // successful initialized case actually depends on catch-up.
            assert_eq!(
                fs::read(fixture.plan.local.path.join("a")).unwrap(),
                b"source-before"
            );
            assert!(!fixture.plan.local.path.join("c").exists());
        }
    }
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; optionally uses owned guest media"]
fn initial_population_failures_keep_completed_files_incomplete_until_a_verified_resume() {
    for failure in ["cancel", "missing-source", "changed-source"] {
        let fixture = Population::new();
        let result = fixture.after_first_install(|fixture| match failure {
            "cancel" => fixture.intent.cancel.store(true, Ordering::Release),
            "missing-source" => fs::remove_file(fixture.plan.portable.path.join("b")).unwrap(),
            "changed-source" => fixture.changed_source("b", b"second-modified"),
            _ => unreachable!(),
        });
        assert!(result.is_err(), "{failure}: {result:?}");
        let error = result.as_ref().unwrap_err();
        match failure {
            "cancel" => assert!(error.to_string().contains("Cancelled"), "{error}"),
            "missing-source" => assert!(
                matches!(error, crate::Error::Io(io) if io.raw_os_error() == Some(libc::ENOENT)),
                "{error}"
            ),
            "changed-source" => assert!(
                error.to_string().contains("changed after inventory"),
                "{error}"
            ),
            _ => unreachable!(),
        }
        persist_result(&fixture.db, &fixture.intent, &result).unwrap();
        let retained = fixture.session();
        assert!(!retained.browsable());
        assert!(retained.backing.as_ref().unwrap().mounted);
        assert!(retained.backing.as_ref().unwrap().initialized);
        assert!(retained.last_flushed.is_none());
        let pair: SyncPair = fixture
            .db
            .get("pair", &fixture.intent.pair)
            .unwrap()
            .unwrap();
        assert!(pair.paused && pair.last_success.is_none());
        assert!(
            super::super::ram_pending::snapshot(&fixture.db, &fixture.intent.pair)
                .unwrap()
                .full_check_required
        );
        let names = fs::read_dir(&fixture.plan.local.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("a")],
            "{failure}: staging must not be published"
        );
        assert_eq!(
            fs::read(fixture.plan.local.path.join("a")).unwrap(),
            b"source-before"
        );
        fixture.intent.cancel.store(false, Ordering::Release);
        if failure == "missing-source" {
            fs::write(fixture.plan.portable.path.join("b"), b"second-original").unwrap();
        }
        let resumed = fixture.run(&mut |_, _| {});
        assert!(resumed.is_ok(), "{failure}: {resumed:?}");
        fixture.publish_complete(&resumed);
        fixture.assert_converged(&["a", "b"]);
    }
}
