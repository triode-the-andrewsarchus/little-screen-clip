//! Deliberate refresh stalls never stand in for a stalled coordinator.
use super::*;
use std::sync::atomic::AtomicUsize;

fn traced_publication() -> (
    publication::Publication,
    Arc<Mutex<Option<std::thread::ThreadId>>>,
) {
    let thread = Arc::new(Mutex::new(None));
    let published = thread.clone();
    let publisher = publication::Publication::new(Arc::new(move |input, previous| {
        *published.lock().unwrap() = Some(std::thread::current().id());
        publication::observe(input, previous)
    }));
    (publisher, thread)
}

pub(super) struct Held {
    pub service: Service,
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
    armed: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}
impl Held {
    pub(super) fn new(fixture: &TestDrives, config: DriveConfig) -> Self {
        let armed = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = armed.clone();
        let seen = calls.clone();
        let once = AtomicBool::new(false);
        let (publisher, _) = traced_publication();
        let (entered, release, gate) = worker_gate();
        let service = fixture.start_with_publication(
            config,
            Some(Arc::new(move || {
                let thread = std::thread::current();
                let refresh = thread
                    .name()
                    .is_some_and(|name| name.starts_with("drive-status-"));
                if refresh && observed.load(Ordering::Acquire) {
                    seen.fetch_add(1, Ordering::AcqRel);
                    if !once.swap(true, Ordering::AcqRel) {
                        gate();
                    }
                }
            })),
            fixture.image_authority(),
            publisher,
        );
        // Identify the persistent publication worker before arming its separate
        // inventory peer. Fresh policy and execution admission remain ungated.
        finish(
            &service,
            DriveRequest::Rule {
                drive: "part:small".into(),
                relative: "seed".into(),
                catalog: true,
                index: true,
            },
        );
        Self {
            service,
            entered,
            release: Some(release),
            armed,
            calls,
        }
    }
    pub(super) fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }
    pub(super) fn wait(&self) {
        self.entered.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    pub(super) fn release(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        // Only owned temporary source directories are used by this fixture.
        let _ = self.service.shutdown();
    }
}

fn config() -> DriveConfig {
    DriveConfig {
        ram_min_available: 0,
        sync_interval_seconds: 3600,
        ..Default::default()
    }
}

#[test]
fn an_unknown_ram_save_never_creates_its_missing_output_directory() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_authority(None, fixture.image_authority());
    let destination = fixture.temp.path().join("missing-parent/export");
    let result = completed(
        &service,
        DriveRequest::SaveRam {
            session: "not-enrolled".into(),
            destination: destination.clone(),
        },
    );
    assert_eq!(result.state, "failed");
    assert!(
        result.message.contains("RAM enrollment is missing"),
        "{}",
        result.message
    );
    assert!(!destination.parent().unwrap().exists());
    service.shutdown().unwrap();
}

