//! Kernel event loss must recover without a GUI or a periodic comparison.
use super::*;
use crate::sync_edge_tests::native_fs;
use std::{fs::FileTimes, io::Write, os::unix::fs::MetadataExt};

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
    let inventory_drive = drive.clone();
    let last_owner_query = Mutex::new(Instant::now());
    let inventory: Inventory = Arc::new(move || {
        if std::thread::current().name() == Some("portable-drive-owner") {
            let mut last = last_owner_query.lock().unwrap();
            let quiet = last.elapsed() >= Duration::from_secs(2);
            *last = Instant::now();
            drop(last);
            if quiet && held.swap(false, Ordering::AcqRel) {
                gate();
            }
        }
        Ok(vec![inventory_drive.clone()])
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
    // Wait through another unchanged inventory tick, allowing notifications
    // produced by the recovery itself to settle before the later nested edit.
    armed.store(true, Ordering::Release);
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    cleanup_release.send(()).unwrap();
    fs::write(media.path().join(nested), b"later nested portable edit").unwrap();
    wait_contents(&local.join(nested), b"later nested portable edit");
    assert_eq!(status(&service).conflicts.len(), 1);
    service.shutdown().unwrap();
}
