//! Read-only reply handoff tests never start IPC or open the fixture device paths.
use super::*;

fn preview(name: &str) -> BackupPreview {
    BackupPreview {
        token: name.into(),
        drive: Drive {
            id: name.into(),
            label: name.into(),
            device: format!("/dev/owned-ui-{name}").into(),
            mount: Some(format!("/owned-ui/{name}").into()),
            part_uuid: Some(name.into()),
            filesystem_uuid: None,
            filesystem: "vfat".into(),
            capacity: 1_000_000,
            drive_capacity: 1_000_000,
            disk_sequence: Some(1),
            removable: true,
            read_only: false,
            evidence: name.into(),
            ambiguous: false,
        },
        kind: BackupKind::Image,
        destination: format!("/owned-ui/backups/{name}").into(),
        space: BackupSpace {
            available_bytes: 100_000_000,
            full_image_bytes: Some(68_108_864),
            btrfs: false,
        },
    }
}
fn request(name: &str) -> DriveRequest {
    DriveRequest::PreviewBackup {
        drive: name.into(),
        kind: BackupKind::Image,
        destination: Some(preview(name).destination),
    }
}

#[test]
fn an_old_preview_and_its_failure_cannot_replace_a_newer_drive_selection() {
    let mut drives = Drives::default();
    let old = request("old");
    drives.send(old.clone());
    let old_generation = drives.preview_generation;
    drives.dialog = None;
    let current = request("current");
    drives.send(current.clone());
    let generation = drives.preview_generation;
    drives.accept_reply(old_generation, old.clone(), Err("late old failure".into()));
    assert!(drives.notice.is_none());
    assert!(
        matches!(drives.dialog, Some(Dialog::BackupLoading { generation: active }) if active == generation)
    );
    drives.accept_reply(
        old_generation,
        old,
        Ok(DriveReply::BackupPreview(Box::new(preview("old")))),
    );
    assert!(
        matches!(drives.dialog, Some(Dialog::BackupLoading { generation: active }) if active == generation)
    );
    assert!(drives.queue.iter().any(|(_, request)| matches!(request, DriveRequest::DiscardBackupReview { token } if token == "old")));
    drives.accept_reply(
        generation,
        current,
        Ok(DriveReply::BackupPreview(Box::new(preview("current")))),
    );
    let Some(Dialog::BackupReview { preview, .. }) = drives.dialog else {
        panic!("current review must open");
    };
    assert_eq!(preview.drive.id, "current");
    assert_eq!(
        preview.destination,
        PathBuf::from("/owned-ui/backups/current")
    );
}

#[test]
fn cancelling_while_a_preview_is_pending_retires_its_late_reply_without_a_notice() {
    let mut drives = Drives::default();
    let request = request("cancelled");
    drives.send(request.clone());
    let generation = drives.preview_generation;
    drives.dialog = None;
    drives.accept_reply(
        generation,
        request.clone(),
        Err("late cancelled failure".into()),
    );
    assert!(drives.notice.is_none());
    drives.accept_reply(
        generation,
        request,
        Ok(DriveReply::BackupPreview(Box::new(preview("cancelled")))),
    );
    assert!(drives.dialog.is_none());
    assert!(drives.queue.iter().any(|(_, request)| matches!(request, DriveRequest::DiscardBackupReview { token } if token == "cancelled")));
    drives.accept_reply(
        generation,
        DriveRequest::DiscardBackupReview {
            token: "cancelled".into(),
        },
        Ok(DriveReply::Done("review released".into())),
    );
    assert!(drives.notice.is_none());
}

#[test]
fn a_current_preview_failure_closes_only_its_loading_dialog_and_reports_the_error() {
    let mut drives = Drives::default();
    let request = request("current");
    drives.send(request.clone());
    drives.accept_reply(
        drives.preview_generation,
        request,
        Err("destination unavailable".into()),
    );
    assert!(drives.dialog.is_none());
    assert_eq!(
        drives.notice,
        Some(("destination unavailable".into(), true))
    );
}

