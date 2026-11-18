//! Baseline faults preserve installed bytes and retry through a reopened local registry.
use super::*;
use little_clip_core::file_source::FileSource;
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, FileTimes},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::atomic::Ordering,
};

const OLD: &[u8] = b"old payload";
const NEW: &[u8] = b"new payload";
const EDIT: &[u8] = b"bad payload";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    Journal,
    Receipt,
    Cancel,
    SourceMissing,
    SourceChanged,
    DestinationChanged,
}

impl Fixture {
    fn reopened(self) -> Self {
        let Self {
            _temp,
            db,
            a,
            b,
            pair,
            cancel: _,
        } = self;
        drop(db);
        let db = Store::existing(&_temp.path().join("state/drives.sqlite"), false).unwrap();
        let saved: SyncPair = db.get("pair", &pair.id).unwrap().unwrap();
        assert_eq!(saved.relative, Path::new("selected"));
        assert_eq!(saved.local, b);
        assert_eq!(saved.local_identity, pair.local_identity);
        Self {
            _temp,
            db,
            a,
            b,
            pair: saved,
            cancel: AtomicBool::new(false),
        }
    }
}

fn record_operation(db: &Store) {
    let operation = crate::DriveOperation {
        id: "interrupted-sync".into(),
        title: "Test sync".into(),
        state: "running".into(),
        message: String::new(),
        files: 0,
        bytes: 0,
        started: crate::now(),
        finished: None,
        partition: None,
        sync_completion: None,
    };
    db.put("operation", &operation.id, &operation).unwrap();
}

