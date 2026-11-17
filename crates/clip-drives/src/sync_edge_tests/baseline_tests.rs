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
        let db = Store::existing(&_temp.path().join("state.sqlite"), false).unwrap();
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
    fn interrupt(&self, fault: Fault) {
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
        let result = self.fixture.run_with(|_, _| {
            if !injected && fs::read(self.sides().1.join("a")).ok().as_deref() == Some(NEW) {
                self.baseline("a", Some(NEW));
                Self::file(self.sides().1, "b", self.prior());
                match fault {
                    Fault::Journal => (),
                    Fault::Cancel => self.fixture.cancel.store(true, Ordering::Release),
                    Fault::SourceMissing => fs::remove_file(self.sides().0.join("b")).unwrap(),
                    Fault::SourceChanged => Self::edit(&self.sides().0.join("b")),
                    Fault::DestinationChanged => Self::edit(&self.sides().1.join("b")),
                }
                injected = true;
            }
        });
        assert!(injected, "The fault must follow the first committed file");
        let error = result
            .err()
            .expect("the injected fault must interrupt sync");
        match fault {
            Fault::Journal => assert!(
                error.to_string().contains("baseline publication fault"),
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
                Fault::Journal => Some(NEW),
                Fault::DestinationChanged => Some(EDIT),
                _ => self.prior(),
            },
        );
        Self::file(self.sides().1, "c", self.prior());
        self.baseline("a", Some(NEW));
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
    fn recover(mut self, fault: Fault) {
        let target = Root::open(self.sides().1).unwrap();
        let installed = ["a", "b"]
            .into_iter()
            .filter(|name| *name == "a" || fault == Fault::Journal)
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
            .execute_batch("DROP TRIGGER IF EXISTS baseline_write_fault")
            .unwrap();
        if fault == Fault::SourceMissing {
            fs::write(self.sides().0.join("b"), NEW).unwrap();
        }
        self.fixture = self.fixture.reopened();
        self.baseline("b", self.prior());
        self.fixture.run().unwrap();
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
