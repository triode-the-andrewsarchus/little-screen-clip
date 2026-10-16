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
    let (item, info, _) = trash_item(&fixture, "original");
    let target = item.original_path();
    fs::write(&target, b"existing").unwrap();
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

fn admission_result(running: &mut Running, jobs: &BackgroundJobs) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match running.admit(jobs) {
            Ok(true) => return Ok(()),
            Err(error) => return Err(error),
            Ok(false) => {
                assert!(Instant::now() < deadline, "admission did not finish");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

fn unadmitted_copy(source: PathBuf, destination: PathBuf) -> Running {
    Running {
        batch: Batch {
            kind: Kind::Copy(destination),
            sources: vec![source],
            expected: None,
            native_drop: None,
        },
        control: Arc::new(Control::default()),
        receiver: None,
        admission: Admission::Unchecked,
    }
}

#[test]
fn shared_memory_files_keep_normal_admission_reading_and_text_previews() {
    let fixture = Fixture::new();
    let source = fixture.file(
        "ordinary.txt",
        b"Shared memory remains ordinary.\nSecond line.\n",
    );
    let destination = tempfile::tempdir_in("/dev/shm").unwrap();
    let jobs = BackgroundJobs::new(1, 2);
    let mut running = unadmitted_copy(source, destination.path().into());
    admission_result(&mut running, &jobs).unwrap();
    assert!(running.receiver.is_none());
    assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 0);
    let report = running.batch.execute(&running.control);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.completed.len(), 1);
    let copied = destination.path().join("ordinary.txt");
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let directory =
        super::super::filesystem::read_directory(destination.path(), &cancelled).unwrap();
    assert!(!directory.system);
    assert!(directory.entries[0].regular_file);
    assert!(directory.entries[0].system.is_none());
    let text = crate::file_source::read_file_bytes(&copied, 1024, true, false).unwrap();
    assert_eq!(text, b"Shared memory remains ordinary.\nSecond line.\n");
    let preview = super::super::text_strip::load(&copied, &cancelled).unwrap();
    assert_eq!(preview.lines, 2);
}

#[test]
fn recursive_kernel_alias_is_rejected_before_mutation_submission() {
    let fixture = Fixture::new();
    fixture.file("ordinary", b"must not be copied");
    symlink("/proc/uptime", fixture.source.join("kernel")).unwrap();
    let jobs = BackgroundJobs::new(1, 2);
    let mut running = unadmitted_copy(fixture.source.clone(), fixture.destination.clone());
    let error = admission_result(&mut running, &jobs).unwrap_err();
    assert!(error.contains("kernel"), "{error}");
    assert!(!matches!(running.admission, Admission::Accepted));
    assert!(running.receiver.is_none());
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
    assert_eq!(
        fs::read(fixture.source.join("ordinary")).unwrap(),
        b"must not be copied"
    );
}

#[test]
fn protected_destination_is_rejected_before_mutation_submission() {
    let fixture = Fixture::new();
    let source = fixture.file("ordinary", b"kept");
    let alias = fixture._root.path().join("destination-alias");
    symlink("/sys", &alias).unwrap();
    let jobs = BackgroundJobs::new(1, 2);
    let mut running = unadmitted_copy(source, alias);
    assert!(admission_result(&mut running, &jobs).is_err());
    assert!(running.receiver.is_none());
    assert!(!matches!(running.admission, Admission::Accepted));
}

#[test]
fn ordinary_batch_admission_is_read_only_and_execution_rechecks_replacement() {
    let fixture = Fixture::new();
    let source = fixture.file("ordinary", b"original");
    let jobs = BackgroundJobs::new(1, 2);
    let mut running = unadmitted_copy(source.clone(), fixture.destination.clone());
    admission_result(&mut running, &jobs).unwrap();
    assert!(matches!(running.admission, Admission::Accepted));
    assert!(running.receiver.is_none());
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
    fs::remove_file(&source).unwrap();
    symlink("/proc/uptime", &source).unwrap();
    let report = running.batch.execute(&running.control);
    assert_eq!(report.failed.len(), 1);
    assert!(report.completed.is_empty());
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
}

