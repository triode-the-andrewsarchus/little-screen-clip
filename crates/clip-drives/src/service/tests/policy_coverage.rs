use super::*;
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

fn policy_fixture() -> (TestDrives, Drive, Store) {
    let fixture = TestDrives::new(0);
    let state = fixture.temp.path().join("state");
    fs::create_dir(&state).unwrap();
    let db = Store::open(&state.join("drives.sqlite")).unwrap();
    // Policy tests never open this device; use the same selector namespace as
    // production discovery rather than the ordinary-file imaging fixture.
    fixture.drives.lock().unwrap()[0].device = "/dev/portable-policy-fixture".into();
    let drive = fixture.drives.lock().unwrap()[0].clone();
    (fixture, drive, db)
}

fn policy_observer_once(
    relative: &'static str,
    before: impl Fn() + Send + Sync + 'static,
) -> publication::Publication {
    let notified = AtomicBool::new(false);
    publication::Publication::new(Arc::new(move |input, previous| {
        if input
            .rules
            .iter()
            .any(|rule| rule.relative == Path::new(relative))
            && !notified.swap(true, Ordering::AcqRel)
        {
            before();
        }
        publication::observe(input, previous)
    }))
}

#[test]
fn a_saved_rule_is_published_before_its_completion_is_reported() {
    let (fixture, drive, _) = policy_fixture();
    let source = drive.mount.clone().unwrap();
    let selected = drive.id.clone();
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Rule {
            drive: selected,
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    let policy = published_policy(&service);
    assert!(policy.excluded.contains(&source.join("private")));
    service.shutdown().unwrap();
}

#[test]
fn published_policy_includes_rules_beyond_one_database_page() {
    let (fixture, drive, db) = policy_fixture();
    let state = fixture.temp.path().join("state");
    let source = drive.mount.as_ref().unwrap();
    let mut expected = IndexPolicy::default();
    let transaction = db.0.unchecked_transaction().unwrap();
    for index in 0..1026 {
        let relative = if index == 0 {
            PathBuf::from(OsString::from_vec(b"private_\xff".to_vec()))
        } else {
            PathBuf::from(format!("private-{index}"))
        };
        expected.excluded.push(source.join(&relative));
        let rule = DriveRule {
            id: format!("rule-{index}"),
            drive: drive.id.clone(),
            relative,
            catalog: true,
            index: false,
        };
        db.put("rule", &rule.id, &rule).unwrap();
    }
    transaction.commit().unwrap();
    fs::write(
        state.join("expected-policy.json"),
        serde_json::to_vec(&expected).unwrap(),
    )
    .unwrap();
    let service = fixture.start_authority(None, fixture.image_authority());
    service.shutdown().unwrap();
    let mut location = OsString::from("LITTLE_CLIP_DRIVE_STATE_DIR=");
    location.push(&state);
    let result = CommandRequest::new("env")
        .arg(location)
        .arg("LITTLE_CLIP_DRIVE_POLICY_PROBE=1")
        .arg(std::env::current_exe().unwrap())
        .args([
            "service::tests::policy_coverage::index_policy_client_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .timeout(Duration::from_secs(15))
        .run()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        result.stdout_lossy(),
        result.stderr_lossy()
    );
    assert_eq!(
        result
            .stdout_lossy()
            .matches("POLICY_CLIENT_VERIFIED")
            .count(),
        1
    );
}

#[test]
fn exact_rule_reads_preserve_raw_addresses_and_do_not_create_operations() {
    let (fixture, drive, db) = policy_fixture();
    let selected = drive.id.clone();
    let original = [
        DriveRule {
            id: rule_id(&selected, Path::new("")),
            drive: selected.clone(),
            relative: PathBuf::new(),
            catalog: false,
            index: true,
        },
        DriveRule {
            id: rule_id(
                &selected,
                &PathBuf::from(OsString::from_vec(b"private_\xff".to_vec())),
            ),
            drive: selected.clone(),
            relative: OsString::from_vec(b"private_\xff".to_vec()).into(),
            catalog: true,
            index: false,
        },
    ];
    let transaction = db.0.unchecked_transaction().unwrap();
    for rule in &original {
        db.put("rule", &rule.id, rule).unwrap();
    }
    for index in 0..1025 {
        let rule = DriveRule {
            id: format!("other-{index}"),
            drive: "other-drive".into(),
            relative: format!("folder-{index}").into(),
            catalog: true,
            index: true,
        };
        db.put("rule", &rule.id, &rule).unwrap();
    }
    transaction.commit().unwrap();
    let service = fixture.start_authority(None, fixture.image_authority());
    assert!(
        status(&service)
            .rules
            .iter()
            .all(|rule| rule.drive != selected)
    );
    for expected in &original {
        let reply = service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: expected.relative.clone(),
            })
            .unwrap();
        let DriveReply::Rule(Some(actual)) = reply else {
            panic!("missing exact rule");
        };
        assert_eq!(actual.relative, expected.relative);
        assert_eq!(
            (actual.catalog, actual.index),
            (expected.catalog, expected.index)
        );
    }
    assert!(matches!(
        service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: "no-override".into()
            })
            .unwrap(),
        DriveReply::Rule(None)
    ));
    assert!(
        service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: "../escape".into()
            })
            .is_err()
    );
    let mut invalid = original[0].clone();
    invalid.relative = "wrong-address".into();
    db.put("rule", &invalid.id, &invalid).unwrap();
    assert!(
        service
            .request(DriveRequest::GetRule {
                drive: selected,
                relative: PathBuf::new()
            })
            .is_err()
    );
    assert!(status(&service).operations.is_empty());
    service.shutdown().unwrap();
}

