//! Owned helper gates exercise actual mount command and shutdown lifetimes.
use super::*;

#[test]
fn startup_catalog_cleanup_visits_saved_headers_beyond_status_pagination() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    finish(
        &service,
        DriveRequest::Catalog {
            drive: "part:large".into(),
        },
    );
    let catalog = status(&service).catalogs.pop().unwrap();
    service.shutdown().unwrap();
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let transaction = db.0.unchecked_transaction().unwrap();
    for number in 0..1101 {
        let mut row = catalog.clone();
        row.id = format!("older-{number:04}");
        db.put("catalog", &row.id, &row).unwrap();
    }
    let mut retained = catalog.clone();
    retained.id = "retained-after-page".into();
    retained.mount = Some(service.state.join("mounts").join(&retained.id));
    db.put("catalog", &retained.id, &retained).unwrap();
    db.put("catalog-mount", &retained.id, &1_u64).unwrap();
    // Newer rows put the retained boundary beyond the status window too.
    for number in 0..1101 {
        let mut row = catalog.clone();
        row.id = format!("newer-{number:04}");
        db.put("catalog", &row.id, &row).unwrap();
    }
    transaction.commit().unwrap();
    assert!(
        db.list::<Catalog>("catalog", 0, 1000)
            .unwrap()
            .iter()
            .all(|row| row.id != retained.id)
    );
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    assert_configuration_responsive(&service);
    let started = Instant::now();
    while db
        .get::<Catalog>("catalog", &retained.id)
        .unwrap()
        .unwrap()
        .mount
        .is_some()
    {
        assert!(started.elapsed() < Duration::from_secs(25));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        db.get::<u64>("catalog-mount", &retained.id)
            .unwrap()
            .is_none()
    );
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires owned FUSE RAM helper; suspends only its own mountpoint parent"]
fn stopped_catalog_mountpoint_keeps_controls_sync_and_actual_shutdown_owned() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let local = fixture.pair(&service, "small", "");
    finish(
        &service,
        DriveRequest::Catalog {
            drive: "part:large".into(),
        },
    );
    let catalog = status(&service).catalogs.pop().unwrap();
    let (backing, id, mut native, view) = fixture.native_source("catalog-target");
    std::os::unix::fs::symlink(&view, service.state.join("mounts")).unwrap();
    let mut operation = None;
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        native.with_suspended(|| {
            let accepted = accept(
                &service,
                DriveRequest::MountCatalog {
                    catalog: catalog.id.clone(),
                },
            );
            operation = Some(accepted.clone());
            let (tid, wait) = wait_fuse_request(|| worker_threads("drive-catalog"));
            eprintln!("CATALOG_TARGET_FUSE_BLOCKED tid={tid} wchan={wait}");
            assert_configuration_responsive(&service);
            fs::write(
                local.join("independent"),
                b"sync during catalog target preparation",
            )
            .unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during catalog target preparation",
            );
            service
                .request(DriveRequest::Cancel {
                    operation: accepted.clone(),
                })
                .unwrap();
            assert_unfinished(&service, &accepted);
            closing = Some(shutdown_while_held(&service));
        });
    }));
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    service.shutdown().unwrap();
    if result.is_ok() {
        let operation = wait_operation(&service, operation.as_ref().unwrap(), |op| {
            op.finished.is_some()
        });
        assert_eq!(operation.state, "cancelled", "{}", operation.message);
        let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
        assert!(
            db.get::<u64>("catalog-mount", &catalog.id)
                .unwrap()
                .is_none()
        );
        assert!(
            db.get::<Catalog>("catalog", &catalog.id)
                .unwrap()
                .unwrap()
                .mount
                .is_none()
        );
    }
    finish_native_fixture(backing, id, native, result);
}

#[test]
#[ignore = "internal owned process of retained_catalog_mount_commands_keep_controls_and_sync_live"]
fn catalog_control_owner_probe() {
    let control = PathBuf::from(std::env::var_os("LITTLE_CLIP_CATALOG_CONTROL_FIXTURE").unwrap());
    for case in [
        "mount",
        "mount-shutdown",
        "mount-changed",
        "mount-publication",
        "unmount",
        "unmount-shutdown",
        "startup",
        "startup-shutdown",
        "startup-orphan",
    ] {
        run_case(&control, case);
    }
}

