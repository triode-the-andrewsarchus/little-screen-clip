//! Kernel event loss must recover without a GUI or a periodic comparison.
use super::*;
use crate::sync_edge_tests::native_fs;
use std::{fs::FileTimes, io::Write, os::unix::fs::MetadataExt};

fn inotify_instances() -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|entry| fs::read_link(entry.unwrap().path()).ok())
        .filter(|target| target == Path::new("anon_inode:inotify"))
        .count()
}

#[test]
#[ignore = "changes inotify limits only inside a disposable guest"]
fn guest_unavailable_watcher_uses_periodic_fallback_and_recovers_when_capacity_returns() {
    use std::sync::atomic::AtomicU64;
    let mut drive = native_fs::guest_drive();
    let media = tempfile::tempdir_in(drive.mount.as_ref().unwrap()).unwrap();
    drive.mount = Some(media.path().into());
    fs::write(media.path().join("edit"), b"initial bytes").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("local");
    let queries = Arc::new(AtomicU64::new(0));
    let observed = queries.clone();
    let selector = drive.id.clone();
    fs::write("/proc/sys/fs/inotify/max_user_instances", b"0").unwrap();
    assert_eq!(inotify_instances(), 0);
    let service = Service::start_with_inventory(
        DriveConfig {
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        temp.path().join("state"),
        Arc::new(move || {
            if std::thread::current().name().is_some_and(|name| {
                name == "portable-drive-owner" || name.starts_with("drive-sync-")
            }) {
                observed.fetch_add(1, Ordering::Relaxed);
            }
            Ok(vec![drive.clone()])
        }),
    )
    .unwrap();
    finish(
        &service,
        DriveRequest::Pair {
            drive: selector,
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    assert_eq!(inotify_instances(), 0);
    wait_contents(&local.join("edit"), b"initial bytes");
    // Permit the single startup fallback to finish, then reject the former
    // every-200ms complete-scan loop. One pending completion/query is allowed.
    std::thread::sleep(Duration::from_secs(1));
    let before = queries.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        queries.load(Ordering::Relaxed) - before <= 2,
        "missing inotify coverage caused continuously repeated full comparisons"
    );
    fs::write("/proc/sys/fs/inotify/max_user_instances", b"128").unwrap();
    let mut config = status(&service).config;
    config.sync_interval_seconds = 1;
    finish(
        &service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    let started = Instant::now();
    while inotify_instances() == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "inotify instance was never retried"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(inotify_instances(), 1);
    let pair = status(&service).pairs.pop().unwrap();
    finish(&service, DriveRequest::Sync { pair: pair.id });
    config.sync_interval_seconds = 3600;
    finish(&service, DriveRequest::Configure { config });
    fs::write(media.path().join("edit"), b"event after watcher recovery").unwrap();
    wait_contents(&local.join("edit"), b"event after watcher recovery");
    service.shutdown().unwrap();
    assert_eq!(inotify_instances(), 0);
}

#[test]
#[ignore = "changes inotify limits only inside a disposable guest"]
fn guest_event_loss_recovers_union_conflicts_and_prompt_nested_edits_without_a_client() {
    let mut drive = native_fs::guest_drive();
    fs::write("/proc/sys/fs/inotify/max_queued_events", b"64").unwrap();
    let media = tempfile::tempdir_in(drive.mount.as_ref().unwrap()).unwrap();
    drive.mount = Some(media.path().into());
    fs::write(media.path().join("report.txt"), b"base").unwrap();
    fs::write(media.path().join("restore.txt"), b"restore by default").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("local");
    let armed = Arc::new(AtomicBool::new(false));
    let held = armed.clone();
    let (entered, release, gate) = worker_gate();
    let gate_source = temp.path().join("gate-source");
    fs::create_dir(&gate_source).unwrap();
    let gate_drive = fixture_drive(&gate_source, "overflow-owner-gate");
    let inventory_drive = drive.clone();
    let inventory: Inventory = Arc::new(move || {
        if std::thread::current().name() == Some("portable-drive-owner")
            && held.swap(false, Ordering::AcqRel)
        {
            gate();
        }
        Ok(vec![inventory_drive.clone(), gate_drive.clone()])
    });
    let service = Service::start_with_inventory(
        DriveConfig {
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        temp.path().join("state"),
        inventory,
    )
    .unwrap();
    // Release the held owner before Service joins it if an assertion fails.
    let cleanup_release = release;
    finish(
        &service,
        DriveRequest::Pair {
            drive: drive.id.clone(),
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    armed.store(true, Ordering::Release);
    // Explicit enrollment still has a coordinator-owned stage. Hold that stage
    // on a separate ordinary fixture so the native watcher truly stops draining;
    // background sync inventory no longer provides a coordinator barrier.
    let first_gate = accept(
        &service,
        DriveRequest::Pair {
            drive: "part:overflow-owner-gate".into(),
            relative: PathBuf::new(),
            local: temp.path().join("gate-local-one"),
        },
    );
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    for index in 0..256 {
        fs::write(media.path().join(format!("noise-{}", index % 2)), b"noise").unwrap();
    }
    // This inventory callback returns exactly the same drive facts. Recovery
    // therefore cannot be explained by insertion/removal or the one-hour timer.
    for (root, bytes) in [(media.path(), b"card"), (local.as_path(), b"host")] {
        let mut file = File::options()
            .read(true)
            .write(true)
            .open(root.join("report.txt"))
            .unwrap();
        let before = file.metadata().unwrap();
        file.write_all(bytes).unwrap();
        file.set_times(FileTimes::new().set_modified(before.modified().unwrap()))
            .unwrap();
        let after = file.metadata().unwrap();
        assert_eq!(
            (after.len(), after.mtime(), after.mtime_nsec()),
            (before.len(), before.mtime(), before.mtime_nsec())
        );
    }
    fs::remove_file(media.path().join("restore.txt")).unwrap();
    let nested = Path::new("lost-directory/deep/edit.txt");
    fs::create_dir_all(media.path().join(nested.parent().unwrap())).unwrap();
    fs::write(media.path().join(nested), b"created after overflow").unwrap();
    fs::write(local.join("local-only.txt"), b"union from local").unwrap();
    cleanup_release.send(()).unwrap();
    assert_eq!(
        wait_operation(&service, &first_gate, |op| op.finished.is_some()).state,
        "complete"
    );
    wait_contents(&local.join(nested), b"created after overflow");
    wait_contents(&media.path().join("local-only.txt"), b"union from local");
    wait_contents(&media.path().join("restore.txt"), b"restore by default");
    let started = Instant::now();
    let conflict = loop {
        let conflicts = status(&service).conflicts;
        if let Some(conflict) = conflicts.into_iter().find(|conflict| conflict.complete) {
            break conflict;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "missing preserved conflict"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!conflict.resolved);
    for root in [media.path(), local.as_path()] {
        wait_contents(&root.join(&conflict.portable), b"card");
        wait_contents(&root.join(&conflict.local), b"host");
    }
    assert_eq!(fs::read(media.path().join("report.txt")).unwrap(), b"card");
    assert_eq!(fs::read(local.join("report.txt")).unwrap(), b"host");
    // Wait through another unchanged explicit inventory observation, allowing notifications
    // produced by the recovery itself to settle before the later nested edit.
    armed.store(true, Ordering::Release);
    let second_gate = accept(
        &service,
        DriveRequest::Pair {
            drive: "part:overflow-owner-gate".into(),
            relative: PathBuf::new(),
            local: temp.path().join("gate-local-two"),
        },
    );
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    cleanup_release.send(()).unwrap();
    assert_eq!(
        wait_operation(&service, &second_gate, |op| op.finished.is_some()).state,
        "complete"
    );
    fs::write(media.path().join(nested), b"later nested portable edit").unwrap();
    wait_contents(&local.join(nested), b"later nested portable edit");
    assert_eq!(status(&service).conflicts.len(), 1);
    service.shutdown().unwrap();
}
