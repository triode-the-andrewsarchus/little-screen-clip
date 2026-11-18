//! Live reload changes policy without shrinking or evicting retained RAM copies.
use super::*;

fn retained(service: &Service, copies: &[RamSession]) {
    let observed = status(service);
    for copy in copies {
        let current = observed.ram.iter().find(|ram| ram.id == copy.id).unwrap();
        assert_eq!(current.path, copy.path);
        let backing = current.backing.as_ref().unwrap();
        let before = copy.backing.as_ref().unwrap();
        assert_eq!(
            (backing.capacity, backing.inodes),
            (before.capacity, before.inodes)
        );
        assert_eq!(
            fs::read(copy.path.join("quota-dirty.txt")).unwrap(),
            b"retained quota edit"
        );
        assert!(
            observed
                .pairs
                .iter()
                .find(|pair| pair.id == copy.id)
                .unwrap()
                .paused
        );
    }
    assert!(
        observed
            .pairs
            .iter()
            .find(|pair| pair.id == copies[0].id)
            .unwrap()
            .deletions
    );
}

fn reject_reductions(service: &Service, copies: &[RamSession], previous: &DriveConfig) {
    type Change = fn(&mut DriveConfig, &RamBacking, &RamBacking);
    let changes: [(&str, Change); 5] = [
        ("session-bytes", |c, a, _| {
            c.ram_session_max_bytes = a.capacity - 1
        }),
        ("total-bytes", |c, a, b| {
            c.ram_session_max_bytes = a.capacity.max(b.capacity);
            c.ram_total_max_bytes = a.capacity + b.capacity - 1;
        }),
        ("session-inodes", |c, a, _| {
            c.ram_session_max_entries = a.inodes - 1
        }),
        ("total-inodes", |c, a, b| {
            c.ram_session_max_entries = a.inodes.max(b.inodes);
            c.ram_total_max_entries = a.inodes + b.inodes - 1;
        }),
        ("sessions", |c, _, _| c.ram_max_sessions = 1),
    ];
    let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
    let saved = db.get::<DriveConfig>("settings", "config").unwrap();
    let a = copies[0].backing.as_ref().unwrap();
    let b = copies[1].backing.as_ref().unwrap();
    // Exercise reservation validation, not just the minimum shape validator.
    assert!(a.capacity > 128 * MIB && a.inodes > 4096);
    for (name, change) in changes {
        let mut config = previous.clone();
        change(&mut config, a, b);
        policy::validate_config(&config).unwrap();
        failed(
            service,
            DriveRequest::Configure { config },
            "Aggregate or per-session RAM reservation",
        );
        assert_eq!(status(service).config, *previous);
        assert_eq!(db.get::<DriveConfig>("settings", "config").unwrap(), saved);
        retained(service, copies);
        fs::write(
            copies[0].path.join(format!("after-{name}")),
            b"existing RAM remains writable",
        )
        .unwrap();
        eprintln!("LIVE_QUOTA_REDUCTION_REFUSED={name}");
    }
}

fn exact_config(base: &DriveConfig, copies: &[RamSession], fixture: &TestDrives) -> DriveConfig {
    let a = copies[0].backing.as_ref().unwrap();
    let b = copies[1].backing.as_ref().unwrap();
    let mut config = base.clone();
    config.ram_session_max_bytes = a.capacity.max(b.capacity);
    config.ram_total_max_bytes = a.capacity + b.capacity;
    config.ram_session_max_entries = a.inodes.max(b.inodes);
    config.ram_total_max_entries = a.inodes + b.inodes;
    config.ram_max_sessions = 2;
    config.ram_directory = base.ram_directory.join("future-sessions");
    fs::create_dir(&config.ram_directory).unwrap();
    config.catalog_max_entries = 1;
    config.catalog_max_bytes = 2 * MIB;
    config.catalog_max_depth = 8;
    config.catalog_timeout_seconds = 30;
    config.catalog_workers = 2;
    config.backup_keep_versions = Some(2);
    config.sync_interval_seconds = 2;
    config.backup_directory = Some(fixture.temp.path().join("reloaded-backups"));
    config
}

