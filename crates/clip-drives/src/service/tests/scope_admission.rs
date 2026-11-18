//! Scheduling admission retains blocked reads without freezing independent work.
use super::*;
use std::collections::HashSet;

#[test]
fn ram_source_checks_do_not_reenter_the_coordinator() {
    use std::sync::atomic::AtomicUsize;
    let fixture = TestDrives::new(0);
    let armed = Arc::new(AtomicBool::new(false));
    let active = armed.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let service = fixture.start_observed_reads(Arc::new(move || {
        if active.load(Ordering::Acquire)
            && std::thread::current().name() == Some("portable-drive-owner")
        {
            observed.fetch_add(1, Ordering::AcqRel);
        }
    }));
    armed.store(true, Ordering::Release);
    let result = completed(
        &service,
        DriveRequest::CopyToRam {
            drive: "part:large".into(),
        },
    );
    armed.store(false, Ordering::Release);
    service.shutdown().unwrap();
    assert_eq!(result.state, "failed", "{}", result.message);
    assert!(status(&service).ram.is_empty());
    assert_eq!(
        calls.load(Ordering::Acquire),
        0,
        "RAM source discovery ran on the coordinator"
    );
}

#[test]
fn held_ram_source_revalidation_keeps_independent_work_live_and_retires_before_allocation() {
    for fault in ["drive", "root", "shutdown"] {
        let shutdown = fault == "shutdown";
        let fixture = TestDrives::new(0);
        let (entered, release, observe) =
            worker_observer("drive-ram-setup-", Arc::new(AtomicBool::new(true)));
        let service = fixture.start_observed_reads(observe);
        let local = fixture.pair(&service, "small", "");
        let operation = accept(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:large".into(),
            },
        );
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_unfinished(&service, &operation);
        finish(
            &service,
            DriveRequest::Rule {
                drive: "part:small".into(),
                relative: "private".into(),
                catalog: true,
                index: false,
            },
        );
        fs::write(
            local.join("independent"),
            b"sync during RAM source validation",
        )
        .unwrap();
        wait_contents(
            &fixture.temp.path().join("small/independent"),
            b"sync during RAM source validation",
        );
        let closing = if shutdown {
            service
                .request(DriveRequest::Cancel {
                    operation: operation.clone(),
                })
                .unwrap();
            assert_unfinished(&service, &operation);
            Some(shutdown_while_held(&service))
        } else {
            if fault == "drive" {
                fixture.drives.lock().unwrap()[1].evidence =
                    "replaced during source validation".into();
            } else {
                let source = fixture.temp.path().join("large");
                fs::rename(&source, fixture.temp.path().join("removed-large")).unwrap();
                fs::create_dir(source).unwrap();
            }
            None
        };
        release.send(()).unwrap();
        if let Some(closing) = closing {
            finish_held_shutdown(closing);
        }
        let result = wait_operation(&service, &operation, |op| op.finished.is_some());
        if shutdown {
            assert_eq!(result.state, "cancelled", "{}", result.message);
        } else {
            assert_eq!(result.state, "failed", "{}", result.message);
            assert!(
                result.message.contains(if fault == "drive" {
                    "Drive changed during RAM preparation"
                } else {
                    "Directory or mount was replaced or removed"
                }),
                "{}",
                result.message
            );
            service.shutdown().unwrap();
        }
        assert!(status(&service).ram.is_empty());
        let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
        assert!(db.ids("ram-allocation", "", 128).unwrap().is_empty());
        let entries: u64 =
            db.0.query_row(
                "SELECT count(*) FROM entries WHERE scan LIKE 'ram-preflight:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(entries, 0);
    }
}

fn warm_observers(
    fixture: &TestDrives,
    service: &Service,
    threads: &Mutex<HashSet<std::thread::ThreadId>>,
) -> PathBuf {
    let local = fixture.pair(service, "small", "");
    finish(
        service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "seed".into(),
            catalog: true,
            index: true,
        },
    );
    // Observe policy/refresh and scope peers before arming the held-work phase.
    let started = Instant::now();
    while threads.lock().unwrap().len() < 4 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Refresh/policy peers did not enter inventory"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    local
}

