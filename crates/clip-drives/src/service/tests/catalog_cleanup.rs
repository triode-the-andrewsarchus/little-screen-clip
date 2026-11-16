use super::*;

#[test]
fn pending_catalog_exclusions_allow_sync_and_resume_after_daemon_restart_and_reader_release() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("portable");
    let local = temp.path().join("local");
    let state = temp.path().join("state");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&local).unwrap();
    fs::create_dir(source.join("private")).unwrap();
    fs::write(source.join("private/secret"), b"retained payload").unwrap();
    fs::write(source.join("live.txt"), b"before").unwrap();
    let drive = fixture_drive(&source, "catalog-cleanup");
    let selected = drive.id.clone();
    let inventory: Inventory = Arc::new(move || Ok(vec![drive.clone()]));
    let service =
        Service::start_with_inventory(DriveConfig::default(), state.clone(), inventory.clone())
            .unwrap();
    finish(
        &service,
        DriveRequest::Pair {
            drive: selected.clone(),
            relative: PathBuf::new(),
            local: local.clone(),
        },
    );
    finish(
        &service,
        DriveRequest::Catalog {
            drive: selected.clone(),
        },
    );
    let catalog = status(&service).catalogs.pop().unwrap();
    let lease = catalog::redaction::Lease::acquire(
        Arc::new(Root::open(&state).unwrap()),
        &catalog.id,
        false,
    )
    .unwrap()
    .unwrap();
    finish(
        &service,
        DriveRequest::Rule {
            drive: selected,
            relative: "private".into(),
            catalog: false,
            index: false,
        },
    );
    assert_eq!(status(&service).catalogs[0].pending_redactions, 1);
    fs::write(
        source.join("live.txt"),
        b"while catalog reader holds its snapshot",
    )
    .unwrap();
    wait_contents(
        &local.join("live.txt"),
        b"while catalog reader holds its snapshot",
    );
    assert_eq!(status(&service).catalogs[0].pending_redactions, 1);
    service.shutdown().unwrap();
    drop(service);
    // The cleanup is state-only: it still runs with the original source absent.
    fs::rename(&source, temp.path().join("removed-portable")).unwrap();
    let service =
        Service::start_with_inventory(DriveConfig::default(), state.clone(), inventory).unwrap();
    assert_eq!(status(&service).catalogs[0].pending_redactions, 1);
    drop(lease);
    let start = Instant::now();
    let updated = loop {
        let current = status(&service).catalogs.pop().unwrap();
        if current.pending_redactions == 0 {
            break current;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "cleanup did not resume: {:?}",
            current.redaction_error
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(updated.captured, catalog.captured);
    assert!(updated.redacted.is_some());
    let db = Store::existing(
        &state
            .join("catalogs")
            .join(format!("{}.sqlite", catalog.id)),
        true,
    )
    .unwrap();
    assert!(
        scan::entry(&db, "catalog", Path::new("private/secret"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fs::read(temp.path().join("removed-portable/private/secret")).unwrap(),
        b"retained payload"
    );
    service.shutdown().unwrap();
}
