//! Actual emulated USB removal during a confirmed partial native staging write.
use super::*;
use crate::root::copy_gate::Gate;
use crate::sync_edge_tests::native_fs::GuestMount;
use little_clip_core::kernel_interface::MountIdentity;

fn device(present: bool) -> PathBuf {
    let started = Instant::now();
    loop {
        let mut devices = fs::read_dir("/sys/class/block")
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_str().is_some_and(|name| name.starts_with("sd")))
            .collect::<Vec<_>>();
        if present && devices.len() == 1 {
            let name = devices.pop().unwrap();
            assert_eq!(
                fs::read_to_string(Path::new("/sys/class/block").join(&name).join("removable"))
                    .unwrap()
                    .trim(),
                "1"
            );
            return Path::new("/dev").join(name);
        }
        if !present && devices.is_empty() {
            return PathBuf::new();
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "USB presence never became {present}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires vm_usb_writeback.sh, owned virtual USB, tmpfs and packaged RAM helper"]
fn virtual_usb_removal_retains_ram_and_reconciles_after_return() {
    crate::require_disposable_guest();
    assert_eq!(
        std::env::var("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").unwrap(),
        "/work/media"
    );
    let filesystem = std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
    assert!(["vfat", "exfat", "ntfs3"].contains(&filesystem.as_str()));
    let fixture = TestDrives::new(0);
    let source = source::Portable::new(&fixture);
    Root::open(&source.path).unwrap().flush().unwrap();
    let old_mount = MountIdentity::directory(Path::new("/work/media")).unwrap();
    let native = crate::mounts::inventory()
        .unwrap()
        .into_iter()
        .find(|mount| mount.id == old_mount.mount)
        .unwrap();
    assert_eq!(
        native.filesystem, filesystem,
        "The USB fixture must use the selected kernel driver"
    );
    let drive = fixture.drives.lock().unwrap()[1].clone();
    let service = fixture.start(
        None,
        Arc::new(|_| panic!("unmount must not follow a failed USB write")),
    );
    finish(
        &service,
        DriveRequest::CopyToRam {
            drive: drive.id.clone(),
        },
    );
    let ram = status(&service).ram.pop().unwrap();
    finish(
        &service,
        DriveRequest::Pause {
            pair: ram.id.clone(),
            paused: true,
        },
    );
    // Pause does not cancel already accepted automatic work. Retire the owner
    // before introducing the controlled edit, then recover the paused copy.
    service.shutdown().unwrap();
    let service = fixture.start(None, Arc::new(|_| panic!("unexpected unmount")));
    wait_ram(&service, &ram.id, RamSession::browsable);
    source::seed_confirmation(&service, &ram);
    let edits = vec![0x6b; 8 * 1024 * 1024];
    source::write_edit(&ram, &edits);
    wait_ram(&service, &ram.id, |ram| {
        ram.pending.as_ref().unwrap().paths > 0
    });
    let mut gate = Gate::arm(
        &Root::open(&source.path).unwrap(),
        Path::new("edit"),
        1024 * 1024,
    );
    let flush = accept(
        &service,
        DriveRequest::FlushRam {
            session: ram.id.clone(),
        },
    );
    let checkpoint = gate.wait();
    assert_eq!(
        checkpoint.written,
        checkpoint.file.metadata().unwrap().len()
    );
    assert!(checkpoint.written > 0 && checkpoint.written < edits.len() as u64);
    let staged = checkpoint.path.file_name().unwrap().to_owned();
    eprintln!("USB_PARTIAL_STAGING={}/{}", checkpoint.written, edits.len());
    drop(checkpoint);
    assert_unfinished(&service, &flush);
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: PathBuf::new(),
            catalog: false,
            index: false,
        },
    );
    eprintln!("LITTLE_CLIP_USB_UNPLUG=1");
    device(false);
    fixture
        .drives
        .lock()
        .unwrap()
        .retain(|candidate| candidate.id != drive.id);
    assert_unfinished(&service, &flush);
    gate.release();
    let failed = wait_operation(&service, &flush, |op| op.finished.is_some());
    assert_eq!(failed.state, "failed", "{}", failed.message);
    assert_eq!(failed.sync_completion, Some(SyncCompletion::default()));
    eprintln!("USB_INTERRUPTED_RESULT={}", failed.message);
    let retained = wait_ram(&service, &ram.id, |ram| !ram.connected);
    assert!(retained.browsable() && retained.pending.as_ref().unwrap().paths > 0);
    assert_eq!(retained.last_flushed, Some(1));
    assert_eq!(fs::read(ram.path.join("edit")).unwrap(), edits);
    // Recovery must use the old prefix receipt even after the source changes.
    let edits = vec![0x7c; edits.len()];
    source::write_edit(&ram, &edits);
    fs::write(ram.path.join("offline-new"), b"after native removal").unwrap();
    let export = fixture.temp.path().join("export");
    finish(
        &service,
        DriveRequest::SaveRam {
            session: ram.id.clone(),
            destination: export.clone(),
        },
    );
    let exported = status(&service).ram[0].last_saved.clone().unwrap();
    assert!(exported.populated && exported.path.starts_with(&export));
    assert_eq!(fs::read(exported.path.join("edit")).unwrap(), edits);
    assert_eq!(
        fs::read(exported.path.join("offline-new")).unwrap(),
        b"after native removal"
    );
    assert_eq!(status(&service).ram[0].last_flushed, Some(1));
    service.shutdown().unwrap();
    assert_eq!(
        MountIdentity::directory(Path::new("/work/media")).unwrap(),
        old_mount
    );
    let unmounted = CommandRequest::new("/usr/bin/umount")
        .args(["--lazy", "--no-canonicalize", "/work/media"])
        .timeout(Duration::from_secs(5))
        .run()
        .unwrap();
    assert!(unmounted.status.success(), "{}", unmounted.stderr_lossy());
    eprintln!("LITTLE_CLIP_USB_REPLUG=1");
    let returned = device(true);
    let mounted = GuestMount::start(
        CommandRequest::new("/usr/bin/mount")
            .arg("--internal-only")
            .args(["-t", filesystem.as_str(), "-o", "ro"])
            .arg(&returned),
        Path::new("/work/media"),
    );
    assert_eq!(
        fs::read(source.path.join("edit")).unwrap(),
        b"original portable content"
    );
    eprintln!(
        "USB_STAGING_AFTER_RETURN={}",
        source.path.join(&staged).exists()
    );
    mounted.remount("remount,rw");
    fixture.drives.lock().unwrap().push(drive);
    let restarted = fixture.start(None, Arc::new(|_| panic!("unexpected unmount")));
    wait_ram(&restarted, &ram.id, RamSession::browsable);
    assert_eq!(fs::read(ram.path.join("edit")).unwrap(), edits);
    assert_eq!(
        fs::read(ram.path.join("offline-new")).unwrap(),
        b"after native removal"
    );
    assert_eq!(status(&restarted).ram[0].last_flushed, Some(1));
    finish(
        &restarted,
        DriveRequest::FlushRam {
            session: ram.id.clone(),
        },
    );
    assert_eq!(fs::read(source.path.join("edit")).unwrap(), edits);
    assert_eq!(
        fs::read(source.path.join("offline-new")).unwrap(),
        b"after native removal"
    );
    let leaked_to_ram = ram.path.join(&staged).exists();
    let left_on_media = source.path.join(&staged).exists();
    discard_ram(&restarted, &ram);
    restarted.shutdown().unwrap();
    assert!(
        !leaked_to_ram,
        "Unpublished staging must not become a user file in RAM"
    );
    assert!(
        !left_on_media,
        "Unpublished staging must be reconciled after reconnect"
    );
    source.finish();
    drop(mounted);
}
