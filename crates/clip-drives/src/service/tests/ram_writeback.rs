//! Real bounded source-filesystem failures retain edits in a separate RAM mount.
use super::*;
use std::{io::Write, sync::atomic::AtomicUsize};

fn leave_free(view: &Path, remaining: u64) -> PathBuf {
    let path = view.join("fixture-space-reservation");
    let mut fill = File::create(&path).unwrap();
    let directory = File::open(view).unwrap();
    let chunk = vec![0xa5; 1024 * 1024];
    loop {
        let stats = rustix::fs::fstatvfs(&directory).unwrap();
        let amount = stats
            .f_bavail
            .saturating_mul(stats.f_frsize)
            .saturating_sub(remaining)
            .min(chunk.len() as u64) as usize;
        if amount == 0 {
            break;
        }
        fill.write_all(&chunk[..amount]).unwrap();
    }
    fill.sync_all().unwrap();
    path
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; fills only an owned bounded filesystem"]
fn full_media_retains_ram_edits_and_paused_eject_waits_for_preflush() {
    let fixture = TestDrives::new(0);
    let (backing, source_id, native, portable) = fixture.native_source("partition");
    let unmounts = Arc::new(AtomicUsize::new(0));
    let counted = unmounts.clone();
    let allow_unmount = Arc::new(AtomicBool::new(false));
    let allowed = allow_unmount.clone();
    let (entered, release, gate) = worker_gate();
    let unmount = fixture.image_authority().unmount;
    let service = fixture.start(
        None,
        Arc::new(move |drive| {
            counted.fetch_add(1, Ordering::AcqRel);
            if !allowed.load(Ordering::Acquire) {
                return Err("Owned fixture reached unmount before a successful preflush".into());
            }
            gate();
            unmount(drive)
        }),
    );
    finish(
        &service,
        DriveRequest::CopyToRam {
            drive: "part:large".into(),
        },
    );
    let ram = status(&service).ram.into_iter().next().unwrap();
    finish(
        &service,
        DriveRequest::Pause {
            pair: ram.id.clone(),
            paused: true,
        },
    );
    // Distinct old confirmation avoids a false pass when failed and successful
    // requests finish within the timestamp's same one-second precision.
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let mut seeded: RamSession = db.get("ram", &ram.id).unwrap().unwrap();
    seeded.last_flushed = Some(1);
    db.put("ram", &ram.id, &seeded).unwrap();

    // Reserve source capacity outside the selected subtree, so it cannot be
    // mistaken for a user file to union into the separate RAM filesystem.
    let reservation = leave_free(&backing.directory.join("contents"), 1024 * 1024);
    let edits = vec![0x5a; 8 * 1024 * 1024];
    fs::write(ram.path.join("edit"), &edits).unwrap();
    File::open(ram.path.join("edit"))
        .unwrap()
        .sync_all()
        .unwrap();
    wait_ram(&service, &ram.id, |session| {
        session.pending.as_ref().unwrap().paths != 0
    });
    let confirmed = status(&service).ram[0].last_flushed;
    failed(
        &service,
        DriveRequest::FlushRam {
            session: ram.id.clone(),
        },
        "No space left",
    );
    let retained = status(&service).ram.remove(0);
    assert!(retained.browsable());
    assert!(retained.status.contains("RAM reconciliation incomplete"));
    assert!(retained.status.contains("RAM contents retained"));
    assert_eq!(retained.last_flushed, confirmed);
    assert!(retained.pending.as_ref().unwrap().paths != 0);
    assert_eq!(fs::read(ram.path.join("edit")).unwrap(), edits);
    assert_eq!(
        fs::read(portable.join("edit")).unwrap(),
        b"original portable content"
    );
    assert_eq!(
        fs::read_dir(&portable).unwrap().count(),
        1,
        "unpublished staging must be retired"
    );

    failed(
        &service,
        DriveRequest::Eject {
            drive: "part:large".into(),
        },
        "No space left",
    );
    assert_eq!(unmounts.load(Ordering::Acquire), 0);
    assert!(
        status(&service)
            .drives
            .iter()
            .find(|drive| drive.id == "part:large")
            .unwrap()
            .mount
            .is_some()
    );
    assert_eq!(status(&service).ram[0].last_flushed, confirmed);

    fs::remove_file(reservation).unwrap();
    finish(
        &service,
        DriveRequest::FlushRam {
            session: ram.id.clone(),
        },
    );
    assert_eq!(fs::read(portable.join("edit")).unwrap(), edits);
    assert!(status(&service).ram[0].last_flushed > confirmed);
    assert_eq!(status(&service).ram[0].pending.as_ref().unwrap().paths, 0);
    fs::write(ram.path.join("edit"), b"new paused edit flushed by eject").unwrap();
    wait_ram(&service, &ram.id, |session| {
        session.pending.as_ref().unwrap().paths != 0
    });
    allow_unmount.store(true, Ordering::Release);
    let eject = accept(
        &service,
        DriveRequest::Eject {
            drive: "part:large".into(),
        },
    );
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(
        fs::read(portable.join("edit")).unwrap(),
        b"new paused edit flushed by eject"
    );
    assert_unfinished(&service, &eject);
    let discard = accept(
        &service,
        DriveRequest::DiscardRam {
            session: ram.id.clone(),
            confirmed: false,
        },
    );
    assert_unfinished(&service, &discard);
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: PathBuf::new(),
            catalog: false,
            index: false,
        },
    );
    assert_eq!(finish_released(&service, &eject, release).state, "complete");
    let refusal = wait_operation(&service, &discard, |op| op.finished.is_some());
    assert_eq!(refusal.state, "failed");
    assert!(refusal.message.contains("Confirm discarding"));
    assert_eq!(unmounts.load(Ordering::Acquire), 1);
    let retained = wait_ram(&service, &ram.id, |session| !session.connected);
    assert!(retained.browsable());
    assert!(
        status(&service)
            .pairs
            .iter()
            .find(|pair| pair.id == ram.id)
            .unwrap()
            .paused
    );
    fs::write(ram.path.join("edit"), b"still editable after eject").unwrap();
    failed(
        &service,
        DriveRequest::DiscardRam {
            session: ram.id.clone(),
            confirmed: false,
        },
        "Confirm discarding",
    );
    assert_eq!(
        fs::read(ram.path.join("edit")).unwrap(),
        b"still editable after eject"
    );
    discard_ram(&service, &ram);
    service.shutdown().unwrap();
    drop(native);
    crate::ram::discard(&backing, &source_id).unwrap();
}