#[test]
#[ignore = "requires /dev/fuse and the packaged catalog helper; owns all subprocesses and mounts"]
fn retained_catalog_mount_commands_keep_controls_and_sync_live() {
    let temp = tempfile::tempdir().unwrap();
    let real_unmount = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join("fusermount3"))
        .find(|path| path.is_file())
        .unwrap();
    let real_helper = std::env::var_os("LITTLE_CLIP_DRIVE_MOUNT_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("clip-drive-mount")
        });
    let gate = "if [ \"$(cat \"$LITTLE_CLIP_CATALOG_CONTROL_FIXTURE/armed\" 2>/dev/null)\" = TAG ]; then\n printf ready > \"$LITTLE_CLIP_CATALOG_CONTROL_FIXTURE/entered\"\n while [ ! -f \"$LITTLE_CLIP_CATALOG_CONTROL_FIXTURE/release\" ]; do sleep 0.02; done\nfi\n";
    for (file, tag, program) in [
        ("catalog-helper", "mount", "LITTLE_CLIP_CATALOG_REAL_HELPER"),
        ("fusermount3", "unmount", "LITTLE_CLIP_CATALOG_REAL_UNMOUNT"),
    ] {
        let path = temp.path().join(file);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\n{}exec \"${program}\" \"$@\"\n",
                gate.replace("TAG", tag)
            ),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut paths = vec![temp.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut command = CommandRequest::new(std::env::current_exe().unwrap())
        .args([
            "service::tests::catalog_controls::catalog_control_owner_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .die_with_parent()
        .build_command();
    command
        .env("LITTLE_CLIP_CATALOG_CONTROL_FIXTURE", temp.path())
        .env("LITTLE_CLIP_CATALOG_REAL_HELPER", real_helper)
        .env("LITTLE_CLIP_CATALOG_REAL_UNMOUNT", real_unmount)
        .env(
            "LITTLE_CLIP_DRIVE_MOUNT_HELPER",
            temp.path().join("catalog-helper"),
        )
        .env("PATH", std::env::join_paths(paths).unwrap());
    let mut child = crate::test_child::Owned(command.spawn().unwrap());
    assert!(child.wait(Duration::from_secs(120)).success());
}

fn run_case(control: &Path, case: &str) {
    let fixture = TestDrives::new(0);
    let _media = fixture.guest_source();
    let source = fixture.drives.lock().unwrap()[1].mount.clone().unwrap();
    fs::write(source.join("catalog-source"), b"catalog source").unwrap();
    let mut service = fixture.start_observed_reads(Arc::new(|| {}));
    let local = fixture.pair(&service, "small", "");
    finish(
        &service,
        DriveRequest::Catalog {
            drive: "part:large".into(),
        },
    );
    let mut catalog = status(&service).catalogs.pop().unwrap();
    let view = service.state.join("mounts").join(&catalog.id);
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let mut external = None;
    if case.starts_with("startup") {
        external = Some(prepare_startup(
            &service,
            &mut catalog,
            &db,
            &view,
            case == "startup-orphan",
        ));
    } else if case.starts_with("unmount") {
        finish(
            &service,
            DriveRequest::MountCatalog {
                catalog: catalog.id.clone(),
            },
        );
    }
    let _ = fs::remove_file(control.join("release"));
    let _ = fs::remove_file(control.join("entered"));
    fs::write(
        control.join("armed"),
        if case.starts_with("mount") {
            "mount"
        } else {
            "unmount"
        },
    )
    .unwrap();
    let mut operation = None;
    let mut closing = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if case.starts_with("startup") {
            let started = Instant::now();
            service = fixture.start_observed_reads(Arc::new(|| {}));
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "Startup waited for catalog cleanup"
            );
        } else {
            operation = Some(accept(
                &service,
                if case.starts_with("mount") {
                    DriveRequest::MountCatalog {
                        catalog: catalog.id.clone(),
                    }
                } else {
                    DriveRequest::UnmountCatalog {
                        catalog: catalog.id.clone(),
                    }
                },
            ));
        }
        wait_contents(&control.join("entered"), b"ready");
        eprintln!("CATALOG_CONTROL_HELD case={case}");
        if let Some(id) = &operation {
            assert_unfinished(&service, id);
        }
        assert_configuration_responsive(&service);
        fs::write(local.join("independent"), case.as_bytes()).unwrap();
        wait_contents(
            &fixture.temp.path().join("small/independent"),
            case.as_bytes(),
        );
        if case == "mount-changed" {
            catalog.label = "Changed while helper starts".into();
            db.put("catalog", &catalog.id, &catalog).unwrap();
        } else if case == "mount-publication" {
            db.0.execute_batch("CREATE TRIGGER mount_publication_fault BEFORE INSERT ON entities WHEN NEW.kind='catalog-mount' BEGIN SELECT RAISE(ABORT,'owned catalog publication fault'); END;").unwrap();
        }
        if case.contains("shutdown") {
            closing = Some(shutdown_while_held(&service));
        } else if case == "unmount" {
            service
                .request(DriveRequest::Cancel {
                    operation: operation.clone().unwrap(),
                })
                .unwrap();
            assert_unfinished(&service, operation.as_ref().unwrap());
        }
    }));
    fs::write(control.join("release"), b"release").unwrap();
    if let Some(closing) = closing {
        finish_held_shutdown(closing);
    }
    if result.is_ok() {
        verify_result(&service, &db, &catalog.id, case, operation.as_deref());
    }
    service.shutdown().unwrap();
    assert!(
        crate::mounts::catalog_mount(&view, &catalog.id)
            .unwrap()
            .is_none()
    );
    drop(external);
    fs::remove_file(control.join("armed")).unwrap();
    result.unwrap();
}