#[test]
fn a_held_refresh_coalesces_configurations_and_cannot_supply_stale_policy_sources() {
    let fixture = TestDrives::new(0);
    let mut held = Held::new(&fixture, config());
    let local = fixture.pair(&held.service, "small", "");
    held.arm();
    let mut latest = status(&held.service).config;
    latest.sync_interval_seconds += 1;
    finish(
        &held.service,
        DriveRequest::Configure {
            config: latest.clone(),
        },
    );
    held.wait();
    for step in 0..12 {
        latest.sync_interval_seconds = 3700 + step;
        latest.ram_min_available = u64::MAX;
        let started = Instant::now();
        finish(
            &held.service,
            DriveRequest::Configure {
                config: latest.clone(),
            },
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    fs::write(local.join("independent"), b"sync during held refresh").unwrap();
    wait_contents(
        &fixture.temp.path().join("small/independent"),
        b"sync during held refresh",
    );
    let pair = status(&held.service).pairs[0].id.clone();
    finish(&held.service, DriveRequest::Pause { pair, paused: true });
    let original = fixture.temp.path().join("small");
    let moved = fixture.temp.path().join("moved-small");
    fs::rename(&original, &moved).unwrap();
    fs::create_dir(moved.join("private")).unwrap();
    fs::write(moved.join("private/secret"), b"must stay excluded").unwrap();
    fixture.drives.lock().unwrap()[0].mount = Some(moved.clone());
    finish(
        &held.service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    assert_eq!(
        status(&held.service).drives[0].mount.as_ref(),
        Some(&original)
    );
    let policy = published_policy(&held.service);
    assert!(policy.excluded.contains(&moved.join("private")));
    assert_eq!(
        held.calls(),
        1,
        "Refresh requests queued behind a held observation"
    );
    assert_eq!(status(&held.service).config, latest);
    held.release();
    let started = Instant::now();
    loop {
        let current = status(&held.service);
        if current.drives[0].mount.as_ref() == Some(&moved)
            && current
                .ram_unavailable
                .as_deref()
                .is_some_and(|reason| reason.contains("Insufficient available RAM"))
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "Latest refresh did not apply"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        held.calls(),
        2,
        "Intermediate configurations became a probe backlog"
    );
}

#[test]
fn uncertain_policy_inventory_and_changed_mount_context_keep_indexing_paused() {
    for mount_fault in [false, true] {
        let fixture = TestDrives::new(0);
        let fault = Arc::new(AtomicBool::new(false));
        let observed_fault = fault.clone();
        let publication_thread = Arc::new(Mutex::new(None));
        let published = publication_thread.clone();
        let publisher = publication::Publication::new(Arc::new(move |mut input, previous| {
            *published.lock().unwrap() = Some(std::thread::current().id());
            if mount_fault && observed_fault.load(Ordering::Acquire) {
                input.mount_signature = Some("superseded-mount-observation".into());
            }
            publication::observe(input, previous)
        }));
        let observed_fault = fault.clone();
        let drives = fixture.drives.clone();
        let inventory: Inventory = Arc::new(move || {
            if !mount_fault
                && observed_fault.load(Ordering::Acquire)
                && Some(std::thread::current().id()) == *publication_thread.lock().unwrap()
            {
                return Err("Fresh policy inventory unavailable".into());
            }
            Ok(drives.lock().unwrap().clone())
        });
        let service = Service::start_with_authority(
            config(),
            fixture.temp.path().join("state"),
            inventory,
            None,
            Arc::new(fixture.image_authority()),
            publisher,
        )
        .unwrap();
        finish(
            &service,
            DriveRequest::Rule {
                drive: "part:small".into(),
                relative: "seed".into(),
                catalog: true,
                index: false,
            },
        );
        let verified = published_policy(&service);
        fault.store(true, Ordering::Release);
        let request = DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "private".into(),
            catalog: true,
            index: false,
        };
        assert_policy_paused(
            &service,
            request.clone(),
            &verified,
            if mount_fault {
                "Mount inventory changed"
            } else {
                "Fresh policy inventory unavailable"
            },
        );
        fault.store(false, Ordering::Release);
        finish(&service, request);
        assert!(
            !published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
        service.shutdown().unwrap();
    }
}

#[test]
fn shutdown_retains_a_live_status_probe_and_the_singleton_until_actual_exit() {
    let fixture = TestDrives::new(0);
    let mut held = Held::new(&fixture, config());
    held.arm();
    finish(&held.service, DriveRequest::Configure { config: config() });
    held.wait();
    let service = held.service.clone();
    let (exited, finished) = mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        let result = service.shutdown();
        exited.send(()).unwrap();
        result
    });
    assert_shutdown_retained(&held.service, &finished);
    held.release();
    finished.recv_timeout(Duration::from_secs(5)).unwrap();
    shutdown.join().unwrap().unwrap();
    let restarted = fixture.start_authority(None, fixture.image_authority());
    restarted.shutdown().unwrap();
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned child"]
fn a_stalled_ram_availability_probe_keeps_controls_and_sync_live_and_rejects_stale_reasons() {
    let fixture = TestDrives::new(0);
    let (backing, source_id, mut native, source) = fixture.native_source("partition");
    let (publisher, publication_thread) = traced_publication();
    let threads = Arc::new(Mutex::new(HashMap::new()));
    let observed_threads = threads.clone();
    let gate_thread = Arc::new(Mutex::new(None));
    let selected_thread = gate_thread.clone();
    let arm_next = Arc::new(AtomicBool::new(false));
    let next = arm_next.clone();
    let (entered, release, gate) = worker_gate();
    let service = fixture.start_with_publication(
        config(),
        Some(Arc::new(move || {
            // SAFETY: gettid has no arguments or memory effects.
            let tid = unsafe { libc::syscall(libc::SYS_gettid) };
            observed_threads
                .lock()
                .unwrap()
                .insert(std::thread::current().id(), tid);
            if Some(tid) == *selected_thread.lock().unwrap() && next.swap(false, Ordering::AcqRel) {
                gate();
            }
        })),
        fixture.image_authority(),
        publisher,
    );
    let local = fixture.pair(&service, "small", "");
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "seed".into(),
            catalog: true,
            index: true,
        },
    );
    native.with_suspended(|| {
        let mut current = status(&service).config;
        current.ram_directory = source.clone();
        finish(&service, DriveRequest::Configure { config: current });
        let (tid, wait) = wait_fuse_request(|| {
            let published = *publication_thread.lock().unwrap();
            threads
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(thread, tid)| (Some(*thread) != published).then_some(*tid))
                .collect()
        });
        eprintln!("REFRESH_FUSE_METADATA_BLOCKED tid={tid} wchan={wait}");
        *gate_thread.lock().unwrap() = Some(tid);
        let started = Instant::now();
        finish(&service, DriveRequest::Configure { config: config() });
        assert!(started.elapsed() < Duration::from_secs(2));
        fs::write(local.join("independent"), b"sync while RAM probe waits").unwrap();
        wait_contents(
            &fixture.temp.path().join("small/independent"),
            b"sync while RAM probe waits",
        );
        assert_eq!(
            status(&service).ram_unavailable.as_deref(),
            Some("Checking RAM storage availability")
        );
        arm_next.store(true, Ordering::Release);
    });
    // Hold the next probe before its RAM check. A subsequent owner round trip
    // proves it has processed the old FUSE failure, which must stay discarded.
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    finish(&service, DriveRequest::Configure { config: config() });
    assert_eq!(
        status(&service).ram_unavailable.as_deref(),
        Some("Checking RAM storage availability")
    );
    release.send(()).unwrap();
    let started = Instant::now();
    while status(&service).ram_unavailable.is_some() {
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(10));
    }
    service.shutdown().unwrap();
    drop(native);
    crate::ram::discard(&backing, &source_id).unwrap();
}