#[test]
#[ignore = "owned subprocess probe; requires its explicitly selected policy fixture"]
fn index_policy_client_probe() {
    assert_eq!(
        std::env::var("LITTLE_CLIP_DRIVE_POLICY_PROBE").unwrap(),
        "1"
    );
    let state = little_clip_core::drives::state_directory();
    let expected: IndexPolicy =
        serde_json::from_slice(&fs::read(state.join("expected-policy.json")).unwrap()).unwrap();
    let allowed = state.parent().unwrap().join("ordinary-data/file.txt");
    fs::create_dir_all(allowed.parent().unwrap()).unwrap();
    fs::write(&allowed, b"ordinary fixture data").unwrap();
    let policy = little_clip_core::drives::index_policy().unwrap();
    for path in &expected.excluded {
        assert!(
            policy.excluded.contains(path),
            "Missing exclusion: {path:?}"
        );
        assert!(little_clip_core::drives::index_excluded(
            &path.join("child.txt")
        ));
    }
    assert!(!little_clip_core::drives::index_excluded(&allowed));
    let registry = state.join("index-policy.json");
    let original = fs::read(&registry).unwrap();
    for invalid in [b"malformed".to_vec(), vec![b' '; 1024 * 1024 + 1]] {
        fs::write(&registry, invalid).unwrap();
        assert!(little_clip_core::drives::index_policy().is_none());
        assert!(little_clip_core::drives::index_excluded(&allowed));
    }
    fs::write(&registry, original).unwrap();
    assert!(!little_clip_core::drives::index_excluded(&allowed));
    println!("POLICY_CLIENT_VERIFIED");
}

