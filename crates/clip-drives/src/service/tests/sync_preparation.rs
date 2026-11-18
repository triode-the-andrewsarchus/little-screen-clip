//! Prepared roots remain read-only until current ownership and policy are checked.
use super::*;

fn paused_pair(fixture: &TestDrives, service: &Service, drive: &str) -> (String, PathBuf) {
    let local = fixture.pair(service, drive, "");
    let pair = status(service)
        .pairs
        .into_iter()
        .find(|pair| pair.drive == format!("part:{drive}"))
        .unwrap()
        .id;
    finish(
        service,
        DriveRequest::Pause {
            pair: pair.clone(),
            paused: true,
        },
    );
    (pair, local)
}

struct Held {
    service: Service,
    armed: Arc<AtomicBool>,
    entered: mpsc::Receiver<()>,
    release: Option<mpsc::Sender<()>>,
}
impl Held {
    fn new(fixture: &TestDrives) -> Self {
        let armed = Arc::new(AtomicBool::new(false));
        let observe = armed.clone();
        let (entered, release, gate) = worker_gate();
        let service = fixture.start_observed_reads(Arc::new(move || {
            if std::thread::current()
                .name()
                .is_some_and(|name| name.starts_with("drive-sync-"))
                && observe.swap(false, Ordering::AcqRel)
            {
                gate();
            }
        }));
        Self {
            service,
            armed,
            entered,
            release: Some(release),
        }
    }
    fn sync(&self, pair: &str) -> String {
        self.armed.store(true, Ordering::Release);
        let id = accept(&self.service, DriveRequest::Sync { pair: pair.into() });
        self.entered.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_unfinished(&self.service, &id);
        id
    }
    fn release(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }
}
impl Drop for Held {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
        let _ = self.service.shutdown();
    }
}

#[test]
fn held_sync_preparation_keeps_controls_and_another_pair_live_and_shutdown_owned() {
    let fixture = TestDrives::new(0);
    fs::write(fixture.temp.path().join("large/edit"), b"original").unwrap();
    let mut held = Held::new(&fixture);
    let (small, local) = paused_pair(&fixture, &held.service, "small");
    let (large, large_local) = paused_pair(&fixture, &held.service, "large");
    let confirmed = status(&held.service)
        .pairs
        .into_iter()
        .find(|p| p.id == large)
        .unwrap()
        .last_success;
    fs::write(large_local.join("edit"), b"edit waiting for preparation").unwrap();
    let operation = held.sync(&large);
    assert_configuration_responsive(&held.service);
    finish(
        &held.service,
        DriveRequest::Pause {
            pair: small,
            paused: false,
        },
    );
    fs::write(
        local.join("independent"),
        b"sync during held root preparation",
    )
    .unwrap();
    wait_contents(
        &fixture.temp.path().join("small/independent"),
        b"sync during held root preparation",
    );
    held.service
        .request(DriveRequest::Cancel {
            operation: operation.clone(),
        })
        .unwrap();
    assert_unfinished(&held.service, &operation);
    let (shutdown, completed) = shutdown_while_held(&held.service);
    held.release();
    finish_held_shutdown((shutdown, completed));
    assert_eq!(
        wait_operation(&held.service, &operation, |op| op.finished.is_some()).state,
        "cancelled"
    );
    assert_eq!(
        fs::read(fixture.temp.path().join("large/edit")).unwrap(),
        b"original"
    );
    assert_eq!(
        fs::read(large_local.join("edit")).unwrap(),
        b"edit waiting for preparation"
    );
    assert_eq!(
        status(&held.service)
            .pairs
            .into_iter()
            .find(|p| p.id == large)
            .unwrap()
            .last_success,
        confirmed
    );
    let restarted = fixture.start_configured(
        status(&held.service).config,
        None,
        fixture.image_authority(),
    );
    assert_eq!(
        wait_operation(&restarted, &operation, |op| op.finished.is_some()).state,
        "cancelled"
    );
    restarted.shutdown().unwrap();
}