struct Case {
    fixture: Fixture,
    _media: Option<tempfile::TempDir>,
    portable_to_local: bool,
    existing: bool,
}
impl Case {
    fn new(portable_to_local: bool, existing: bool) -> Self {
        let (media, mut fixture) =
            if std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").is_some() {
                let (media, fixture, _) = super::native_fs::fixture();
                (Some(media), fixture)
            } else {
                (None, Fixture::new())
            };
        fixture.a = fixture.a.join("selected");
        fs::create_dir(&fixture.a).unwrap();
        fixture.pair.relative = "selected".into();
        fixture.pair.local = fixture.b.clone();
        fixture.pair.local_identity =
            Some(Root::open(&fixture.b).unwrap().storage_identity().unwrap());
        let mut case = Self {
            fixture,
            _media: media,
            portable_to_local,
            existing,
        };
        if existing {
            for name in ["a", "b", "c"] {
                fs::write(case.sides().0.join(name), OLD).unwrap();
            }
            case.fixture.run().unwrap();
            case.fixture.pair.last_success = Some(crate::now());
        }
        for name in ["a", "b", "c"] {
            fs::write(case.sides().0.join(name), NEW).unwrap();
        }
        case.fixture
            .db
            .put("pair", &case.fixture.pair.id, &case.fixture.pair)
            .unwrap();
        case
    }
    fn sides(&self) -> (&Path, &Path) {
        if self.portable_to_local {
            (&self.fixture.a, &self.fixture.b)
        } else {
            (&self.fixture.b, &self.fixture.a)
        }
    }
    fn baseline(&self, name: &str, bytes: Option<&[u8]>) {
        let saved: Option<String> = self
            .fixture
            .db
            .0
            .query_row(
                "SELECT hash FROM baseline WHERE pair=?1 AND path=?2",
                params![self.fixture.pair.id, name.as_bytes()],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(
            saved,
            bytes.map(|bytes| format!("{:x}", Sha256::digest(bytes))),
            "{name}"
        );
    }
    fn prior(&self) -> Option<&[u8]> {
        self.existing.then_some(OLD)
    }
    fn file(root: &Path, name: &str, bytes: Option<&[u8]>) {
        if let Some(bytes) = bytes {
            assert_eq!(fs::read(root.join(name)).unwrap(), bytes);
        } else {
            assert!(!root.join(name).exists());
        }
    }
    fn edit(path: &Path) {
        let old = fs::metadata(path).ok();
        fs::write(path, EDIT).unwrap();
        if let Some(old) = old {
            File::open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(old.modified().unwrap()))
                .unwrap();
            let current = fs::metadata(path).unwrap();
            assert_eq!(current.len(), old.len());
            assert_eq!(
                (current.mtime(), current.mtime_nsec()),
                (old.mtime(), old.mtime_nsec())
            );
        }
    }
    fn run_recorded(&self, progress: impl FnMut(u64, u64)) -> Result<sync::SyncReport> {
        record_operation(&self.fixture.db);
        sync::synchronize_with(
            &self.fixture.db,
            &self.fixture.pair,
            (
                &Root::open(&self.fixture.a).unwrap(),
                &Root::open(&self.fixture.b).unwrap(),
            ),
            &self.fixture.cancel,
            progress,
            sync::Options {
                operation: Some("interrupted-sync"),
                ..Default::default()
            },
        )
    }
    fn interrupt(&self, fault: Fault) {
        if fault == Fault::Receipt {
            self.fixture
                .db
                .0
                .execute_batch(
                    "CREATE TRIGGER receipt_write_fault BEFORE INSERT ON sync_receipts
                 WHEN NEW.path=x'62' BEGIN SELECT RAISE(FAIL,'receipt publication fault'); END;",
                )
                .unwrap();
        }
        if fault == Fault::Journal {
            self.fixture
                .db
                .0
                .execute_batch(
                    "CREATE TRIGGER baseline_write_fault BEFORE INSERT ON baseline
                WHEN NEW.path=x'62' BEGIN SELECT RAISE(FAIL,'baseline publication fault'); END;",
                )
                .unwrap();
        }
        let mut injected = false;
        let progress = |_, _| {
            if !injected && fs::read(self.sides().1.join("a")).ok().as_deref() == Some(NEW) {
                self.baseline("a", Some(NEW));
                Self::file(self.sides().1, "b", self.prior());
                match fault {
                    Fault::Journal | Fault::Receipt => (),
                    Fault::Cancel => self.fixture.cancel.store(true, Ordering::Release),
                    Fault::SourceMissing => fs::remove_file(self.sides().0.join("b")).unwrap(),
                    Fault::SourceChanged => Self::edit(&self.sides().0.join("b")),
                    Fault::DestinationChanged => Self::edit(&self.sides().1.join("b")),
                }
                injected = true;
            }
        };
        let result = self.run_recorded(progress);
        assert!(injected, "The fault must follow the first committed file");
        let error = result
            .err()
            .expect("the injected fault must interrupt sync");
        match fault {
            Fault::Journal => assert!(
                error.to_string().contains("baseline publication fault"),
                "{error}"
            ),
            Fault::Receipt => assert!(
                error.to_string().contains("receipt publication fault"),
                "{error}"
            ),
            Fault::Cancel => assert!(error.to_string().contains("Cancelled"), "{error}"),
            Fault::SourceMissing => assert_eq!(crate::raw_errno(error), Some(libc::ENOENT)),
            Fault::SourceChanged => assert!(
                error.to_string().contains("changed after inventory"),
                "{error}"
            ),
            Fault::DestinationChanged => {
                assert!(error.to_string().contains("Destination"), "{error}")
            }
        }
        Self::file(self.sides().1, "a", Some(NEW));
        Self::file(
            self.sides().1,
            "b",
            match fault {
                Fault::Journal | Fault::Receipt => Some(NEW),
                Fault::DestinationChanged => Some(EDIT),
                _ => self.prior(),
            },
        );
        Self::file(self.sides().1, "c", self.prior());
        self.baseline("a", Some(NEW));
        self.receipt();
        for name in ["b", "c"] {
            self.baseline(name, self.prior());
        }
        assert!(
            self.fixture
                .db
                .list::<DriveConflict>("conflict", 0, 10)
                .unwrap()
                .is_empty()
        );
    }
    fn receipt(&self) {
        let operation: crate::DriveOperation = self
            .fixture
            .db
            .get("operation", "interrupted-sync")
            .unwrap()
            .unwrap();
        assert_eq!(
            operation.sync_completion,
            Some(crate::SyncCompletion {
                installed: 1,
                removed: 0,
                bytes: NEW.len() as u64
            })
        );
        let receipts: Vec<(Vec<u8>, String, String, u64)> = self.fixture.db.0
            .prepare("SELECT path,target,hash,bytes FROM sync_receipts WHERE operation='interrupted-sync' ORDER BY id").unwrap()
            .query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap()
            .collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(
            receipts,
            vec![(
                b"a".to_vec(),
                if self.portable_to_local {
                    "local"
                } else {
                    "portable"
                }
                .into(),
                format!("{:x}", Sha256::digest(NEW)),
                NEW.len() as u64
            )]
        );
    }
    fn recover(mut self, fault: Fault) {
        let target = Root::open(self.sides().1).unwrap();
        let installed = ["a", "b"]
            .into_iter()
            .filter(|name| *name == "a" || matches!(fault, Fault::Journal | Fault::Receipt))
            .map(|name| {
                let file = target.input(Path::new(name)).unwrap();
                let revision = FileSource::capture_file(&file).unwrap();
                (file, revision)
            })
            .collect::<Vec<_>>();
        drop(target);
        self.fixture
            .db
            .0
            .execute_batch("DROP TRIGGER IF EXISTS baseline_write_fault; DROP TRIGGER IF EXISTS receipt_write_fault")
            .unwrap();
        if fault == Fault::SourceMissing {
            fs::write(self.sides().0.join("b"), NEW).unwrap();
        }
        self.fixture = self.fixture.reopened();
        self.receipt();
        self.baseline("b", self.prior());
        self.fixture.run().unwrap();
        self.receipt();
        for (file, revision) in installed {
            revision.check_file(&file).unwrap();
        }
        for name in ["a", "c"] {
            for root in [&self.fixture.a, &self.fixture.b] {
                Self::file(root, name, Some(NEW));
            }
            self.baseline(name, Some(NEW));
        }
        if fault == Fault::DestinationChanged {
            let conflicts = self
                .fixture
                .db
                .list::<DriveConflict>("conflict", 0, 10)
                .unwrap();
            assert_eq!(conflicts.len(), 1);
            assert!(conflicts[0].complete && !conflicts[0].resolved);
            for root in [&self.fixture.a, &self.fixture.b] {
                Self::file(
                    root,
                    conflicts[0].portable.to_str().unwrap(),
                    Some(if self.portable_to_local { NEW } else { EDIT }),
                );
                Self::file(
                    root,
                    conflicts[0].local.to_str().unwrap(),
                    Some(if self.portable_to_local { EDIT } else { NEW }),
                );
            }
            Self::file(self.sides().0, "b", Some(NEW));
            Self::file(self.sides().1, "b", Some(EDIT));
            self.baseline("b", self.prior());
        } else {
            let bytes = if fault == Fault::SourceChanged {
                EDIT
            } else {
                NEW
            };
            for root in [&self.fixture.a, &self.fixture.b] {
                Self::file(root, "b", Some(bytes));
            }
            self.baseline("b", Some(bytes));
        }
    }
}

