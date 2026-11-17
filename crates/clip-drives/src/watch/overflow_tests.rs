//! Real kernel queue loss is generated only inside an owned disposable guest.
use super::*;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn settle(watch: &mut Watch) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let changes = watch.changes();
        if !changes.full && changes.paths.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "watch registration did not settle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn interrupted_rebuild_retains_pending_roots_and_releases_retired_scopes() {
    let temp = tempfile::tempdir().unwrap();
    let first = Arc::new(Root::create(&temp.path().join("first")).unwrap());
    let second = Arc::new(Root::create(&temp.path().join("second")).unwrap());
    let mut watch = Watch::new().unwrap();
    watch.add(first.clone());
    watch.add(second.clone());
    watch.rebuild().unwrap();
    assert!(watch.locations.is_empty());
    assert_eq!(watch.pending.len(), 2);
    watch.rebuild().unwrap();
    assert_eq!(watch.pending.len(), 2);
    watch.remove_scope(&first.scope());
    assert_eq!(Arc::strong_count(&first), 1);
    assert_eq!(watch.pending.len(), 1);
    settle(&mut watch);
    let file = second.path.join("still-observed");
    fs::write(&file, b"retained root").unwrap();
    let changes = watch.changes();
    assert!(!changes.full);
    assert!(changes.paths.contains(&file));
    watch.remove_scope(&second.scope());
    assert_eq!(Arc::strong_count(&second), 1);
}

#[test]
#[ignore = "changes inotify limits only inside a disposable guest"]
fn guest_queue_overflow_rebuilds_missed_nested_directory_watches() {
    crate::require_disposable_guest();
    let media = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").unwrap();
    assert_eq!(Path::new(&media), Path::new("/work/media"));
    // The limit is captured at instance creation. Alternating names prevent
    // consecutive identical close events from being coalesced by the kernel.
    fs::write("/proc/sys/fs/inotify/max_queued_events", b"64").unwrap();
    let temp = tempfile::tempdir_in(media).unwrap();
    fs::create_dir(temp.path().join("existing")).unwrap();
    let root = Arc::new(Root::open(temp.path()).unwrap());
    let mut watch = Watch::new().unwrap();
    watch.add(root.clone());
    settle(&mut watch);
    for generation in 0..2 {
        for index in 0..256 {
            fs::write(root.path.join(format!("noise-{}", index % 2)), b"noise").unwrap();
        }
        let directories = [
            format!("dropped-{generation}/deep"),
            format!("existing/dropped-{generation}"),
        ];
        for directory in &directories {
            fs::create_dir_all(root.path.join(directory)).unwrap();
        }
        let changes = watch.changes();
        assert!(changes.kernel_overflow, "must consume a real IN_Q_OVERFLOW");
        assert!(changes.full);
        assert!(
            changes
                .paths
                .iter()
                .all(|path| !path.to_string_lossy().contains("dropped-")),
            "the directory creation events must actually have been lost"
        );
        settle(&mut watch);
        for directory in directories {
            let file = root.path.join(directory).join("later-edit");
            fs::write(&file, b"prompt notification after lost creation").unwrap();
            let changes = watch.changes();
            assert!(!changes.full, "registration must have completed");
            assert!(
                changes.paths.contains(&file),
                "missing recovered watch: {}",
                file.display()
            );
        }
    }
    watch.remove_scope(&root.scope());
    assert_eq!(Arc::strong_count(&root), 1);
    assert!(watch.locations.is_empty());
    assert!(watch.pending.is_empty());
    assert!(watch.walking.is_none());
}
