//! Prepared roots remain read-only until current ownership and policy are checked.
use super::*;

#[test]
fn enrollment_source_checks_do_not_reenter_the_coordinator() {
    use std::sync::atomic::AtomicUsize;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(move || {
        if std::thread::current().name() == Some("portable-drive-owner") {
            observed.fetch_add(1, Ordering::AcqRel);
        }
    }));
    fixture.pair(&service, "large", "");
    let count = calls.load(Ordering::Acquire);
    service.shutdown().unwrap();
    assert_eq!(count, 0, "Enrollment discovery ran on the coordinator");
}

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
        let (entered, release, observe) = worker_observer("drive-sync-", armed.clone());
        let service = fixture.start_observed_reads(observe);
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
    fn enroll(&self, drive: &str, local: &Path) -> String {
        self.armed.store(true, Ordering::Release);
        let id = accept(
            &self.service,
            DriveRequest::Pair {
                drive: drive.into(),
                relative: PathBuf::new(),
                local: local.into(),
            },
        );
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
fn enrollment_preparation_keeps_controls_live_and_publishes_only_current_uncancelled_inputs() {
    for outcome in [
        "success",
        "cancel",
        "config",
        "destination",
        "drive",
        "read-only",
        "registry",
    ] {
        let fixture = TestDrives::new(0);
        fs::write(
            fixture.temp.path().join("large/edit"),
            b"portable enrollment content",
        )
        .unwrap();
        let mut held = Held::new(&fixture);
        let (small_pair, small) = paused_pair(&fixture, &held.service, "small");
        let local = fixture.temp.path().join("uncreated/local");
        if outcome == "read-only" {
            fixture.drives.lock().unwrap()[1].read_only = true;
        }
        let operation = held.enroll("part:large", &local);
        assert!(
            !local.exists(),
            "Source preparation created the destination before validation"
        );
        assert!(
            status(&held.service)
                .pairs
                .iter()
                .all(|p| p.drive != "part:large")
        );
        assert_configuration_responsive(&held.service);
        finish(
            &held.service,
            DriveRequest::Pause {
                pair: small_pair,
                paused: false,
            },
        );
        fs::write(
            small.join("independent"),
            b"sync during enrollment preparation",
        )
        .unwrap();
        wait_contents(
            &fixture.temp.path().join("small/independent"),
            b"sync during enrollment preparation",
        );
        let mut shutdown = None;
        match outcome {
            "cancel" => {
                held.service
                    .request(DriveRequest::Cancel {
                        operation: operation.clone(),
                    })
                    .unwrap();
                assert_unfinished(&held.service, &operation);
                shutdown = Some(shutdown_while_held(&held.service));
            }
            "config" => {
                let mut config = status(&held.service).config;
                config.backup_directory = Some(fixture.temp.path().join("new-backups"));
                finish(&held.service, DriveRequest::Configure { config });
            }
            "destination" => fs::create_dir(local.parent().unwrap()).unwrap(),
            "drive" => fixture.drives.lock().unwrap()[1].evidence = "replacement evidence".into(),
            "registry" => {
                let db = Store::existing(&held.service.state.join("drives.sqlite"), false).unwrap();
                db.0.execute_batch("CREATE TRIGGER pair_publication_fault BEFORE INSERT ON entities
                    WHEN NEW.kind='pair' BEGIN SELECT RAISE(FAIL,'pair publication fixture fault'); END;").unwrap();
            }
            _ => (),
        }
        held.release();
        if let Some(shutdown) = shutdown {
            finish_held_shutdown(shutdown);
        }
        let result = wait_operation(&held.service, &operation, |op| op.finished.is_some());
        match outcome {
            "success" => {
                assert_eq!(result.state, "complete", "{}", result.message);
                assert_eq!(
                    fs::read(local.join("edit")).unwrap(),
                    b"portable enrollment content"
                );
            }
            "cancel" => assert_eq!(result.state, "cancelled", "{}", result.message),
            _ => {
                assert_eq!(result.state, "failed", "{}", result.message);
                let reason = match outcome {
                    "config" => "Enrollment inputs changed",
                    "destination" => "destination changed since scheduling admission",
                    "drive" => "Drive changed while preparing folder enrollment",
                    "read-only" => "Drive is read-only",
                    "registry" => "pair publication fixture fault",
                    _ => unreachable!(),
                };
                assert!(result.message.contains(reason), "{}", result.message);
            }
        }
        if outcome != "success" {
            assert!(!local.join("edit").exists());
            assert!(
                status(&held.service)
                    .pairs
                    .iter()
                    .all(|p| p.drive != "part:large")
            );
        }
    }
}

#[test]
fn pending_enrollment_reserves_its_destination_and_rejects_an_overlapping_second_pair() {
    let fixture = TestDrives::new(0);
    fs::write(
        fixture.temp.path().join("large/from-large"),
        b"belongs to large",
    )
    .unwrap();
    let mut held = Held::new(&fixture);
    let local = fixture.temp.path().join("shared-target");
    let first = held.enroll("part:large", &local);
    let second = accept(
        &held.service,
        DriveRequest::Pair {
            drive: "part:small".into(),
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    assert_configuration_responsive(&held.service);
    assert_unfinished(&held.service, &first);
    assert_unfinished(&held.service, &second);
    assert!(!local.exists());
    held.release();
    assert_eq!(
        wait_operation(&held.service, &first, |op| op.finished.is_some()).state,
        "complete"
    );
    let rejected = wait_operation(&held.service, &second, |op| op.finished.is_some());
    assert_eq!(rejected.state, "failed", "{}", rejected.message);
    assert!(
        rejected
            .message
            .contains("overlaps an existing sync enrollment"),
        "{}",
        rejected.message
    );
    assert_eq!(status(&held.service).pairs.len(), 1);
    assert_eq!(
        fs::read(local.join("from-large")).unwrap(),
        b"belongs to large"
    );
    assert!(!local.join("edit").exists());
    assert_eq!(
        fs::read(fixture.temp.path().join("small/edit")).unwrap(),
        b"initial"
    );
}

#[test]
fn unavailable_enrollment_targets_keep_their_lexical_boundaries() {
    let fixture = TestDrives::new(0);
    let service = fixture.start_observed_reads(Arc::new(|| {}));
    let parent = fixture.temp.path().join("ordinary");
    let local = parent.join("paired-small");
    finish(
        &service,
        DriveRequest::Pair {
            drive: "part:small".into(),
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    let pair = status(&service).pairs.pop().unwrap();
    finish(
        &service,
        DriveRequest::Pause {
            pair: pair.id,
            paused: true,
        },
    );
    fs::rename(&local, fixture.temp.path().join("removed-local")).unwrap();
    failed(
        &service,
        DriveRequest::Pair {
            drive: "part:large".into(),
            relative: PathBuf::new(),
            local: parent.clone(),
        },
        "overlaps an existing sync enrollment",
    );
    assert_eq!(status(&service).pairs.len(), 1);
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
    assert_eq!(
        fs::read(fixture.temp.path().join("removed-local/edit")).unwrap(),
        b"initial"
    );
    service.shutdown().unwrap();
}

#[test]
fn replaced_enrollment_root_during_final_inventory_cannot_be_registered() {
    use std::sync::atomic::AtomicUsize;
    let fixture = TestDrives::new(0);
    let calls = AtomicUsize::new(0);
    let (entered, release, gate) = worker_gate();
    let service = fixture.start_observed_reads(Arc::new(move || {
        if std::thread::current()
            .name()
            .is_some_and(|name| name.starts_with("drive-sync-"))
            && calls.fetch_add(1, Ordering::AcqRel) == 1
        {
            gate();
        }
    }));
    let local = fixture.temp.path().join("local");
    let operation = accept(
        &service,
        DriveRequest::Pair {
            drive: "part:large".into(),
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(local.is_dir());
    assert!(status(&service).pairs.is_empty());
    fs::rename(&local, fixture.temp.path().join("retained-local")).unwrap();
    fs::create_dir(&local).unwrap();
    fs::write(local.join("foreign"), b"preserve replacement").unwrap();
    release.send(()).unwrap();
    let failed = wait_operation(&service, &operation, |op| op.finished.is_some());
    service.shutdown().unwrap();
    assert_eq!(failed.state, "failed", "{}", failed.message);
    assert!(
        status(&service).pairs.is_empty(),
        "A replaced root was registered before the write worker refused it"
    );
    assert_eq!(
        fs::read(local.join("foreign")).unwrap(),
        b"preserve replacement"
    );
    assert!(!local.join("large-sparse").exists());
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
    for enrollment in [false, true] {
        let fixture = TestDrives::new(0);
        let (backing, source_id, mut native, source) = fixture.native_source("partition");
        if enrollment {
            fixture.drives.lock().unwrap()[1].mount = Some(fixture.temp.path().join("large"));
            fs::write(
                fixture.temp.path().join("large/new-from-large"),
                b"retain portable source",
            )
            .unwrap();
        }
        let threads = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let observed = threads.clone();
        let armed = Arc::new(AtomicBool::new(false));
        let (entered, resume, observe) = worker_observer("drive-sync-", armed.clone());
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
            observe();
        }));
        let (small, small_local) = paused_pair(&fixture, &service, "small");
        let large = (!enrollment).then(|| paused_pair(&fixture, &service, "large"));
        let large_local = large
            .as_ref()
            .map_or_else(|| source.join("new-enrollment"), |(_, path)| path.clone());
        let confirmed = if let Some((large, _)) = &large {
            status(&service)
                .pairs
                .into_iter()
                .find(|p| &p.id == large)
                .unwrap()
                .last_success
        } else {
            None
        };
        let accepted = if enrollment {
            armed.store(true, Ordering::Release);
            let operation = accept(
                &service,
                DriveRequest::Pair {
                    drive: "part:large".into(),
                    relative: PathBuf::new(),
                    local: large_local.clone(),
                },
            );
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            Some(operation)
        } else {
            finish(
                &service,
                DriveRequest::Pause {
                    pair: large.as_ref().unwrap().0.clone(),
                    paused: false,
                },
            );
            None
        };
        let mut shutdown = None;
        native.with_suspended(|| {
            if enrollment {
                resume.send(()).unwrap();
            } else {
                fs::write(large_local.join("edit"), b"local edit during source stall").unwrap();
            }
            let (tid, wait) =
                wait_fuse_request(|| threads.lock().unwrap().iter().copied().collect());
            eprintln!(
                "SYNC_PREPARATION_FUSE_BLOCKED enrollment={enrollment} tid={tid} wchan={wait}"
            );
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
        if let Some(operation) = accepted {
            assert_eq!(
                wait_operation(&service, &operation, |op| op.finished.is_some()).state,
                "cancelled"
            );
            assert!(
                status(&service)
                    .pairs
                    .iter()
                    .all(|pair| pair.drive != "part:large")
            );
            assert!(!large_local.join("new-from-large").exists());
            assert_eq!(
                fs::read(fixture.temp.path().join("large/new-from-large")).unwrap(),
                b"retain portable source"
            );
        } else {
            assert_eq!(
                fs::read(large_local.join("edit")).unwrap(),
                b"local edit during source stall"
            );
            assert_eq!(
                status(&service)
                    .pairs
                    .into_iter()
                    .find(|p| p.id == large.as_ref().unwrap().0)
                    .unwrap()
                    .last_success,
                confirmed
            );
        }
        drop(native);
        crate::ram::discard(&backing, &source_id).unwrap();
    }
}