#[test]
fn device_policy_selectors_bind_to_the_persistent_identity_and_remain_editable_offline() {
    let (fixture, drive, _) = policy_fixture();
    let service = fixture.start_authority(None, fixture.image_authority());
    let selector = drive.device.to_str().unwrap().to_owned();
    finish(
        &service,
        DriveRequest::Rule {
            drive: selector.clone(),
            relative: "private".into(),
            catalog: false,
            index: false,
        },
    );
    for selected in [&selector, &drive.id] {
        let DriveReply::Rule(Some(rule)) = service
            .request(DriveRequest::GetRule {
                drive: selected.clone(),
                relative: "private".into(),
            })
            .unwrap()
        else {
            panic!("Missing bound rule for {selected}")
        };
        assert_eq!(rule.drive, drive.id);
        assert!(!rule.catalog && !rule.index);
    }
    assert!(
        published_policy(&service)
            .excluded
            .contains(&drive.mount.unwrap().join("private"))
    );
    fixture.drives.lock().unwrap().clear();
    finish(
        &service,
        DriveRequest::Rule {
            drive: drive.id.clone(),
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    let DriveReply::Rule(Some(rule)) = service
        .request(DriveRequest::GetRule {
            drive: drive.id,
            relative: "private".into(),
        })
        .unwrap()
    else {
        panic!("Offline rule disappeared")
    };
    assert!(rule.catalog && !rule.index);
    assert!(
        service
            .request(DriveRequest::GetRule {
                drive: selector,
                relative: "private".into()
            })
            .is_err()
    );
    service.shutdown().unwrap();
}

#[test]
fn equivalent_rule_paths_share_one_address_and_legacy_duplicates_preserve_exclusions() {
    let (fixture, drive, db) = policy_fixture();
    let canonical = "private/child";
    for (path, catalog, index) in [("private//child/", false, true), (canonical, true, false)] {
        let id = format!("{}:{path}", drive.id);
        db.put(
            "rule",
            &id,
            &DriveRule {
                id: id.clone(),
                drive: drive.id.clone(),
                relative: path.into(),
                catalog,
                index,
            },
        )
        .unwrap();
    }
    let service = fixture.start_authority(None, fixture.image_authority());
    for path in [canonical, "private//child/", "private/./child"] {
        let DriveReply::Rule(Some(rule)) = service
            .request(DriveRequest::GetRule {
                drive: drive.id.clone(),
                relative: path.into(),
            })
            .unwrap()
        else {
            panic!("Missing equivalent rule {path}")
        };
        assert!(!rule.catalog && !rule.index, "{rule:?}");
        assert_eq!(rule.relative.as_os_str(), std::ffi::OsStr::new(canonical));
    }
    assert_eq!(db.list::<DriveRule>("rule", 0, 100).unwrap().len(), 1);
    assert!(
        published_policy(&service)
            .excluded
            .contains(&drive.mount.unwrap().join(canonical))
    );
    finish(
        &service,
        DriveRequest::Rule {
            drive: drive.id.clone(),
            relative: "private//child/".into(),
            catalog: true,
            index: true,
        },
    );
    let rules = db.list::<DriveRule>("rule", 0, 100).unwrap();
    assert_eq!(rules.len(), 1);
    assert!(rules[0].catalog && rules[0].index);
    service.shutdown().unwrap();
    let service = fixture.start_authority(None, fixture.image_authority());
    let DriveReply::Rule(Some(rule)) = service
        .request(DriveRequest::GetRule {
            drive: drive.id,
            relative: canonical.into(),
        })
        .unwrap()
    else {
        panic!("Canonical rule lost after restart")
    };
    assert!(rule.catalog && rule.index);
    service.shutdown().unwrap();
}

#[test]
fn policy_writes_refuse_a_colliding_saved_key_instead_of_overwriting_another_address() {
    let (fixture, drive, db) = policy_fixture();
    let service = fixture.start_authority(None, fixture.image_authority());
    let id = rule_id(&drive.id, Path::new("private"));
    let other = DriveRule {
        id: id.clone(),
        drive: "another-drive".into(),
        relative: "kept".into(),
        catalog: false,
        index: false,
    };
    db.put("rule", &id, &other).unwrap();
    let operation = completed(
        &service,
        DriveRequest::Rule {
            drive: drive.id,
            relative: "private".into(),
            catalog: true,
            index: true,
        },
    );
    assert_eq!(operation.state, "failed", "{}", operation.message);
    let retained: DriveRule = db.get("rule", &id).unwrap().unwrap();
    assert_eq!(retained.drive, other.drive);
    assert_eq!(retained.relative, other.relative);
    assert!(!retained.catalog && !retained.index);
    service.shutdown().unwrap();
}

#[test]
fn policy_device_selectors_refuse_cloned_persistent_identities() {
    let (fixture, drive, db) = policy_fixture();
    // Explicit device selection cannot disambiguate a policy which would later
    // apply automatically to both partitions with the same persistent key.
    fixture.drives.lock().unwrap()[1].id = drive.id.clone();
    let service = fixture.start_authority(None, fixture.image_authority());
    let selector = drive.device.to_str().unwrap().to_owned();
    assert!(
        service
            .request(DriveRequest::GetRule {
                drive: selector.clone(),
                relative: PathBuf::new()
            })
            .is_err()
    );
    assert!(
        service
            .request(DriveRequest::Rule {
                drive: selector,
                relative: PathBuf::new(),
                catalog: false,
                index: false
            })
            .is_err()
    );
    assert!(db.list::<DriveRule>("rule", 0, 100).unwrap().is_empty());
    service.shutdown().unwrap();
}

#[test]
fn accepted_policy_binds_before_a_device_node_is_reused_and_retry_keeps_the_original_binding() {
    let (fixture, drive, _) = policy_fixture();
    let service = fixture.start_authority(None, fixture.image_authority());
    let mut intercepted = service.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    intercepted.sender = sender;
    let key = uuid::Uuid::new_v4();
    let request = DriveRequest::Rule {
        drive: drive.device.to_str().unwrap().into(),
        relative: "private//child/".into(),
        catalog: false,
        index: false,
    };
    let DriveReply::Accepted(id) = intercepted.submit(key, request.clone()).unwrap() else {
        panic!("Missing acceptance")
    };
    let envelope = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    let DriveRequest::Rule {
        drive: bound,
        relative,
        ..
    } = &envelope.request
    else {
        panic!("Wrong queued request")
    };
    assert_eq!(bound, &drive.id);
    assert_eq!(relative.as_os_str(), std::ffi::OsStr::new("private/child"));
    fixture.drives.lock().unwrap()[0].id = "part:replacement".into();
    service.sender.send(envelope).unwrap();
    let operation = wait_operation(&service, &id, |op| op.finished.is_some());
    assert_eq!(operation.state, "complete", "{}", operation.message);
    let DriveReply::Accepted(retried) = service.submit(key, request).unwrap() else {
        panic!("Missing replay")
    };
    assert_eq!(retried, id);
    for (selector, present) in [(drive.id, true), ("part:replacement".into(), false)] {
        let DriveReply::Rule(rule) = service
            .request(DriveRequest::GetRule {
                drive: selector,
                relative: "private/child".into(),
            })
            .unwrap()
        else {
            panic!("Wrong reply")
        };
        assert_eq!(rule.is_some(), present);
    }
    service.shutdown().unwrap();
}

#[test]
fn rule_alias_migration_spans_pages_and_rolls_back_completely_on_install_failure() {
    let (_fixture, drive, db) = policy_fixture();
    for index in 0..1100 {
        let relative = if index == 0 {
            PathBuf::from(OsString::from_vec(b"private_\xff//child/".to_vec()))
        } else {
            format!("folder-{index}//child/").into()
        };
        let id = format!(
            "{}:{}",
            drive.id,
            little_clip_core::path_address::display_path(&relative)
        );
        db.put(
            "rule",
            &id,
            &DriveRule {
                id: id.clone(),
                drive: drive.id.clone(),
                relative,
                catalog: false,
                index: false,
            },
        )
        .unwrap();
    }
    db.0.execute_batch("CREATE TRIGGER fail_rule_address_install BEFORE INSERT ON entities WHEN NEW.kind='rule' AND NEW.id LIKE '%:folder-1001/child' BEGIN SELECT RAISE(ABORT,'owned rule migration failure'); END").unwrap();
    let error = rules::migrate(&db).unwrap_err();
    assert!(
        error.to_string().contains("owned rule migration failure"),
        "{error}"
    );
    assert!(
        db.get::<u32>("migration", "rule-address")
            .unwrap()
            .is_none()
    );
    let count: u64 =
        db.0.query_row(
            "SELECT count(*) FROM entities WHERE kind='rule'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1100);
    assert!(
        db.get::<DriveRule>("rule", &format!("{}:folder-1//child/", drive.id))
            .unwrap()
            .is_some()
    );
    assert!(
        rules::get(&db, &drive.id, Path::new("folder-1/child"))
            .unwrap()
            .is_none()
    );
    db.0.execute_batch("DROP TRIGGER fail_rule_address_install")
        .unwrap();
    rules::migrate(&db).unwrap();
    assert_eq!(db.get::<u32>("migration", "rule-address").unwrap(), Some(1));
    for index in 0..1100 {
        let relative = if index == 0 {
            PathBuf::from(OsString::from_vec(b"private_\xff/child".to_vec()))
        } else {
            format!("folder-{index}/child").into()
        };
        let rule = rules::get(&db, &drive.id, &relative).unwrap().unwrap();
        assert_eq!(rule.relative.as_os_str(), relative.as_os_str());
        assert!(!rule.catalog && !rule.index);
    }
    rules::migrate(&db).unwrap();
    let count: u64 =
        db.0.query_row(
            "SELECT count(*) FROM entities WHERE kind='rule'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1100);
}

#[test]
fn slow_policy_identity_preparation_holds_neither_admission_nor_sqlite_write_ownership() {
    let (fixture, drive, db) = policy_fixture();
    let service = fixture.start_authority(None, fixture.image_authority());
    let mut preparing = service.clone();
    let inventory = preparing.inventory.clone();
    let (entered, release, gate) = worker_gate();
    preparing.inventory = Arc::new(move || {
        gate();
        inventory()
    });
    std::thread::scope(|scope| {
        let pending = scope.spawn(|| {
            preparing.request(DriveRequest::Rule {
                drive: drive.device.to_str().unwrap().into(),
                relative: PathBuf::new(),
                catalog: false,
                index: false,
            })
        });
        entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let (sent, received) = mpsc::channel();
        let controls = &service;
        scope.spawn(move || {
            finish(
                controls,
                DriveRequest::Configure {
                    config: DriveConfig::default(),
                },
            );
            sent.send(()).unwrap();
        });
        let responsive = received.recv_timeout(Duration::from_secs(3));
        let unlocked = db.0.execute_batch("BEGIN IMMEDIATE; ROLLBACK");
        release.send(()).unwrap();
        let accepted = pending.join().unwrap().unwrap();
        assert!(
            responsive.is_ok(),
            "Selector preparation blocked independent admission"
        );
        unlocked.expect("Selector preparation held the SQLite write transaction");
        let DriveReply::Accepted(id) = accepted else {
            panic!("Missing prepared request")
        };
        assert_eq!(
            wait_operation(&service, &id, |op| op.finished.is_some()).state,
            "complete"
        );
    });
    service.shutdown().unwrap();
}

#[test]
fn policy_addresses_refuse_nul_and_parent_escapes_without_saving_rules() {
    let (fixture, drive, db) = policy_fixture();
    let service = fixture.start_authority(None, fixture.image_authority());
    for path in ["../private", "private/../../escape", "private\0/child"] {
        assert!(
            service
                .request(DriveRequest::GetRule {
                    drive: drive.id.clone(),
                    relative: path.into()
                })
                .is_err()
        );
        assert!(
            service
                .request(DriveRequest::Rule {
                    drive: drive.id.clone(),
                    relative: path.into(),
                    catalog: false,
                    index: false
                })
                .is_err()
        );
    }
    assert!(db.list::<DriveRule>("rule", 0, 100).unwrap().is_empty());
    assert!(status(&service).operations.is_empty());
    service.shutdown().unwrap();
}

#[test]
fn failed_policy_observation_installs_a_fail_closed_publication_and_can_recover() {
    let (fixture, drive, _) = policy_fixture();
    let source = drive.mount.as_ref().unwrap();
    std::os::unix::fs::symlink("loop", source.join("loop")).unwrap();
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Rule {
            drive: drive.id.clone(),
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    let verified = published_policy(&service);
    let request = DriveRequest::Rule {
        drive: drive.id,
        relative: "loop".into(),
        catalog: true,
        index: false,
    };
    assert_policy_paused(
        &service,
        request.clone(),
        &verified,
        "Background indexing paused",
    );
    fs::remove_file(source.join("loop")).unwrap();
    let recovered = completed(&service, request);
    assert_eq!(recovered.state, "complete", "{}", recovered.message);
    let policy = published_policy(&service);
    assert!(policy.context.is_some());
    assert!(policy.excluded.contains(&source.join("loop")));
    service.shutdown().unwrap();
}

#[test]
fn held_policy_observation_coalesces_rules_without_blocking_controls_or_sync() {
    let (fixture, drive, _) = policy_fixture();
    let (entered, release, gate) = worker_gate();
    let held = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let observations = seen.clone();
    let publisher = publication::Publication::new(Arc::new(move |input, previous| {
        let selected = input
            .rules
            .iter()
            .filter(|rule| {
                rule.relative == Path::new("private") || rule.relative.starts_with("queued")
            })
            .count();
        observations.lock().unwrap().push(selected);
        if selected == 1 && !held.swap(true, Ordering::AcqRel) {
            gate();
        }
        publication::observe(input, previous)
    }));
    let service = fixture.start_with_publication(
        DriveConfig {
            ram_min_available: 0,
            sync_interval_seconds: 3600,
            backup_directory: Some(fixture.temp.path().join("backup")),
            ..Default::default()
        },
        None,
        fixture.image_authority(),
        publisher,
    );
    let local = fixture.pair(&service, "large", "");
    finish(
        &service,
        DriveRequest::Rule {
            drive: drive.id.clone(),
            relative: "seed".into(),
            catalog: true,
            index: false,
        },
    );
    let first = accept(
        &service,
        DriveRequest::Rule {
            drive: drive.id.clone(),
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_unfinished(&service, &first);
    let started = Instant::now();
    let mut config = status(&service).config;
    config.sync_interval_seconds = 3601;
    finish(&service, DriveRequest::Configure { config });
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "Control waited for held policy metadata"
    );
    let portable = fixture.drives.lock().unwrap()[1].mount.clone().unwrap();
    fs::write(local.join("independent"), b"sync remains live").unwrap();
    wait_contents(&portable.join("independent"), b"sync remains live");
    let mut later = Vec::new();
    for number in 0..15 {
        let id = accept(
            &service,
            DriveRequest::Rule {
                drive: drive.id.clone(),
                relative: format!("queued/{number}").into(),
                catalog: true,
                index: false,
            },
        );
        wait_operation(&service, &id, |operation| operation.state == "publishing");
        later.push(id);
    }
    // Sixteen applied rules may await one observer. A further request stays
    // queued, and cancelling it must not wait or mutate the saved policy.
    let queued = accept(
        &service,
        DriveRequest::Rule {
            drive: drive.id.clone(),
            relative: "not-applied".into(),
            catalog: true,
            index: false,
        },
    );
    service
        .request(DriveRequest::Cancel {
            operation: queued.clone(),
        })
        .unwrap();
    let cancelled = wait_operation(&service, &queued, |operation| operation.finished.is_some());
    assert_eq!(cancelled.state, "cancelled", "{}", cancelled.message);
    assert!(matches!(
        service
            .request(DriveRequest::GetRule {
                drive: drive.id.clone(),
                relative: "not-applied".into(),
            })
            .unwrap(),
        DriveReply::Rule(None)
    ));
    assert!(
        published_policy(&service)
            .excluded
            .contains(&PathBuf::from("/"))
    );
    service
        .request(DriveRequest::Cancel {
            operation: first.clone(),
        })
        .unwrap();
    assert_unfinished(&service, &first);
    release.send(()).unwrap();
    for id in std::iter::once(&first).chain(&later) {
        let result = wait_operation(&service, id, |operation| operation.finished.is_some());
        assert_eq!(result.state, "complete", "{}", result.message);
    }
    let policy = published_policy(&service);
    assert!(!policy.excluded.contains(&PathBuf::from("/")));
    for relative in std::iter::once("private".to_owned())
        .chain((0..15).map(|number| format!("queued/{number}")))
    {
        assert!(
            policy
                .excluded
                .contains(&drive.mount.as_ref().unwrap().join(relative))
        );
    }
    service.shutdown().unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen.contains(&1) && seen.contains(&16), "{seen:?}");
    assert!(
        !seen.iter().any(|count| (2..16).contains(count)),
        "Intermediate policy backlog was replayed: {seen:?}"
    );
}

#[test]
fn shutdown_retains_policy_ownership_until_observation_and_rule_completion_exit() {
    let (fixture, drive, _) = policy_fixture();
    let (entered, release, gate) = worker_gate();
    let publication_thread = Arc::new(Mutex::new(None));
    let published = publication_thread.clone();
    let publisher = policy_observer_once("private", move || {
        *published.lock().unwrap() = Some(std::thread::current().id());
        gate();
    });
    let tracking = Arc::new(AtomicBool::new(false));
    let tracked = tracking.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_calls = calls.clone();
    let drives = fixture.drives.clone();
    let caller = std::thread::current().id();
    let (late_entered, late_release, late_gate) = worker_gate();
    let service = fixture.start_with_publication(
        DriveConfig {
            ram_min_available: u64::MAX,
            ..Default::default()
        },
        Some(Arc::new(move || {
            let current = std::thread::current();
            if tracked.load(Ordering::Acquire)
                && current.name() != Some("portable-drive-owner")
                && current.id() != caller
                && Some(current.id()) != *publication_thread.lock().unwrap()
            {
                let count = observed_calls.fetch_add(1, Ordering::AcqRel) + 1;
                let mut drives = drives.lock().unwrap();
                drives[0].label = format!("shutdown-status-{count}");
                if count > 1 {
                    // A forbidden late probe would retain this mutex and block
                    // the final policy's fresh inventory until released.
                    late_gate();
                }
            }
        })),
        fixture.image_authority(),
        publisher,
    );
    let request = DriveRequest::Rule {
        drive: drive.id,
        relative: "private".into(),
        catalog: true,
        index: false,
    };
    let identity = uuid::Uuid::new_v4();
    let DriveReply::Accepted(id) = service.submit(identity, request.clone()).unwrap() else {
        panic!("expected accepted rule");
    };
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    tracking.store(true, Ordering::Release);
    let draining = service.clone();
    let (exited, completed) = mpsc::channel();
    let shutdown = std::thread::spawn(move || {
        let result = draining.shutdown();
        exited.send(()).unwrap();
        result
    });
    assert_shutdown_retained(&service, &completed);
    assert_unfinished(&service, &id);
    let config = status(&service).config;
    let observed = Instant::now();
    while !status(&service)
        .drives
        .iter()
        .any(|drive| drive.label == "shutdown-status-1")
    {
        assert!(
            observed.elapsed() < Duration::from_secs(5),
            "Final status observation did not retire"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    release.send(()).unwrap();
    completed.recv_timeout(Duration::from_secs(5)).unwrap();
    shutdown.join().unwrap().unwrap();
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "Completing a closing rule queued a new status probe"
    );
    assert!(late_entered.try_recv().is_err());
    let _ = late_release.send(());
    let result = wait_operation(&service, &id, |operation| operation.finished.is_some());
    assert_eq!(result.state, "complete", "{}", result.message);
    let drives = fixture.drives.clone();
    let restarted = Service::start_with_inventory(
        config,
        service.state.clone(),
        Arc::new(move || Ok(drives.lock().unwrap().clone())),
    )
    .unwrap();
    assert!(
        matches!(restarted.submit(identity, request).unwrap(), DriveReply::Accepted(replayed) if replayed == id)
    );
    assert_eq!(
        wait_operation(&restarted, &id, |operation| operation.finished.is_some()).state,
        "complete"
    );
    restarted.shutdown().unwrap();
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; suspends only its owned FUSE child"]
fn stalled_fuse_policy_metadata_leaves_controls_and_another_pair_responsive() {
    let fixture = TestDrives::new(0);
    let (backing, source_id, mut native, source) = fixture.native_source("partition");
    let (entered, observing) = mpsc::channel();
    let publisher = policy_observer_once("unseen-policy-boundary", move || {
        // SAFETY: gettid has no arguments or memory effects.
        entered
            .send(unsafe { libc::syscall(libc::SYS_gettid) })
            .unwrap();
    });
    let service = fixture.start_with_publication(
        DriveConfig {
            sync_interval_seconds: 3600,
            ..Default::default()
        },
        None,
        fixture.image_authority(),
        publisher,
    );
    let local = fixture.pair(&service, "small", "");
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:large".into(),
            relative: "seed".into(),
            catalog: true,
            index: false,
        },
    );
    let mut accepted = None;
    native.with_suspended(|| {
        let id = accept(
            &service,
            DriveRequest::Rule {
                drive: "part:large".into(),
                relative: "unseen-policy-boundary".into(),
                catalog: true,
                index: false,
            },
        );
        let tid = observing.recv_timeout(Duration::from_secs(5)).unwrap();
        let (tid, wait) = wait_fuse_request(|| vec![tid]);
        eprintln!("POLICY_FUSE_METADATA_BLOCKED tid={tid} wchan={wait}");
        assert_unfinished(&service, &id);
        assert!(
            published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
        let mut config = status(&service).config;
        config.sync_interval_seconds = 3601;
        let started = Instant::now();
        finish(&service, DriveRequest::Configure { config });
        assert!(started.elapsed() < Duration::from_secs(2));
        fs::write(
            local.join("independent"),
            b"sync through a real metadata stall",
        )
        .unwrap();
        let portable = fixture.temp.path().join("small/independent");
        wait_contents(&portable, b"sync through a real metadata stall");
        assert_unfinished(&service, &id);
        accepted = Some(id);
    });
    let id = accepted.unwrap();
    let completed = wait_operation(&service, &id, |operation| operation.finished.is_some());
    assert_eq!(completed.state, "complete", "{}", completed.message);
    let policy = published_policy(&service);
    assert!(!policy.excluded.contains(&PathBuf::from("/")));
    assert!(
        policy
            .excluded
            .contains(&source.join("unseen-policy-boundary"))
    );
    service.shutdown().unwrap();
    drop(native);
    crate::ram::discard(&backing, &source_id).unwrap();
}