#[test]
fn four_held_scope_probes_keep_controls_sync_cancellation_and_shutdown_owned() {
    let fixture = TestDrives::new(0);
    // Inventory-only selector: this test never opens or creates a device node.
    fixture.drives.lock().unwrap()[1].device = "/dev/little-clip-owned-scope-fixture".into();
    let threads = Arc::new(Mutex::new(HashSet::new()));
    let known = threads.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let active = armed.clone();
    let (entered, release, gate) = worker_gate();
    let blocked = Mutex::new(HashSet::new());
    let service = fixture.start_observed_reads(Arc::new(move || {
        let thread = std::thread::current();
        known.lock().unwrap().insert(thread.id());
        if thread
            .name()
            .is_some_and(|name| name.starts_with("drive-scope-"))
            && active.load(Ordering::Acquire)
            && blocked.lock().unwrap().insert(thread.id())
        {
            gate();
        }
    }));
    let local = warm_observers(&fixture, &service, &threads);
    armed.store(true, Ordering::Release);
    let selector = fixture.drives.lock().unwrap()[1]
        .device
        .to_str()
        .unwrap()
        .to_owned();
    let operations = (0..4)
        .map(|_| {
            accept(
                &service,
                DriveRequest::Catalog {
                    drive: selector.clone(),
                },
            )
        })
        .collect::<Vec<_>>();
    for _ in 0..4 {
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    let destination = fixture.temp.path().join("uncreated/output");
    let waiting = accept(
        &service,
        DriveRequest::SaveRam {
            session: "missing".into(),
            destination: destination.clone(),
        },
    );
    assert_configuration_responsive(&service);
    assert_unfinished(&service, &waiting);
    service
        .request(DriveRequest::Cancel {
            operation: waiting.clone(),
        })
        .unwrap();
    assert_eq!(
        wait_operation(&service, &waiting, |op| op.finished.is_some()).state,
        "cancelled"
    );
    assert!(!destination.parent().unwrap().exists());
    // Cancellation of a queued save neither creates its directory nor releases
    // any of the four independently held probe owners.
    for operation in &operations {
        assert_unfinished(&service, operation);
    }
    let waiting = accept(
        &service,
        DriveRequest::SaveRam {
            session: "missing".into(),
            destination: destination.clone(),
        },
    );
    fs::write(
        local.join("independent"),
        b"sync during four blocked probes",
    )
    .unwrap();
    wait_contents(
        &fixture.temp.path().join("small/independent"),
        b"sync during four blocked probes",
    );
    for operation in &operations {
        service
            .request(DriveRequest::Cancel {
                operation: operation.clone(),
            })
            .unwrap();
        assert_unfinished(&service, operation);
    }
    // One actual exit, rather than a cancellation request, releases one slot.
    release.send(()).unwrap();
    let result = wait_operation(&service, &waiting, |op| op.finished.is_some());
    assert_eq!(result.state, "failed", "{}", result.message);
    assert!(result.message.contains("RAM enrollment is missing"));
    assert!(!destination.parent().unwrap().exists());
    let terminal = status(&service)
        .operations
        .into_iter()
        .filter(|op| operations.contains(&op.id) && op.finished.is_some())
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].state, "cancelled");
    let (shutdown, completed) = shutdown_while_held(&service);
    for _ in 0..3 {
        release.send(()).unwrap();
    }
    finish_held_shutdown((shutdown, completed));
    for id in operations {
        assert_eq!(
            wait_operation(&service, &id, |op| op.finished.is_some()).state,
            "cancelled"
        );
    }
    assert!(status(&service).catalogs.is_empty());
    let restarted =
        fixture.start_configured(status(&service).config, None, fixture.image_authority());
    assert!(status(&restarted).catalogs.is_empty());
    restarted.shutdown().unwrap();
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned child"]
fn stalled_fuse_scope_admission_keeps_controls_sync_and_shutdown_owned() {
    for operation in ["catalog", "ram", "enrollment"] {
        let fixture = TestDrives::new(0);
        let (backing, source_id, mut native, source) = fixture.native_source("partition");
        let selector = "/dev/little-clip-owned-scope-fixture";
        fixture.drives.lock().unwrap()[1].device = selector.into();
        let threads = Arc::new(Mutex::new(HashMap::new()));
        let observed = threads.clone();
        let service = fixture.start_configured(
            DriveConfig {
                ram_min_available: u64::MAX,
                sync_interval_seconds: 3600,
                ..Default::default()
            },
            Some(Arc::new(move || {
                // SAFETY: gettid has no arguments or memory effects.
                let tid = unsafe { libc::syscall(libc::SYS_gettid) };
                observed.lock().unwrap().insert(
                    tid,
                    std::thread::current().name() == Some("portable-drive-owner"),
                );
            })),
            fixture.image_authority(),
        );
        let local = fixture.pair(&service, "small", "");
        let mut shutdown = None;
        let mut accepted = None;
        native.with_suspended(|| {
            let id = accept(
                &service,
                match operation {
                    "ram" => DriveRequest::CopyToRam {
                        drive: "part:large".into(),
                    },
                    "enrollment" => DriveRequest::Pair {
                        drive: "part:large".into(),
                        relative: PathBuf::new(),
                        local: fixture.temp.path().join("uncreated-enrollment"),
                    },
                    _ => DriveRequest::Catalog {
                        drive: selector.into(),
                    },
                },
            );
            let (tid, wait) =
                wait_fuse_request(|| threads.lock().unwrap().keys().copied().collect());
            eprintln!("SCOPE_FUSE_METADATA_BLOCKED operation={operation} tid={tid} wchan={wait}");
            assert!(
                !threads.lock().unwrap()[&tid],
                "Coordinator entered the FUSE wait"
            );
            assert_configuration_responsive(&service);
            fs::write(
                local.join("independent"),
                b"sync during a real source-scope stall",
            )
            .unwrap();
            wait_contents(
                &fixture.temp.path().join("small/independent"),
                b"sync during a real source-scope stall",
            );
            service
                .request(DriveRequest::Cancel {
                    operation: id.clone(),
                })
                .unwrap();
            assert_unfinished(&service, &id);
            let (joining, completed) = shutdown_while_held(&service);
            accepted = Some(id);
            shutdown = Some((joining, completed));
        });
        finish_held_shutdown(shutdown.unwrap());
        assert_eq!(
            wait_operation(&service, &accepted.unwrap(), |op| op.finished.is_some()).state,
            "cancelled"
        );
        assert!(status(&service).catalogs.is_empty());
        assert!(!fixture.temp.path().join("uncreated-enrollment").exists());
        assert!(
            status(&service)
                .pairs
                .iter()
                .all(|pair| pair.drive != "part:large")
        );
        assert_eq!(
            fs::read(source.join("edit")).unwrap(),
            b"original portable content"
        );
        drop(native);
        crate::ram::discard(&backing, &source_id).unwrap();
    }
}

