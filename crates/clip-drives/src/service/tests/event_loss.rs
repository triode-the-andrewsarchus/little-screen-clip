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
    let _limit = Limit::set("max_user_instances", b"0");
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

struct Limit {
    path: PathBuf,
    original: Vec<u8>,
}
impl Limit {
    fn set(name: &str, value: &[u8]) -> Self {
        crate::require_disposable_guest();
        let path = Path::new("/proc/sys/fs/inotify").join(name);
        let original = fs::read(&path).unwrap();
        fs::write(&path, value).unwrap();
        Self { path, original }
    }
}
impl Drop for Limit {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, &self.original);
    }
}

#[test]
#[ignore = "internal owned daemon child of the disposable event-overflow fixture"]
fn inotify_owner_probe() {
    crate::require_disposable_guest();
    let root = PathBuf::from(std::env::var_os("LITTLE_CLIP_EVENT_LOSS_FIXTURE").unwrap());
    assert!(root.starts_with("/work/tmp"));
    let drive: Drive = serde_json::from_slice(&fs::read(root.join("drive.json")).unwrap()).unwrap();
    let selected = drive.id.clone();
    let service = Service::start_with_inventory(
        DriveConfig {
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        root.join("state"),
        Arc::new(move || Ok(vec![drive.clone()])),
    )
    .unwrap();
    finish(
        &service,
        DriveRequest::Pair {
            drive: selected.clone(),
            relative: PathBuf::new(),
            local: root.join("local"),
        },
    );
    fs::write(root.join("ready"), b"ready").unwrap();
    let started = Instant::now();
    let mut settled = false;
    while !root.join("stop").exists() {
        if !settled && root.join("settle").exists() {
            // Observe unchanged drive facts without refreshing sync watches or
            // running an explicit comparison that could conceal lost coverage.
            finish(
                &service,
                DriveRequest::Rule {
                    drive: selected.clone(),
                    relative: "settle".into(),
                    catalog: true,
                    index: true,
                },
            );
            fs::write(root.join("settled"), b"settled").unwrap();
            settled = true;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "Overflow parent did not retire its child"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    service.shutdown().unwrap();
}

#[test]
#[ignore = "changes inotify limits only inside a disposable guest"]
fn guest_event_loss_recovers_union_conflicts_and_prompt_nested_edits_without_a_client() {
    let mut drive = native_fs::guest_drive();
    let _limit = Limit::set("max_queued_events", b"64");
    let media = tempfile::tempdir_in(drive.mount.as_ref().unwrap()).unwrap();
    drive.mount = Some(media.path().into());
    fs::write(media.path().join("report.txt"), b"base").unwrap();
    fs::write(media.path().join("restore.txt"), b"restore by default").unwrap();
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("local");
    fs::write(
        temp.path().join("drive.json"),
        serde_json::to_vec(&drive).unwrap(),
    )
    .unwrap();
    let mut command = CommandRequest::new(std::env::current_exe().unwrap())
        .args([
            "service::tests::event_loss::inotify_owner_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .die_with_parent()
        .build_command();
    command.env("LITTLE_CLIP_EVENT_LOSS_FIXTURE", temp.path());
    let mut owner = crate::test_child::Owned(command.spawn().unwrap());
    wait_contents(&temp.path().join("ready"), b"ready");
    let nested = Path::new("lost-directory/deep/edit.txt");
    // No daemon thread can drain inotify while its owned process is stopped.
    // Alternating names prevent event coalescing; >64 queued mutations force
    // real kernel overflow with unchanged drive facts and a one-hour fallback.
    crate::test_child::suspended(&mut owner.0, || {
        for index in 0..256 {
            fs::write(media.path().join(format!("noise-{}", index % 2)), b"noise").unwrap();
        }
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
        fs::create_dir_all(media.path().join(nested.parent().unwrap())).unwrap();
        fs::write(media.path().join(nested), b"created after overflow").unwrap();
        fs::write(local.join("local-only.txt"), b"union from local").unwrap();
    });
    wait_contents(&local.join(nested), b"created after overflow");
    wait_contents(&media.path().join("local-only.txt"), b"union from local");
    wait_contents(&media.path().join("restore.txt"), b"restore by default");
    let db = Store::existing(&temp.path().join("state/drives.sqlite"), true).unwrap();
    let started = Instant::now();
    let conflict = loop {
        let conflicts = db.list::<DriveConflict>("conflict", 0, 100).unwrap();
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
    fs::write(temp.path().join("settle"), b"settle").unwrap();
    wait_contents(&temp.path().join("settled"), b"settled");
    fs::write(media.path().join(nested), b"later nested portable edit").unwrap();
    wait_contents(&local.join(nested), b"later nested portable edit");
    assert_eq!(
        db.list::<DriveConflict>("conflict", 0, 100).unwrap().len(),
        1
    );
    fs::write(temp.path().join("stop"), b"stop").unwrap();
    let started = Instant::now();
    loop {
        if let Some(status) = owner.0.try_wait().unwrap() {
            assert!(status.success(), "Owned overflow daemon failed: {status}");
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Overflow child did not exit"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
