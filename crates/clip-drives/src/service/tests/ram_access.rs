//! An owned e2fsck wrapper holds real recovery before the RAM server is mounted.
use super::*;

struct Release(PathBuf);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"release");
    }
}

#[test]
#[ignore = "internal child of held_ram_recovery_keeps_controls_and_sync_live"]
fn ram_access_owner_probe() {
    let control = PathBuf::from(std::env::var_os("LITTLE_CLIP_RAM_ACCESS_FIXTURE").unwrap());
    for outcome in [
        "success",
        "cancel",
        "policy",
        "destination",
        "sync",
        "stalled",
    ] {
        run_recovery_case(&control, outcome);
    }
}

fn run_recovery_case(control: &Path, outcome: &str) {
    let fixture = TestDrives::new(0);
    let (mut backing, id, native, view) = fixture.native_source("");
    backing.mounted = false;
    backing.initialized = true;
    let identity = kernel_interface::MountIdentity::directory(&view).unwrap();
    let local_identity = Root::open(&view).unwrap().storage_identity().unwrap();
    let session = RamSession {
        id: id.clone(),
        drive: "part:large".into(),
        path: view.clone(),
        identity,
        ready: true,
        connected: false,
        status: "Retained owned image".into(),
        backing: Some(backing.clone()),
        last_saved: None,
        save_status: None,
        last_flushed: Some(1),
        pending: None,
    };
    let pair = SyncPair {
        id: id.clone(),
        drive: session.drive.clone(),
        evidence: "large".into(),
        relative: PathBuf::new(),
        local: view.clone(),
        local_identity: Some(local_identity),
        paused: true,
        deletions: false,
        ram: true,
        last_success: Some(1),
        status: "Paused".into(),
    };
    drop(native);
    let mut output = (outcome == "stalled").then(|| fixture.native_source("export-base"));
    if outcome == "sync" {
        fixture.drives.lock().unwrap()[1].mount = Some(fixture.temp.path().join("large"));
    } else {
        fixture
            .drives
            .lock()
            .unwrap()
            .retain(|drive| drive.id == "part:small");
    }
    let service = fixture.start_configured(
        DriveConfig {
            ram_min_available: 0,
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        None,
        fixture.image_authority(),
    );
    let small = fixture.pair(&service, "small", "");
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    db.put("ram", &id, &session).unwrap();
    db.put("pair", &id, &pair).unwrap();
    let destination = output.as_ref().map_or_else(
        || fixture.temp.path().join("not-created/export"),
        |(_, _, _, path)| path.join("missing/export"),
    );
    let _ = fs::remove_file(control.join("release"));
    let _ = fs::remove_file(control.join("entered"));
    fs::write(control.join("armed"), b"arm").unwrap();
    let mut release = Some(Release(control.join("release")));
    let request = if outcome == "sync" {
        DriveRequest::FlushRam {
            session: id.clone(),
        }
    } else {
        DriveRequest::SaveRam {
            session: id.clone(),
            destination: destination.clone(),
        }
    };
    let operation = accept(&service, request);
    wait_contents(&control.join("entered"), b"ready");
    assert_unfinished(&service, &operation);
    assert!(!destination.exists());
    assert_configuration_responsive(&service);
    fs::write(small.join("independent"), b"sync while RAM recovery waits").unwrap();
    wait_contents(
        &fixture.temp.path().join("small/independent"),
        b"sync while RAM recovery waits",
    );
    let mut closing = None;
    if let Some((_, _, native, _)) = &mut output {
        closing = Some(stall_export(
            &fixture,
            &service,
            &small,
            native,
            release.take(),
            &operation,
        ));
    } else if outcome == "cancel" {
        service
            .request(DriveRequest::Cancel {
                operation: operation.clone(),
            })
            .unwrap();
        assert_unfinished(&service, &operation);
        closing = Some(shutdown_while_held(&service));
    } else if outcome == "policy" {
        let mut config = status(&service).config;
        config.backup_directory = Some(fixture.temp.path().join("changed-backups"));
        finish(&service, DriveRequest::Configure { config });
    } else if outcome == "destination" {
        fs::create_dir(destination.parent().unwrap()).unwrap();
    }
    drop(release);
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    let retained = verify_result(&fixture, &service, &id, &destination, outcome, &operation);
    if outcome != "cancel" && outcome != "stalled" {
        assert!(
            retained.browsable(),
            "Recovered helper was not retained: {retained:?}"
        );
        assert_eq!(
            fs::read(view.join("edit")).unwrap(),
            b"original portable content"
        );
        discard_ram(&service, &retained);
        service.shutdown().unwrap();
    } else {
        // Shutdown must wait for recovery, adopt its helper, then close it.
        let helper = crate::ram::Mounted::start(&backing, &id, false, None).unwrap();
        assert_eq!(
            fs::read(view.join("edit")).unwrap(),
            b"original portable content"
        );
        drop(helper);
        crate::ram::discard(&backing, &id).unwrap();
    }
    if let Some((backing, id, native, path)) = output {
        assert!(
            !path.join("missing").exists(),
            "Cancelled export created its output tree"
        );
        drop(native);
        crate::ram::discard(&backing, &id).unwrap();
    }
    fs::remove_file(control.join("armed")).unwrap();
}

fn stall_export(
    fixture: &TestDrives,
    service: &Service,
    small: &Path,
    native: &mut crate::ram::Mounted,
    release: Option<Release>,
    operation: &str,
) -> (std::thread::JoinHandle<Result<()>>, mpsc::Receiver<()>) {
    let mut closing = None;
    native.with_suspended(|| {
        drop(release);
        let (tid, wait) = wait_fuse_request(|| worker_threads("drive-ram-setup"));
        eprintln!("RAM_EXPORT_PREPARATION_FUSE_BLOCKED tid={tid} wchan={wait}");
        assert_configuration_responsive(service);
        fs::write(
            small.join("after-recovery"),
            b"sync during stopped export preparation",
        )
        .unwrap();
        wait_contents(
            &fixture.temp.path().join("small/after-recovery"),
            b"sync during stopped export preparation",
        );
        service
            .request(DriveRequest::Cancel {
                operation: operation.into(),
            })
            .unwrap();
        assert_unfinished(service, operation);
        closing = Some(shutdown_while_held(service));
    });
    closing.unwrap()
}

fn verify_result(
    fixture: &TestDrives,
    service: &Service,
    id: &str,
    destination: &Path,
    outcome: &str,
    operation: &str,
) -> RamSession {
    let result = wait_operation(service, operation, |op| op.finished.is_some());
    let retained = status(service)
        .ram
        .into_iter()
        .find(|s| s.id == id)
        .unwrap();
    if outcome != "sync" {
        assert_eq!(retained.last_flushed, Some(1));
    }
    match outcome {
        "success" => {
            assert_eq!(result.state, "complete", "{}", result.message);
            let saved = retained.last_saved.as_ref().unwrap();
            assert_eq!(
                fs::read(saved.path.join("edit")).unwrap(),
                b"original portable content"
            );
            assert!(saved.populated);
        }
        "cancel" | "stalled" => {
            assert_eq!(result.state, "cancelled", "{}", result.message);
            assert!(retained.last_saved.is_none());
        }
        "sync" => {
            assert_eq!(result.state, "complete", "{}", result.message);
            assert_ne!(retained.last_flushed, Some(1));
            assert!(retained.last_saved.is_none());
            assert_eq!(
                fs::read(fixture.temp.path().join("large/edit")).unwrap(),
                b"original portable content"
            );
        }
        _ => {
            assert_eq!(result.state, "failed", "{}", result.message);
            assert!(
                result.message.contains(if outcome == "destination" {
                    "RAM save destination changed since scheduling admission"
                } else {
                    "RAM preparation inputs changed"
                }),
                "{}",
                result.message
            );
            assert!(retained.last_saved.is_none());
            assert!(
                !destination.exists()
                    || fs::read_dir(destination).unwrap().all(|entry| fs::read_dir(
                        entry.unwrap().path()
                    )
                    .unwrap()
                    .count()
                        == 0)
            );
        }
    }
    retained
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
fn held_ram_recovery_keeps_controls_and_sync_live() {
    let temp = tempfile::tempdir().unwrap();
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("e2fsck"))
        .find(|path| path.is_file())
        .unwrap();
    let wrapper = temp.path().join("e2fsck");
    fs::write(&wrapper, b"#!/bin/sh\nif [ -f \"$LITTLE_CLIP_RAM_ACCESS_FIXTURE/armed\" ]; then\n printf ready > \"$LITTLE_CLIP_RAM_ACCESS_FIXTURE/entered\"\n while [ ! -f \"$LITTLE_CLIP_RAM_ACCESS_FIXTURE/release\" ]; do sleep 0.02; done\nfi\nexec \"$LITTLE_CLIP_RAM_ACCESS_E2FSCK\" \"$@\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let mut paths = vec![temp.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut command = CommandRequest::new(std::env::current_exe().unwrap())
        .args([
            "service::tests::ram_access::ram_access_owner_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .die_with_parent()
        .build_command();
    command
        .env("LITTLE_CLIP_RAM_ACCESS_FIXTURE", temp.path())
        .env("LITTLE_CLIP_RAM_ACCESS_E2FSCK", real)
        .env("PATH", std::env::join_paths(paths).unwrap());
    let mut child = crate::test_child::Owned(command.spawn().unwrap());
    let started = Instant::now();
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "Owned RAM recovery probe timed out"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
