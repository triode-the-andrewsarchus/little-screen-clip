//! Reconciliation regressions exercised with owned ordinary filesystem fixtures.
use crate::{root::Root, store::Store, sync, *};
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};
struct Fixture {
    _temp: tempfile::TempDir,
    db: Store,
    a: PathBuf,
    b: PathBuf,
    pair: SyncPair,
    cancel: AtomicBool,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("portable");
        let b = temp.path().join("local");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        Self {
            _temp: temp,
            db,
            a,
            b,
            pair: SyncPair {
                local_identity: None,
                id: "edge".into(),
                drive: "fixture".into(),
                evidence: "fixture".into(),
                relative: PathBuf::new(),
                local: PathBuf::new(),
                paused: false,
                deletions: false,
                ram: false,
                last_success: None,
                status: String::new(),
            },
            cancel: AtomicBool::new(false),
        }
    }
    fn run(&self) -> sync::SyncReport {
        sync::synchronize(
            &self.db,
            &self.pair,
            &Root::open(&self.a).unwrap(),
            &Root::open(&self.b).unwrap(),
            &self.cancel,
            |_, _| {},
        )
        .unwrap()
    }
}
#[test]
fn delete_versus_edit_restores_the_edit_and_reports_a_conflict() {
    let mut fixture = Fixture::new();
    fs::write(fixture.a.join("x"), b"baseline").unwrap();
    fixture.run();
    fixture.pair.deletions = true;
    fs::remove_file(fixture.a.join("x")).unwrap();
    fs::write(fixture.b.join("x"), b"offline edit").unwrap();
    fixture.run();
    assert_eq!(fs::read(fixture.a.join("x")).unwrap(), b"offline edit");
    assert!(
        !fixture
            .db
            .list::<DriveConflict>("conflict", 0, 100)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn confirmed_directory_deletions_propagate_after_their_unchanged_children() {
    let mut fixture = Fixture::new();
    fs::create_dir(fixture.a.join("folder")).unwrap();
    fs::write(fixture.a.join("folder/x"), b"baseline").unwrap();
    fixture.run();
    fixture.pair.deletions = true;
    fs::remove_dir_all(fixture.a.join("folder")).unwrap();
    fixture.run();
    assert!(!fixture.a.join("folder").exists());
    assert!(!fixture.b.join("folder").exists());
}
#[test]
fn file_directory_conflicts_preserve_both_complete_variants() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.a.join("collision")).unwrap();
    fs::write(fixture.a.join("collision/child"), b"directory contents").unwrap();
    fs::write(fixture.b.join("collision"), b"regular file").unwrap();
    fixture.run();
    let conflicts = fixture
        .db
        .list::<DriveConflict>("conflict", 0, 100)
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    for root in [&fixture.a, &fixture.b] {
        assert_eq!(
            fs::read(root.join(&conflicts[0].portable).join("child")).unwrap(),
            b"directory contents"
        );
        assert_eq!(
            fs::read(root.join(&conflicts[0].local)).unwrap(),
            b"regular file"
        );
    }
    fixture.run();
    assert_eq!(
        fixture
            .db
            .list::<DriveConflict>("conflict", 0, 100)
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn an_incomplete_tree_never_deletes_previously_synced_data() {
    let mut fixture = Fixture::new();
    fs::write(fixture.a.join("safe"), b"keep").unwrap();
    fixture.run();
    fixture.pair.deletions = true;
    fs::remove_file(fixture.a.join("safe")).unwrap();
    let name = std::ffi::CString::new(
        fixture
            .a
            .join("unreadable-special")
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(
        sync::synchronize(
            &fixture.db,
            &fixture.pair,
            &Root::open(&fixture.a).unwrap(),
            &Root::open(&fixture.b).unwrap(),
            &fixture.cancel,
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(fs::read(fixture.b.join("safe")).unwrap(), b"keep");
}
#[test]
fn symlinks_are_preserved_as_references_without_following_payloads() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink("missing", fixture.a.join("link")).unwrap();
    fixture.run();
    assert_eq!(
        fs::read_link(fixture.b.join("link")).unwrap(),
        PathBuf::from("missing")
    );
    fixture.run();
}
#[test]
fn lossless_names_survive_sync_and_baseline_storage() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let name = std::ffi::OsString::from_vec(b"name_\xff.txt".to_vec());
    fs::write(fixture.a.join(&name), b"raw path").unwrap();
    fixture.run();
    assert_eq!(fs::read(fixture.b.join(&name)).unwrap(), b"raw path");
    fixture.run();
}