#[test]
#[ignore = "requires RAM capability preflight; cancels before allocating any RAM image"]
fn simultaneous_scope_deliveries_cannot_replace_the_retained_ram_setup_owner() {
    use std::sync::atomic::AtomicUsize;
    let fixture = TestDrives::new(0);
    for (index, drive) in fixture.drives.lock().unwrap().iter_mut().enumerate() {
        drive.device = format!("/dev/little-clip-owned-scope-{index}").into();
    }
    let threads = Arc::new(Mutex::new(HashSet::new()));
    let known = threads.clone();
    let phase = Arc::new(AtomicUsize::new(0));
    let current = phase.clone();
    let (entered, release, gate) = worker_gate();
    let (preparing, resume, prepare_gate) = worker_gate();
    let phases = Mutex::new(HashSet::new());
    let service = fixture.start_configured(
        DriveConfig {
            ram_min_available: 0,
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        Some(Arc::new(move || {
            let thread = std::thread::current();
            known.lock().unwrap().insert(thread.id());
            let phase = current.load(Ordering::Acquire);
            if phases.lock().unwrap().insert((phase, thread.id())) {
                match phase {
                    1 if thread
                        .name()
                        .is_some_and(|name| name.starts_with("drive-scope-")) =>
                    {
                        gate()
                    }
                    2 if thread
                        .name()
                        .is_some_and(|name| name.starts_with("drive-ram-setup-")) =>
                    {
                        prepare_gate()
                    }
                    _ => (),
                }
            }
        })),
        fixture.image_authority(),
    );
    warm_observers(&fixture, &service, &threads);
    phase.store(1, Ordering::Release);
    let operations = (0..2)
        .map(|index| {
            accept(
                &service,
                DriveRequest::CopyToRam {
                    drive: format!("/dev/little-clip-owned-scope-{index}"),
                },
            )
        })
        .collect::<Vec<_>>();
    for _ in 0..2 {
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    phase.store(2, Ordering::Release);
    for _ in 0..2 {
        release.send(()).unwrap();
    }
    preparing.recv_timeout(Duration::from_secs(5)).unwrap();
    for operation in &operations {
        service
            .request(DriveRequest::Cancel {
                operation: operation.clone(),
            })
            .unwrap();
    }
    resume.send(()).unwrap();
    for operation in operations {
        let result = wait_operation(&service, &operation, |op| op.finished.is_some());
        assert_eq!(result.state, "cancelled", "{}", result.message);
    }
    assert!(status(&service).ram.is_empty());
    let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
    assert!(
        db.list::<ram_setup::Allocation>("ram-allocation", 0, 10)
            .unwrap()
            .is_empty()
    );
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires the disposable native-filesystem guest and packaged RAM helper"]
fn native_ram_source_handoff_populates_and_flushes_the_selected_partition() {
    crate::require_disposable_guest();
    let drive = crate::sync_edge_tests::native_fs::guest_drive();
    let selected = drive.id.clone();
    let source = drive.mount.clone().unwrap();
    let folder = PathBuf::from(format!("ram-handoff-{}", uuid::Uuid::new_v4()));
    fs::create_dir(source.join(&folder)).unwrap();
    let relative = folder.join("edit");
    fs::write(source.join(&relative), b"original portable bytes").unwrap();
    let fixture = TestDrives::new(0);
    *fixture.drives.lock().unwrap() = vec![drive];
    let service = fixture.start_configured(
        DriveConfig {
            ram_min_available: 0,
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        None,
        fixture.image_authority(),
    );
    finish(
        &service,
        DriveRequest::CopyToRam {
            drive: selected.clone(),
        },
    );
    let session = status(&service).ram.pop().unwrap();
    assert!(session.ready && session.connected);
    assert_eq!(session.drive, selected);
    assert_eq!(
        fs::read(session.path.join(&relative)).unwrap(),
        b"original portable bytes"
    );
    finish(
        &service,
        DriveRequest::Pause {
            pair: session.id.clone(),
            paused: true,
        },
    );
    fs::write(session.path.join(&relative), b"RAM first edit").unwrap();
    assert_eq!(
        fs::read(source.join(&relative)).unwrap(),
        b"original portable bytes"
    );
    finish(
        &service,
        DriveRequest::FlushRam {
            session: session.id.clone(),
        },
    );
    assert_eq!(fs::read(source.join(&relative)).unwrap(), b"RAM first edit");
    discard_ram(&service, &session);
    service.shutdown().unwrap();
    fs::remove_dir_all(source.join(folder)).unwrap();
}