#[test]
fn changed_managed_storage_rejects_a_prepared_sync_plan_before_installation() {
    let fixture = TestDrives::new(0);
    let portable = fixture.temp.path().join("large");
    fs::write(portable.join("edit"), b"original").unwrap();
    let mut held = Held::new(&fixture);
    let (pair, local) = paused_pair(&fixture, &held.service, "large");
    fs::write(local.join("edit"), b"local edit").unwrap();
    let operation = held.sync(&pair);
    let mut config = status(&held.service).config;
    config.backup_directory = Some(portable.clone());
    finish(&held.service, DriveRequest::Configure { config });
    held.release();
    let result = wait_operation(&held.service, &operation, |op| op.finished.is_some());
    assert_eq!(result.state, "failed", "{}", result.message);
    assert!(
        result.message.contains("managed backups"),
        "{}",
        result.message
    );
    assert_eq!(fs::read(portable.join("edit")).unwrap(), b"original");
    assert_eq!(fs::read(local.join("edit")).unwrap(), b"local edit");
    let mut config = status(&held.service).config;
    config.backup_directory = None;
    finish(&held.service, DriveRequest::Configure { config });
    finish(&held.service, DriveRequest::Sync { pair });
    assert_eq!(fs::read(portable.join("edit")).unwrap(), b"local edit");
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; stops only its owned child"]
fn stalled_fuse_sync_preparation_keeps_controls_another_pair_and_shutdown_live() {
    let fixture = TestDrives::new(0);
    let (backing, source_id, mut native, source) = fixture.native_source("partition");
    let threads = Arc::new(Mutex::new(std::collections::HashSet::new()));
    let observed = threads.clone();
    let service = fixture.start_observed_reads(Arc::new(move || {
        if std::thread::current()
            .name()
            .is_some_and(|name| name.starts_with("drive-sync-"))
        {
            // SAFETY: gettid has no arguments or memory effects.
            observed
                .lock()
                .unwrap()
                .insert(unsafe { libc::syscall(libc::SYS_gettid) });
        }
    }));
    let (small, small_local) = paused_pair(&fixture, &service, "small");
    let (large, large_local) = paused_pair(&fixture, &service, "large");
    let confirmed = status(&service)
        .pairs
        .into_iter()
        .find(|p| p.id == large)
        .unwrap()
        .last_success;
    finish(
        &service,
        DriveRequest::Pause {
            pair: large.clone(),
            paused: false,
        },
    );
    let mut shutdown = None;
    native.with_suspended(|| {
        fs::write(large_local.join("edit"), b"local edit during source stall").unwrap();
        let (tid, wait) = wait_fuse_request(|| threads.lock().unwrap().iter().copied().collect());
        eprintln!("SYNC_PREPARATION_FUSE_BLOCKED tid={tid} wchan={wait}");
        assert_configuration_responsive(&service);
        finish(
            &service,
            DriveRequest::Pause {
                pair: small,
                paused: false,
            },
        );
        fs::write(
            small_local.join("independent"),
            b"sync during a real preparation stall",
        )
        .unwrap();
        wait_contents(
            &fixture.temp.path().join("small/independent"),
            b"sync during a real preparation stall",
        );
        let (joining, completed) = shutdown_while_held(&service);
        shutdown = Some((joining, completed));
    });
    finish_held_shutdown(shutdown.unwrap());
    assert_eq!(
        fs::read(source.join("edit")).unwrap(),
        b"original portable content"
    );
    assert_eq!(
        fs::read(large_local.join("edit")).unwrap(),
        b"local edit during source stall"
    );
    assert_eq!(
        status(&service)
            .pairs
            .into_iter()
            .find(|p| p.id == large)
            .unwrap()
            .last_success,
        confirmed
    );
    drop(native);
    crate::ram::discard(&backing, &source_id).unwrap();
}
