use super::*;
use std::fs;
use std::os::unix::fs::symlink;
use std::time::Instant;

struct Fixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    destination: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        Self {
            _root: root,
            source,
            destination,
        }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.source.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn batch(&self, kind: Kind, sources: Vec<PathBuf>) -> Report {
        Batch {
            kind,
            sources,
            expected: None,
            native_drop: None,
        }
        .execute(&Control::default())
    }
}

#[test]
fn recursive_copy_preserves_links_and_source_data() {
    let fixture = Fixture::new();
    let folder = fixture.source.join("tree");
    fs::create_dir_all(folder.join("nested")).unwrap();
    fs::write(folder.join("nested/data"), b"original bytes").unwrap();
    symlink("nested/data", folder.join("alias")).unwrap();
    symlink("missing", folder.join("broken")).unwrap();
    let report = fixture.batch(
        Kind::Copy(fixture.destination.clone()),
        vec![folder.clone()],
    );
    assert!(report.failed.is_empty());
    assert_eq!(report.completed, [folder.clone()]);
    let copied = fixture.destination.join("tree");
    assert_eq!(
        fs::read(copied.join("nested/data")).unwrap(),
        b"original bytes"
    );
    assert_eq!(
        fs::read_link(copied.join("alias")).unwrap(),
        Path::new("nested/data")
    );
    assert_eq!(
        fs::read_link(copied.join("broken")).unwrap(),
        Path::new("missing")
    );
    assert!(folder.join("nested/data").is_file());
}

#[test]
fn same_filesystem_move_is_complete_and_removes_original() {
    let fixture = Fixture::new();
    let source = fixture.file("move", b"move bytes");
    let report = fixture.batch(
        Kind::Move(fixture.destination.clone()),
        vec![source.clone()],
    );
    assert!(report.failed.is_empty());
    assert!(!source.exists());
    assert_eq!(
        fs::read(fixture.destination.join("move")).unwrap(),
        b"move bytes"
    );
    assert_eq!(report.moved, [(source, fixture.destination.join("move"))]);
}

#[test]
fn cross_filesystem_move_finishes_destination_before_removing_source() {
    use std::os::unix::fs::MetadataExt;
    let source_root = tempfile::tempdir_in("/dev/shm").expect("Linux shared-memory filesystem");
    let fixture = Fixture::new();
    assert_ne!(
        fs::metadata(source_root.path()).unwrap().dev(),
        fs::metadata(&fixture.destination).unwrap().dev()
    );
    let source = source_root.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("data"), b"cross-filesystem bytes").unwrap();
    let report = fixture.batch(
        Kind::Move(fixture.destination.clone()),
        vec![source.clone()],
    );
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!source.exists());
    assert_eq!(
        fs::read(fixture.destination.join("tree/data")).unwrap(),
        b"cross-filesystem bytes"
    );
}