fn transfer_report(transfers: &mut Transfers, jobs: &BackgroundJobs) -> Report {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(report) = transfers.tick(jobs, false) {
            return report;
        }
        let running = transfers.running.as_ref().unwrap();
        assert!(running.receiver.is_none() || matches!(running.admission, Admission::Accepted));
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn transfer_tick_reports_rejected_native_drop_without_starting_mutation() {
    let fixture = Fixture::new();
    let source = fixture.file("ordinary", b"kept");
    let mut transfers = Transfers::default();
    assert!(transfers.enqueue(Kind::Copy(PathBuf::from("/sys")), vec![source]));
    transfers.queue.back_mut().unwrap().native_drop = Some(42);
    let jobs = BackgroundJobs::new(1, 2);
    let report = transfer_report(&mut transfers, &jobs);
    assert_eq!(report.native_drop, Some(42));
    assert_eq!(report.unprocessed, 1);
    assert_eq!(report.failed.len(), 1);
    assert!(report.completed.is_empty());
    assert!(!transfers.busy());
}

#[test]
fn transfer_tick_admits_then_completes_ordinary_copy() {
    let fixture = Fixture::new();
    let source = fixture.file("ordinary", b"copied");
    let mut transfers = Transfers::default();
    assert!(transfers.enqueue(Kind::Copy(fixture.destination.clone()), vec![source]));
    let jobs = BackgroundJobs::new(1, 2);
    let report = transfer_report(&mut transfers, &jobs);
    assert_eq!(report.completed.len(), 1);
    assert!(report.failed.is_empty());
    assert_eq!(
        fs::read(fixture.destination.join("ordinary")).unwrap(),
        b"copied"
    );
}

fn trash_item(fixture: &Fixture, name: &str) -> (trash::TrashItem, PathBuf, PathBuf) {
    let root = fixture._root.path().join("Trash");
    fs::create_dir_all(root.join("files")).unwrap();
    fs::create_dir_all(root.join("info")).unwrap();
    let info = root.join("info").join(format!("{name}.trashinfo"));
    let source = root.join("files").join(name);
    fs::write(&info, b"fixture metadata").unwrap();
    fs::write(&source, b"trashed").unwrap();
    let item = trash::TrashItem {
        id: info.clone().into_os_string(),
        name: name.into(),
        original_parent: fixture.destination.clone(),
        time_deleted: 0,
    };
    (item, info, source)
}

#[test]
fn restore_uses_the_held_destination_after_its_name_is_redirected() {
    use std::sync::atomic::AtomicBool;
    let fixture = Fixture::new();
    let (item, info, source) = trash_item(&fixture, "restored");
    trash_support::preflight_restore(&item, &AtomicBool::new(false)).unwrap();
    let held =
        little_clip_core::kernel_interface::pin_content_directory(&fixture.destination).unwrap();
    let moved = fixture._root.path().join("moved-destination");
    fs::rename(&fixture.destination, &moved).unwrap();
    let redirected = fixture._root.path().join("redirected");
    fs::create_dir(&redirected).unwrap();
    symlink(&redirected, &fixture.destination).unwrap();
    trash_support::restore_in(&item, &held).unwrap();
    assert_eq!(fs::read(moved.join("restored")).unwrap(), b"trashed");
    assert_eq!(fs::read_dir(&redirected).unwrap().count(), 0);
    assert!(!source.exists() && !info.exists());
}

#[test]
fn restore_recreates_missing_parents_but_refuses_escaping_names_and_broken_aliases() {
    let fixture = Fixture::new();
    let (mut item, info, source) = trash_item(&fixture, "restored");
    item.name = "../escaped".into();
    assert!(
        trash_support::restore(&item)
            .unwrap_err()
            .contains("filename")
    );
    assert!(info.exists() && source.exists());
    item.name = "restored".into();
    item.original_parent = fixture.destination.join("alias/child");
    symlink("missing", fixture.destination.join("alias")).unwrap();
    assert!(trash_support::restore(&item).is_err());
    assert!(info.exists() && source.exists());
    assert!(!fixture.destination.join("missing").exists());
    item.original_parent = fixture.destination.join("one/two");
    trash_support::restore(&item).unwrap();
    assert_eq!(fs::read(item.original_path()).unwrap(), b"trashed");
    assert!(!source.exists() && !info.exists());
}

#[test]
fn restore_rejects_metadata_names_that_resolve_to_trash_directories() {
    let fixture = Fixture::new();
    let (mut item, info, payload) = trash_item(&fixture, "ordinary");
    for name in ["..trashinfo", "...trashinfo"] {
        let invalid = info.parent().unwrap().join(name);
        fs::write(&invalid, b"invalid metadata name").unwrap();
        item.id = invalid.into_os_string();
        assert!(
            trash_support::preflight_restore(&item, &std::sync::atomic::AtomicBool::new(false))
                .is_err()
        );
        assert!(info.exists() && payload.exists());
        assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
    }
}

#[test]
fn mixed_restore_rejects_protected_payload_before_any_item_is_restored() {
    let fixture = Fixture::new();
    let (ordinary, ordinary_info, _) = trash_item(&fixture, "ordinary");
    let (protected, protected_info, payload) = trash_item(&fixture, "protected");
    fs::remove_file(&payload).unwrap();
    fs::create_dir(&payload).unwrap();
    symlink("/proc/uptime", payload.join("kernel")).unwrap();
    let mut transfers = Transfers::default();
    assert!(transfers.enqueue(
        Kind::Restore(vec![ordinary, protected]),
        vec![ordinary_info.clone(), protected_info.clone()]
    ));
    let report = transfer_report(&mut transfers, &BackgroundJobs::new(1, 2));
    assert!(report.completed.is_empty());
    assert_eq!(report.unprocessed, 2);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
    assert!(ordinary_info.exists() && protected_info.exists());
}

#[test]
fn restore_rejects_protected_destination_ancestor_and_metadata_alias() {
    let fixture = Fixture::new();
    let (mut item, info, payload) = trash_item(&fixture, "ordinary");
    for protected_destination in [true, false] {
        if protected_destination {
            item.original_parent = PathBuf::from("/sys/nonexistent-restore-admission-folder");
        } else {
            item.original_parent = fixture.destination.clone();
            fs::remove_file(&info).unwrap();
            symlink("/proc/uptime", &info).unwrap();
        }
        assert!(
            trash_support::preflight_restore(&item, &std::sync::atomic::AtomicBool::new(false))
                .is_err()
        );
        assert!(trash_support::restore(&item).is_err());
    }
    assert_eq!(fs::read(payload).unwrap(), b"trashed");
}

#[test]
fn cancellation_during_queued_safety_check_never_submits_mutation() {
    let fixture = Fixture::new();
    let source = fixture.file("ordinary", b"kept");
    let mut pool = super::super::system_route::tests::BlockedJobs::new();
    let mut transfers = Transfers::default();
    assert!(transfers.enqueue(
        Kind::Copy(fixture.destination.clone()),
        vec![source.clone()]
    ));
    transfers.queue.back_mut().unwrap().native_drop = Some(91);
    assert!(transfers.tick(&pool.jobs, false).is_none());
    let running = transfers.running.as_ref().unwrap();
    assert!(matches!(running.admission, Admission::Checking(_)));
    assert!(running.receiver.is_none());
    running.control.cancelled.store(true, Ordering::Relaxed);
    let report = transfers.tick(&pool.jobs, false).unwrap();
    assert!(report.cancelled);
    assert_eq!(report.native_drop, Some(91));
    assert_eq!(report.unprocessed, 1);
    assert!(report.completed.is_empty());
    pool.release();
    // Drain behind the cancelled check to prove its worker has returned.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, drained) = pool.jobs.submit(Duration::MAX, || ());
        match drained.recv_timeout(Duration::from_secs(5)).unwrap() {
            Ok(()) => break,
            Err(JobError::QueueFull) => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("drain failed: {error}"),
        }
    }
    assert_eq!(fs::read(source).unwrap(), b"kept");
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 0);
    assert!(!transfers.busy());
}

