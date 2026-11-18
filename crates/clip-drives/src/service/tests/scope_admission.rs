//! Scheduling admission retains blocked reads without freezing independent work.
use super::*;
use std::collections::HashSet;

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
    // Observe the persistent policy/refresh peers before gating previously unseen
    // inventory callers. Stable-ID pairing does not invoke inventory on probes.
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
    let service = fixture.start_observed_reads(Arc::new(move || {
        let thread = std::thread::current();
        if thread
            .name()
            .is_some_and(|name| name.starts_with("drive-sync-"))
        {
            return;
        }
        let first = known.lock().unwrap().insert(thread.id());
        if first
            && thread
                .name()
                .is_some_and(|name| name.starts_with("drive-scope-"))
            && active.load(Ordering::Acquire)
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
            DriveRequest::Catalog {
                drive: selector.into(),
            },
        );
        let (tid, wait) = wait_fuse_request(|| threads.lock().unwrap().keys().copied().collect());
        eprintln!("SCOPE_FUSE_METADATA_BLOCKED tid={tid} wchan={wait}");
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
    assert_eq!(
        fs::read(source.join("edit")).unwrap(),
        b"original portable content"
    );
    drop(native);
    crate::ram::discard(&backing, &source_id).unwrap();
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
    let service = fixture.start_configured(
        DriveConfig {
            ram_min_available: 0,
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        Some(Arc::new(move || {
            let thread = std::thread::current();
            if thread
                .name()
                .is_some_and(|name| name.starts_with("drive-sync-"))
            {
                return;
            }
            if known.lock().unwrap().insert(thread.id()) {
                match current.load(Ordering::Acquire) {
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
