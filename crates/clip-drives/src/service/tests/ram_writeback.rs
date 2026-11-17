//! Real bounded source-filesystem failures retain edits in a separate RAM mount.
use super::*;
use std::{
    io::{Read, Write},
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::atomic::AtomicUsize,
};

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

#[test]
#[ignore = "requires tmpfs, /dev/fuse and LITTLE_CLIP_TEST_RAM_GATE_HELPER built with --test-io-gate"]
fn removing_media_during_a_partial_write_retains_ram_and_queued_operation_results() {
    for cancel in [false, true] {
        eprintln!("partial-write removal, cancel={cancel}");
        let fixture = TestDrives::new(0);
        let (mut control, child) = UnixStream::pair().unwrap();
        control
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        control
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let gate = File::from(OwnedFd::from(child));
        let (backing, source_id, native, portable) = fixture
            .native_source_with("", |backing, id| {
                crate::ram::Mounted::start_gated(backing, id, &gate).unwrap()
            });
        let drive = fixture.drives.lock().unwrap()[1].clone();
        let service = fixture.start(
            None,
            Arc::new(|_| Err("unmount must not run after removal".into())),
        );
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: drive.id.clone(),
            },
        );
        let ram = status(&service).ram.remove(0);
        finish(
            &service,
            DriveRequest::Pause {
                pair: ram.id.clone(),
                paused: true,
            },
        );
        let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
        let mut retained: RamSession = db.get("ram", &ram.id).unwrap().unwrap();
        retained.last_flushed = Some(1);
        db.put("ram", &ram.id, &retained).unwrap();
        let edits = vec![0x6b; 8 * 1024 * 1024];
        fs::write(ram.path.join("edit"), &edits).unwrap();
        wait_ram(&service, &ram.id, |session| {
            session.pending.as_ref().unwrap().paths != 0
        });
        control.write_all(b"A").unwrap();
        let flush = accept(
            &service,
            DriveRequest::FlushRam {
                session: ram.id.clone(),
            },
        );
        let mut progress = [0; 8];
        control.read_exact(&mut progress).unwrap();
        let stored = u64::from_le_bytes(progress);
        eprintln!(
            "stored {stored}/{} staged bytes before removal",
            edits.len()
        );
        assert!(
            stored > 0 && stored < edits.len() as u64,
            "expected actual partial staging: {stored}"
        );
        assert_unfinished(&service, &flush);
        if cancel {
            service
                .request(DriveRequest::Cancel {
                    operation: flush.clone(),
                })
                .unwrap();
            assert_unfinished(&service, &flush);
        }
        let eject = accept(
            &service,
            DriveRequest::Eject {
                drive: drive.id.clone(),
            },
        );
        let discard = accept(
            &service,
            DriveRequest::DiscardRam {
                session: ram.id.clone(),
                confirmed: false,
            },
        );
        finish(
            &service,
            DriveRequest::Rule {
                drive: "part:small".into(),
                relative: PathBuf::new(),
                catalog: false,
                index: false,
            },
        );
        assert_unfinished(&service, &eject);
        assert_unfinished(&service, &discard);

        // Detach only this owned mount while the application still waits for its
        // partial FUSE write. The socket then returns an actual EIO to that write.
        let detached = CommandRequest::new("fusermount3")
            .args(["-uz"])
            .arg(&portable)
            .timeout(Duration::from_secs(5))
            .max_output_bytes(4096)
            .run()
            .unwrap();
        assert!(detached.status.success(), "{}", detached.stderr_lossy());
        fixture
            .drives
            .lock()
            .unwrap()
            .retain(|candidate| candidate.id != drive.id);
        control.write_all(b"F").unwrap();
        let failed_flush = wait_operation(&service, &flush, |op| op.finished.is_some());
        assert_eq!(
            failed_flush.state,
            if cancel { "cancelled" } else { "failed" }
        );
        assert!(
            failed_flush.message.contains("Input/output error"),
            "{}",
            failed_flush.message
        );
        assert_eq!(
            wait_operation(&service, &eject, |op| op.finished.is_some()).state,
            "failed"
        );
        let refused = wait_operation(&service, &discard, |op| op.finished.is_some());
        assert_eq!(refused.state, "failed");
        assert!(refused.message.contains("Confirm discarding"));
        let retained = status(&service).ram.remove(0);
        assert!(retained.browsable());
        assert_eq!(retained.last_flushed, Some(1));
        assert!(retained.pending.as_ref().unwrap().paths != 0);
        assert_eq!(fs::read(ram.path.join("edit")).unwrap(), edits);
        fs::write(ram.path.join("offline-new"), b"accepted after removal").unwrap();
        drop(native);

        let recovered = crate::ram::Mounted::start(&backing, &source_id, false, None).unwrap();
        assert_eq!(
            fs::read(portable.join("edit")).unwrap(),
            b"original portable content"
        );
        assert_eq!(
            fs::read_dir(&portable).unwrap().count(),
            1,
            "partial staging was not cleaned up"
        );
        fs::write(portable.join("edit"), b"portable changed while detached").unwrap();
        fixture.drives.lock().unwrap().push(drive.clone());
        finish(
            &service,
            DriveRequest::FlushRam {
                session: ram.id.clone(),
            },
        );
        let conflict = status(&service)
            .conflicts
            .into_iter()
            .find(|conflict| conflict.pair == ram.id)
            .unwrap();
        for root in [&portable, &ram.path] {
            assert_eq!(
                fs::read(root.join(&conflict.portable)).unwrap(),
                b"portable changed while detached"
            );
            assert_eq!(fs::read(root.join(&conflict.local)).unwrap(), edits);
            assert_eq!(
                fs::read(root.join("offline-new")).unwrap(),
                b"accepted after removal"
            );
        }
        assert!(status(&service).ram[0].last_flushed > Some(1));
        // Conflict installation itself produces newer watch events. Resume
        // automatic catch-up instead of treating them as covered by an older checkpoint.
        let mut config = status(&service).config;
        config.sync_interval_seconds = 1;
        finish(&service, DriveRequest::Configure { config });
        finish(
            &service,
            DriveRequest::Pause {
                pair: ram.id.clone(),
                paused: false,
            },
        );
        wait_ram(&service, &ram.id, |session| {
            session
                .pending
                .as_ref()
                .is_some_and(|pending| pending.paths == 0 && !pending.full_check_required)
        });
        assert_eq!(
            wait_operation(&service, &flush, |op| op.finished.is_some()).state,
            failed_flush.state
        );
        discard_ram(&service, &ram);
        service.shutdown().unwrap();
        drop(recovered);
        crate::ram::discard(&backing, &source_id).unwrap();
    }
}