#[test]
fn cancellation_after_copy_effects_preserves_delayed_mutation_completion() {
    let fixture = Fixture::new();
    let source = fixture.file("ordinary", b"copied before cancellation");
    let jobs = BackgroundJobs::new(1, 2);
    let mut running = unadmitted_copy(source.clone(), fixture.destination.clone());
    admission_result(&mut running, &jobs).unwrap();
    let batch = running.batch.clone();
    let control = running.control.clone();
    let (completed_tx, completed_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (_, receiver) = jobs.submit(Duration::MAX, move || {
        let report = batch.execute(&control);
        completed_tx.send(report.completed.len()).unwrap();
        let _ = release_rx.recv();
        report
    });
    running.receiver = Some(receiver);
    let mut transfers = Transfers {
        running: Some(running),
        ..Default::default()
    };
    assert_eq!(
        completed_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
        1
    );
    transfers
        .running
        .as_ref()
        .unwrap()
        .control
        .cancelled
        .store(true, Ordering::Relaxed);
    assert!(transfers.tick(&jobs, false).is_none());
    assert!(transfers.running.as_ref().unwrap().receiver.is_some());
    release_tx.send(()).unwrap();
    let report = transfer_report(&mut transfers, &jobs);
    assert_eq!(report.completed, vec![source]);
    assert!(report.failed.is_empty());
    assert_eq!(
        fs::read(fixture.destination.join("ordinary")).unwrap(),
        b"copied before cancellation"
    );
}
