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