#[test]
fn interrupted_installs_keep_only_verified_common_baselines_and_recover_after_reopen() {
    for portable_to_local in [false, true] {
        for existing in [false, true] {
            for fault in [
                Fault::Journal,
                Fault::Receipt,
                Fault::Cancel,
                Fault::SourceMissing,
                Fault::SourceChanged,
                Fault::DestinationChanged,
            ] {
                eprintln!(
                    "baseline case: portable_to_local={portable_to_local} existing={existing} fault={fault:?}"
                );
                let case = Case::new(portable_to_local, existing);
                case.interrupt(fault);
                case.recover(fault);
            }
        }
    }
}

#[test]
fn confirmed_file_and_directory_deletions_have_separate_durable_receipts() {
    for portable_to_local in [false, true] {
        let mut case = Case::new(portable_to_local, true);
        fs::create_dir(case.sides().0.join("empty")).unwrap();
        case.fixture.run().unwrap();
        case.fixture.pair.deletions = true;
        for name in ["b", "empty"] {
            let path = case.sides().0.join(name);
            if name == "empty" {
                fs::remove_dir(path).unwrap();
            } else {
                fs::remove_file(path).unwrap();
            }
        }
        case.run_recorded(|_, _| {}).unwrap();
        let operation: crate::DriveOperation = case
            .fixture
            .db
            .get("operation", "interrupted-sync")
            .unwrap()
            .unwrap();
        assert_eq!(
            operation.sync_completion,
            Some(crate::SyncCompletion {
                installed: 0,
                removed: 2,
                bytes: 0
            })
        );
        let mut statement = case
            .fixture
            .db
            .0
            .prepare("SELECT path,target,hash IS NULL AND bytes IS NULL FROM sync_receipts ORDER BY path")
            .unwrap();
        let rows: Vec<(Vec<u8>, String, bool)> = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let target = if portable_to_local {
            "local"
        } else {
            "portable"
        };
        assert_eq!(
            rows,
            vec![
                (b"b".to_vec(), target.into(), true),
                (b"empty".to_vec(), target.into(), true)
            ]
        );
        for name in ["b", "empty"] {
            case.baseline(name, None);
            for root in [&case.fixture.a, &case.fixture.b] {
                assert!(!root.join(name).exists());
            }
        }
    }
}