fn admission(
    service: &Service,
    copies: &[RamSession],
    fixture: &TestDrives,
    config: &mut DriveConfig,
) -> RamSession {
    let source = fixture.temp.path().join("third");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("third.txt"), b"new session after reload").unwrap();
    let drive = fixture_drive(&source, "third");
    let capacity = drive.drive_capacity;
    fixture.drives.lock().unwrap().push(drive);
    let request = DriveRequest::CopyToRam {
        drive: "part:third".into(),
    };
    failed(service, request.clone(), "RAM session limit");
    config.ram_max_sessions = 3;
    config.ram_total_max_bytes += 128 * MIB;
    config.ram_total_max_entries += copies[1].backing.as_ref().unwrap().inodes;
    config.ram_min_available = u64::MAX;
    finish(
        service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    failed(service, request.clone(), "Insufficient available RAM");
    fs::write(
        copies[0].path.join("quota-live-flush.txt"),
        b"writeback with raised memory minimum",
    )
    .unwrap();
    finish(
        service,
        DriveRequest::FlushRam {
            session: copies[0].id.clone(),
        },
    );

    config.ram_min_available = 0;
    config.ram_capacity_limit = capacity;
    finish(
        service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    failed(service, request.clone(), "Drive capacity exceeds");
    fs::write(
        copies[0].path.join("quota-live-flush.txt"),
        b"writeback with reduced capacity eligibility",
    )
    .unwrap();
    finish(
        service,
        DriveRequest::FlushRam {
            session: copies[0].id.clone(),
        },
    );

    retained(service, copies);
    config.ram_capacity_limit += 1;
    finish(
        service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    finish(service, request);
    let third = status(service)
        .ram
        .into_iter()
        .find(|ram| ram.drive == "part:third")
        .unwrap();
    assert!(
        third
            .backing
            .as_ref()
            .unwrap()
            .directory
            .starts_with(&config.ram_directory)
    );
    assert_eq!(
        fs::read(third.path.join("third.txt")).unwrap(),
        b"new session after reload"
    );
    finish(
        service,
        DriveRequest::Pause {
            pair: third.id.clone(),
            paused: true,
        },
    );
    retained(service, copies);
    third
}

fn new_jobs(service: &Service, config: &DriveConfig) {
    finish(
        service,
        DriveRequest::Catalog {
            drive: "part:large".into(),
        },
    );
    let catalog = status(service).catalogs.pop().unwrap();
    assert_eq!(catalog.limits.unwrap(), CatalogLimits::from(config));
    assert_eq!(catalog.entries, 1);
    assert!(!catalog.complete);
    let request = DriveRequest::Backup {
        drive: "part:large".into(),
        kind: BackupKind::Files,
        restore_mounted: false,
    };
    for _ in 0..3 {
        finish(service, request.clone());
    }
    let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
    let current = backup::current_version(&db, "part:large", BackupKind::Files)
        .unwrap()
        .unwrap();
    assert!(
        current
            .path
            .starts_with(config.backup_directory.as_ref().unwrap())
    );
    assert_eq!(status(service).backups.len(), 3);
    finish(
        service,
        DriveRequest::PruneBackups {
            drive: "part:large".into(),
        },
    );
    assert_eq!(status(service).backups.len(), 2);
    assert!(current.path.is_dir());
}

#[test]
#[ignore = "requires vm_ram.sh bounded tmpfs and owned native source fixtures"]
fn live_quota_reloads_preserve_dirty_sessions_and_apply_new_job_limits() {
    let base = config();
    let fixture = TestDrives::new(0);
    let _native = fixture.guest_source();
    {
        let mut drives = fixture.drives.lock().unwrap();
        let drive = &mut drives[1];
        drive.capacity = 64 * MIB;
        drive.drive_capacity = drive.capacity;
        for name in ["edit", "another.txt"] {
            fs::write(
                drive.mount.as_ref().unwrap().join(name),
                b"native source data",
            )
            .unwrap();
        }
    }
    let service = fixture.start_configured(base.clone(), None, fixture.image_authority());
    let large = paused_copy(&service);
    finish(
        &service,
        DriveRequest::CopyToRam {
            drive: "part:small".into(),
        },
    );
    let small = status(&service)
        .ram
        .into_iter()
        .find(|ram| ram.id != large.id)
        .unwrap();
    let copies = [large, small];
    for copy in &copies {
        finish(
            &service,
            DriveRequest::Pause {
                pair: copy.id.clone(),
                paused: true,
            },
        );
        fs::write(copy.path.join("quota-dirty.txt"), b"retained quota edit").unwrap();
        File::open(copy.path.join("quota-dirty.txt"))
            .unwrap()
            .sync_all()
            .unwrap();
    }
    finish(
        &service,
        DriveRequest::Deletions {
            pair: copies[0].id.clone(),
            enabled: true,
        },
    );
    let held = File::open(copies[0].path.join("quota-dirty.txt")).unwrap();
    let identity = held.metadata().unwrap().ino();
    reject_reductions(&service, &copies, &base);
    let mut config = exact_config(&base, &copies, &fixture);
    finish(
        &service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    assert_eq!(status(&service).config, config);
    assert_eq!(held.metadata().unwrap().ino(), identity);
    retained(&service, &copies);
    let third = admission(&service, &copies, &fixture, &mut config);
    new_jobs(&service, &config);
    let settings = fixture.temp.path().join("config.toml");
    fs::write(
        &settings,
        toml::to_string(&little_clip_core::AppConfig {
            drives: config.clone(),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();
    drop(held);
    service.shutdown().unwrap();
    let restored = little_clip_core::AppConfig::load_from_path(&settings).unwrap();
    let observed = fixture.drives.clone();
    let restarted = Service::start_with_settings(
        restored.drives,
        service.state.clone(),
        Arc::new(move || Ok(observed.lock().unwrap().clone())),
        Some(settings),
    )
    .unwrap();
    for copy in copies.iter().chain(std::iter::once(&third)) {
        wait_ram(&restarted, &copy.id, RamSession::browsable);
    }
    assert_eq!(status(&restarted).config, config);
    retained(&restarted, &copies);
    assert_eq!(
        fs::read(third.path.join("third.txt")).unwrap(),
        b"new session after reload"
    );
    for copy in copies.iter().chain(std::iter::once(&third)) {
        discard_ram(&restarted, copy);
    }
    restarted.shutdown().unwrap();
}