fn prepare_startup(
    service: &Service,
    catalog: &mut Catalog,
    db: &Store,
    view: &Path,
    orphan: bool,
) -> crate::test_child::Owned {
    service.shutdown().unwrap();
    fs::create_dir_all(view).unwrap();
    let child = CommandRequest::new(std::env::var_os("LITTLE_CLIP_CATALOG_REAL_HELPER").unwrap())
        .arg(&service.state)
        .arg(&catalog.id)
        .arg(view)
        .die_with_parent()
        .build_command()
        .spawn()
        .unwrap();
    let external = crate::test_child::Owned(child);
    let started = Instant::now();
    let observed = loop {
        if let Some(mount) = crate::mounts::catalog_mount(view, &catalog.id).unwrap() {
            break mount;
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    };
    if !orphan {
        catalog.mount = Some(view.to_path_buf());
        db.put("catalog", &catalog.id, catalog).unwrap();
        db.put("catalog-mount", &catalog.id, &observed.id).unwrap();
    }
    external
}

fn verify_result(
    service: &Service,
    db: &Store,
    catalog: &str,
    case: &str,
    operation: Option<&str>,
) {
    let view = service.state.join("mounts").join(catalog);
    if let Some(id) = operation {
        let completed = wait_operation(service, id, |op| op.finished.is_some());
        if case == "mount-changed" || case == "mount-publication" {
            assert_eq!(completed.state, "failed", "{}", completed.message);
            assert!(
                completed.message.contains(if case == "mount-changed" {
                    "Catalog changed"
                } else {
                    "owned catalog publication fault"
                }),
                "{}",
                completed.message
            );
            assert!(
                crate::mounts::catalog_mount(&view, catalog)
                    .unwrap()
                    .is_some(),
                "Ready server was lost after publication failure"
            );
            if case == "mount-publication" {
                db.0.execute_batch("DROP TRIGGER mount_publication_fault")
                    .unwrap();
            }
        } else {
            assert_eq!(completed.state, "complete", "{}", completed.message);
        }
    }
    if case.starts_with("mount") && !case.contains("shutdown") {
        finish(
            service,
            DriveRequest::UnmountCatalog {
                catalog: catalog.to_owned(),
            },
        );
        // Explicit cleanup must retire transferred ownership even when the
        // failed mount transaction left no saved mount path.
        finish(
            service,
            DriveRequest::MountCatalog {
                catalog: catalog.to_owned(),
            },
        );
    }
}