#[test]
fn installation_receipts_preserve_non_utf8_path_bytes() {
    use std::os::unix::ffi::OsStringExt;
    let case = Case {
        fixture: Fixture::new(),
        _media: None,
        portable_to_local: true,
        existing: false,
    };
    let name = std::ffi::OsString::from_vec(b"entry-\xff".to_vec());
    fs::write(case.fixture.a.join(&name), NEW).unwrap();
    case.run_recorded(|_, _| {}).unwrap();
    let path: Vec<u8> = case
        .fixture
        .db
        .0
        .query_row("SELECT path FROM sync_receipts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(path, b"entry-\xff");
    assert_eq!(fs::read(case.fixture.b.join(&name)).unwrap(), NEW);
}

#[test]
#[ignore = "requires a parent-owned fixture; the parent kills this process after one committed file"]
fn sync_receipt_crash_probe() {
    let root = PathBuf::from(std::env::var_os("LITTLE_CLIP_SYNC_RECEIPT_FIXTURE").unwrap());
    assert_eq!(
        fs::read(root.join(".owned-sync-receipt")).unwrap(),
        b"owned receipt fixture"
    );
    let (a, b): (PathBuf, PathBuf) =
        serde_json::from_slice(&fs::read(root.join("roots.json")).unwrap()).unwrap();
    let db = Store::existing(&root.join("state/drives.sqlite"), false).unwrap();
    let pair: SyncPair = db.get("pair", "edge").unwrap().unwrap();
    sync::synchronize_with(
        &db,
        &pair,
        (&Root::open(&a).unwrap(), &Root::open(&b).unwrap()),
        &AtomicBool::new(false),
        |_, _| {
            let operation: crate::DriveOperation =
                db.get("operation", "interrupted-sync").unwrap().unwrap();
            if operation
                .sync_completion
                .as_ref()
                .is_some_and(|s| s.installed == 1)
            {
                fs::write(root.join("ready"), b"one committed receipt").unwrap();
                loop {
                    std::thread::park();
                }
            }
        },
        sync::Options {
            operation: Some("interrupted-sync"),
            ..Default::default()
        },
    )
    .unwrap();
    panic!("parent must kill the held worker before its next file");
}

#[test]
fn killed_sync_workers_retain_receipts_through_actual_service_startup() {
    use little_clip_core::process::CommandRequest;
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    for portable_to_local in [false, true] {
        for existing in [false, true] {
            let case = Case::new(portable_to_local, existing);
            let root = case.fixture._temp.path();
            record_operation(&case.fixture.db);
            fs::write(root.join(".owned-sync-receipt"), b"owned receipt fixture").unwrap();
            fs::write(
                root.join("roots.json"),
                serde_json::to_vec(&(&case.fixture.a, &case.fixture.b)).unwrap(),
            )
            .unwrap();
            let mut command = CommandRequest::new(std::env::current_exe().unwrap())
                .args([
                    "sync_edge_tests::baseline_tests::sync_receipt_crash_probe",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ])
                .own_process_group()
                .die_with_parent()
                .build_command();
            command.env("LITTLE_CLIP_SYNC_RECEIPT_FIXTURE", root);
            let mut worker = crate::test_child::Owned(command.spawn().unwrap());
            let started = Instant::now();
            while fs::read(root.join("ready")).ok().as_deref() != Some(b"one committed receipt") {
                assert!(
                    worker.0.try_wait().unwrap().is_none(),
                    "fixture worker exited before its committed receipt"
                );
                assert!(started.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(10));
            }
            case.receipt();
            worker.0.kill().unwrap();
            assert!(!worker.0.wait().unwrap().success());
            let service = crate::Service::start_with_inventory(
                crate::DriveConfig {
                    ram_min_available: u64::MAX,
                    ..Default::default()
                },
                root.join("state"),
                Arc::new(|| Ok(Vec::new())),
            )
            .unwrap();
            let crate::DriveReply::Status(status) = service
                .request(crate::DriveRequest::Status { offset: 0 })
                .unwrap()
            else {
                panic!("expected status");
            };
            let operation = status
                .operations
                .iter()
                .find(|op| op.id == "interrupted-sync")
                .unwrap();
            assert_eq!(operation.state, "interrupted");
            assert!(operation.finished.is_some());
            assert_eq!(
                operation.sync_completion,
                Some(crate::SyncCompletion {
                    installed: 1,
                    removed: 0,
                    bytes: NEW.len() as u64
                })
            );
            case.receipt();
            service.shutdown().unwrap();
            case.recover(Fault::Cancel);
        }
    }
}

#[test]
fn a_failed_deletion_baseline_commit_cannot_resurrect_removed_files_after_reopen() {
    for portable_to_local in [false, true] {
        let mut case = Case::new(portable_to_local, true);
        case.fixture.run().unwrap();
        case.fixture.pair.deletions = true;
        case.fixture
            .db
            .put("pair", &case.fixture.pair.id, &case.fixture.pair)
            .unwrap();
        fs::remove_file(case.sides().0.join("b")).unwrap();
        case.fixture
            .db
            .0
            .execute_batch(
                "CREATE TRIGGER baseline_delete_fault BEFORE DELETE ON baseline
            WHEN OLD.path=x'62' BEGIN SELECT RAISE(FAIL,'baseline deletion fault'); END;",
            )
            .unwrap();
        let error = case
            .fixture
            .run()
            .err()
            .expect("baseline deletion must fail");
        assert!(
            error.to_string().contains("baseline deletion fault"),
            "{error}"
        );
        for root in [&case.fixture.a, &case.fixture.b] {
            Case::file(root, "b", None);
        }
        case.baseline("b", Some(NEW));
        case.fixture
            .db
            .0
            .execute_batch("DROP TRIGGER baseline_delete_fault")
            .unwrap();
        case.fixture = case.fixture.reopened();
        case.fixture.run().unwrap();
        case.baseline("b", None);
        for root in [&case.fixture.a, &case.fixture.b] {
            Case::file(root, "b", None);
            for name in ["a", "c"] {
                Case::file(root, name, Some(NEW));
            }
        }
    }
}