#[test]
fn drive_index_policy_snapshots_track_replacement_failure_and_removal() {
    use crate::file_manager::tests::{with_drive_policy, write_drive_policy};
    with_drive_policy(|root| {
        let absent = index_policy().unwrap();
        assert!(absent.excluded.is_empty());
        assert!(Arc::ptr_eq(&absent, &index_policy().unwrap()));
        write_drive_policy(vec![]);
        assert!(Arc::ptr_eq(&absent, &index_policy().unwrap()));
        let excluded = root.join("excluded");
        write_drive_policy(vec![excluded.clone()]);
        let policy = index_policy().unwrap();
        assert!(!Arc::ptr_eq(&absent, &policy));
        assert!(Arc::ptr_eq(&policy, &index_policy().unwrap()));
        write_drive_policy(vec![excluded.clone(), excluded.clone()]);
        assert!(
            Arc::ptr_eq(&policy, &index_policy().unwrap()),
            "Equivalent republished rules must not reset a live view"
        );
        assert!(index_excluded(&excluded.join("file")));
        assert!(!index_excluded(&root.join("allowed")));
        let mut drives = Drives::default();
        drives.accept_policy(Some(policy.clone()));
        assert!(std::mem::take(&mut drives.policy_changed));
        drives.accept_policy(Some(policy));
        assert!(!drives.policy_changed);
        let registry = root.join("state/index-policy.json");
        std::fs::write(&registry, b"invalid policy").unwrap();
        assert!(index_policy().is_none());
        assert!(index_excluded(&root.join("allowed")));
        drives.accept_policy(index_policy());
        assert!(std::mem::take(&mut drives.policy_changed));
        drives.accept_policy(index_policy());
        assert!(!drives.policy_changed);
        std::fs::remove_file(registry).unwrap();
        drives.accept_policy(index_policy());
        assert!(std::mem::take(&mut drives.policy_changed));
        assert!(!index_excluded(&excluded));
        let external = root.join("aliased-policy.json");
        std::fs::write(&external, br#"{"excluded":[]}"#).unwrap();
        std::os::unix::fs::symlink(&external, root.join("state/index-policy.json")).unwrap();
        assert!(
            index_policy().is_none(),
            "A policy alias must not bypass registry identity changes"
        );
        // Immutable observations cannot silently acquire the replacement's rules.
        assert!(absent.excluded.is_empty());
    });
}

#[test]
fn a_policy_poll_is_accepted_even_when_drive_status_ipc_fails() {
    let mut drives = Drives {
        last_poll: Instant::now(),
        ..Default::default()
    };
    let policy = Arc::new(IndexPolicy {
        excluded: vec!["/owned/excluded".into()],
    });
    let returned = policy.clone();
    drives.poll = Some(Task::start(&IO, Duration::from_secs(2), move |_| {
        StatusPoll {
            reply: Err("owned IPC failure".into()),
            policy: Some(returned),
        }
    }));
    let deadline = Instant::now() + Duration::from_secs(3);
    while drives.poll.is_some() {
        drives.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(drives.policy_changed);
    assert!(Arc::ptr_eq(drives.policy.as_ref().unwrap(), &policy));
    assert_eq!(drives.error.as_deref(), Some("owned IPC failure"));
}

#[test]
fn an_observed_policy_change_retires_background_views_without_dropping_shelf_items() {
    let root = tempfile::tempdir().unwrap();
    let mut manager = FileManager::new(root.path().into());
    // This test injects a completed observation; it must not start a real IPC poll.
    manager.drives.last_poll = Instant::now();
    let path = root.path().join("photo.png");
    manager.similarity_canvas.insert_hash(
        path,
        super::super::gallery::similarity::embedding::FeatureVector::DHash(42),
        1,
    );
    manager.shelf.add(
        super::super::content::Payload::Files {
            paths: vec![root.path().into()],
            cut: false,
        },
        "kept folder".into(),
    );
    manager.constellation.current_indexed_folder = Some(root.path().into());
    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (_sender, receiver) = std::sync::mpsc::channel();
    manager.constellation.active_job = Some(super::super::constellation::IndexingJob {
        folder: root.path().into(),
        cancelled: cancelled.clone(),
        receiver,
    });
    manager.drives.accept_policy(Some(Arc::new(IndexPolicy {
        excluded: vec![root.path().into()],
    })));
    manager.tick_drives();
    assert!(manager.similarity_canvas.hashes.is_empty());
    assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
    assert!(manager.constellation.active_job.is_none());
    assert_eq!(
        manager.constellation.current_indexed_folder.as_deref(),
        Some(root.path())
    );
    assert_eq!(manager.shelf.len(), 1);
    assert_eq!(manager.path(), root.path());
    // The same observation is consumed once, not on every UI frame.
    manager.similarity_canvas.insert_hash(
        root.path().join("marker"),
        super::super::gallery::similarity::embedding::FeatureVector::DHash(1),
        1,
    );
    manager.tick_drives();
    assert_eq!(manager.similarity_canvas.hashes.len(), 1);
}

#[test]
fn an_initial_empty_policy_keeps_existing_views_but_an_unreadable_policy_retires_them() {
    let mut drives = Drives::default();
    drives.accept_policy(Some(Arc::new(IndexPolicy::default())));
    assert!(!drives.policy_changed);
    drives.accept_policy(None);
    assert!(std::mem::take(&mut drives.policy_changed));
    let mut unavailable = Drives::default();
    unavailable.accept_policy(None);
    assert!(unavailable.policy_changed);
}