#[test]
fn copy_into_source_and_symlink_to_its_descendant_are_rejected() {
    let fixture = Fixture::new();
    let folder = fixture.source.join("tree");
    fs::create_dir_all(folder.join("nested")).unwrap();
    let alias = fixture.destination.join("alias");
    symlink(folder.join("nested"), &alias).unwrap();
    for destination in [folder.join("nested"), alias] {
        let report = fixture.batch(Kind::Copy(destination), vec![folder.clone()]);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].1.contains("inside the source"));
        assert!(
            fs::read_dir(folder.join("nested"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[test]
fn conflicts_skip_replace_and_alternate_name_without_losing_source() {
    let fixture = Fixture::new();
    let source = fixture.file("same", b"new");
    let target = fixture.destination.join("same");
    fs::write(&target, b"old").unwrap();
    let control = Control::default();
    assert!(
        copy::transfer(
            &source,
            &fixture.destination,
            false,
            &control,
            &mut Some(Decision::Skip)
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(fs::read(&target).unwrap(), b"old");
    assert_eq!(
        copy::transfer(
            &source,
            &fixture.destination,
            false,
            &control,
            &mut Some(Decision::Replace)
        )
        .unwrap(),
        Some(target.clone())
    );
    assert_eq!(fs::read(&target).unwrap(), b"new");
    control.progress.lock().unwrap().decision = Some((Decision::Name("another".into()), false));
    assert_eq!(
        copy::transfer(&source, &fixture.destination, false, &control, &mut None).unwrap(),
        Some(fixture.destination.join("another"))
    );
    assert_eq!(fs::read(&source).unwrap(), b"new");
}

#[test]
fn broken_destination_link_is_a_conflict_and_replace_does_not_follow_it() {
    let fixture = Fixture::new();
    let source = fixture.file("same", b"new");
    let outside = fixture._root.path().join("never-created");
    symlink(&outside, fixture.destination.join("same")).unwrap();
    copy::transfer(
        &source,
        &fixture.destination,
        false,
        &Control::default(),
        &mut Some(Decision::Replace),
    )
    .unwrap();
    assert_eq!(fs::read(fixture.destination.join("same")).unwrap(), b"new");
    assert!(!outside.exists());
}

#[test]
fn batch_reports_partial_failure_and_keeps_successful_results() {
    let fixture = Fixture::new();
    let source = fixture.file("valid", b"retained");
    let missing = fixture.source.join("missing");
    let report = fixture.batch(
        Kind::Copy(fixture.destination.clone()),
        vec![source.clone(), missing.clone()],
    );
    assert_eq!(report.completed, [source]);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, missing);
    assert_eq!(
        fs::read(fixture.destination.join("valid")).unwrap(),
        b"retained"
    );
}

#[test]
fn cancelling_a_staged_copy_keeps_source_and_never_installs_destination() {
    let fixture = Fixture::new();
    let source = fixture.file("cancel", b"unchanged");
    let control = Arc::new(Control::default());
    let guard = control.progress.lock().unwrap();
    let worker_control = Arc::clone(&control);
    let directory = fixture.destination.clone();
    let worker_source = source.clone();
    let worker = std::thread::spawn(move || {
        copy::transfer(
            &worker_source,
            &directory,
            false,
            &worker_control,
            &mut None,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_dir(&fixture.destination).unwrap().next().is_none() && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(1));
    }
    let staged = fs::read_dir(&fixture.destination).unwrap().next().is_some();
    control.cancelled.store(true, Ordering::Relaxed);
    drop(guard);
    assert!(worker.join().unwrap().is_err());
    assert!(staged, "worker reached staging before cancellation");
    assert_eq!(fs::read(&source).unwrap(), b"unchanged");
    assert!(fs::read_dir(&fixture.destination).unwrap().next().is_none());
}

#[test]
fn cancelled_batch_counts_unprocessed_entries() {
    let fixture = Fixture::new();
    let source = fixture.file("cancel", b"unchanged");
    let control = Control::default();
    control.cancelled.store(true, Ordering::Relaxed);
    let report = Batch {
        kind: Kind::Copy(fixture.destination.clone()),
        sources: vec![source],
        expected: None,
        native_drop: None,
    }
    .execute(&control);
    assert!(report.cancelled);
    assert_eq!(report.unprocessed, 1);
    assert!(report.completed.is_empty());
}

#[test]
fn permanent_deletion_refuses_an_entry_changed_after_confirmation() {
    let fixture = Fixture::new();
    let source = fixture.file("changed", b"before");
    let batch = confirmed_deletion(&source);
    fs::write(&source, b"after-and-longer").unwrap();
    let report = batch.execute(&Control::default());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read(source).unwrap(), b"after-and-longer");
}

#[test]
fn permanent_deletion_removes_link_itself_and_preserves_target() {
    let fixture = Fixture::new();
    let target = fixture.file("target", b"kept");
    let source = fixture.source.join("link");
    symlink(&target, &source).unwrap();
    let report = confirmed_deletion(&source).execute(&Control::default());
    assert!(report.failed.is_empty());
    assert!(fs::symlink_metadata(source).is_err());
    assert_eq!(fs::read(target).unwrap(), b"kept");
}

#[test]
fn queue_is_bounded_deduplicates_children_and_retains_order() {
    let mut transfers = Transfers::default();
    assert!(transfers.enqueue(
        Kind::Trash,
        vec![
            "/tmp/root/a".into(),
            "/tmp/root".into(),
            "/tmp/root/a".into()
        ]
    ));
    assert_eq!(transfers.queue[0].sources, [PathBuf::from("/tmp/root")]);
    for index in 1..8 {
        assert!(transfers.enqueue(Kind::Trash, vec![format!("/tmp/{index}").into()]));
    }
    assert!(!transfers.enqueue(Kind::Trash, vec!["/tmp/overflow".into()]));
    assert_eq!(transfers.queue.len(), 8);
}

#[test]
fn restore_refuses_existing_target_and_retains_trash_payload() {
    let fixture = Fixture::new();
    let info_root = fixture._root.path().join("Trash");
    fs::create_dir_all(info_root.join("files")).unwrap();
    fs::create_dir(info_root.join("info")).unwrap();
    let info = info_root.join("info/data.trashinfo");
    fs::write(&info, b"fixture metadata").unwrap();
    fs::write(info_root.join("files/data"), b"trashed").unwrap();
    let target = fixture.destination.join("original");
    fs::write(&target, b"existing").unwrap();
    let item = trash::TrashItem {
        id: info.clone().into_os_string(),
        name: "original".into(),
        original_parent: fixture.destination.clone(),
        time_deleted: 0,
    };
    assert!(trash_support::restore(&item).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"existing");
    fs::remove_file(&target).unwrap();
    trash_support::restore(&item).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"trashed");
    assert!(!info.exists());
}

#[test]
fn desktop_move_cleanup_refuses_changed_descendants_and_accepts_already_moved_sources() {
    let fixture = Fixture::new();
    let folder = fixture.source.join("folder");
    fs::create_dir(&folder).unwrap();
    let file = folder.join("data");
    fs::write(&file, b"before").unwrap();
    let root_identity = super::super::filesystem::identity(&fs::symlink_metadata(&folder).unwrap());
    let file_identity = super::super::filesystem::identity(&fs::symlink_metadata(&file).unwrap());
    let batch = Batch {
        kind: Kind::NativeMoveCleanup(BTreeMap::from([
            (folder.clone(), root_identity),
            (file.clone(), file_identity),
        ])),
        sources: vec![folder.clone()],
        expected: Some(BTreeMap::from([(folder.clone(), Some(root_identity))])),
        native_drop: None,
    };
    fs::write(&file, b"new edits during drop").unwrap();
    let report = batch.execute(&Control::default());
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read(&file).unwrap(), b"new edits during drop");
    fs::rename(&folder, fixture.destination.join("folder")).unwrap();
    let report = batch.execute(&Control::default());
    assert!(report.failed.is_empty());
    assert_eq!(report.completed, [folder]);
}

#[test]
fn desktop_receiver_receipt_records_partial_failure_without_authorizing_source_removal() {
    let fixture = Fixture::new();
    let source = fixture.file("one", b"one");
    let batch = Batch {
        kind: Kind::Copy(fixture.destination.clone()),
        sources: vec![source, fixture.source.join("missing")],
        expected: None,
        native_drop: Some(42),
    };
    let report = batch.execute(&Control::default());
    assert_eq!(report.native_drop, Some(42));
    assert_eq!(report.completed.len(), 1);
    assert_eq!(report.failed.len(), 1);
}

fn confirmed_deletion(source: &Path) -> Batch {
    let expected = super::super::filesystem::identity(&fs::symlink_metadata(source).unwrap());
    Batch {
        kind: Kind::Delete,
        sources: vec![source.into()],
        expected: Some(BTreeMap::from([(source.into(), Some(expected))])),
        native_drop: None,
    }
}

#[test]
fn cancelling_waiting_batches_retains_counts_and_rejects_native_move_acknowledgment() {
    let mut transfers = Transfers::default();
    assert!(transfers.enqueue(
        Kind::Copy("/tmp/destination".into()),
        vec!["/tmp/source".into()]
    ));
    transfers.queue.back_mut().unwrap().native_drop = Some(7);
    transfers.cancel_waiting();
    assert!(transfers.queue.is_empty());
    assert_eq!(transfers.native_results, [(7, false)]);
    assert!(transfers.reports[0].cancelled);
    assert_eq!(transfers.reports[0].unprocessed, 1);
}

#[test]
fn recursive_copy_preserves_file_permissions_and_modification_time() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let fixture = Fixture::new();
    let file = fixture.file("metadata", b"data");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    fs::File::open(&file)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(12345)),
        )
        .unwrap();
    let report = fixture.batch(Kind::Copy(fixture.destination.clone()), vec![file]);
    assert!(report.failed.is_empty());
    let copied = fs::metadata(fixture.destination.join("metadata")).unwrap();
    assert_eq!(copied.mode() & 0o777, 0o640);
    assert_eq!(copied.mtime(), 12345);
}
