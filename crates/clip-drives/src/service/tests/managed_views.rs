//! Uncertain managed boundaries pause optional discovery, while repair is retriable.
use super::*;

#[test]
fn missing_registered_headers_pause_indexing_and_recover_after_repair() {
    use std::sync::atomic::AtomicUsize;
    let fixture = TestDrives::new(0);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let publisher = publication::Publication::new(Arc::new(move |input, previous| {
        observed.fetch_add(1, Ordering::AcqRel);
        publication::observe(input, previous)
    }));
    let service = fixture.start_with_publication(
        DriveConfig::default(),
        None,
        fixture.image_authority(),
        publisher,
    );
    let request = DriveRequest::Rule {
        drive: "part:small".into(),
        relative: "private".into(),
        catalog: true,
        index: false,
    };
    finish(&service, request.clone());
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    for registry in ["catalog-mount", "image-mount"] {
        let previous = published_policy(&service);
        let before = calls.load(Ordering::Acquire);
        db.put(registry, "missing-header", &1_u64).unwrap();
        assert_policy_paused(&service, request.clone(), &previous, "no saved header");
        assert_eq!(
            calls.load(Ordering::Acquire),
            before,
            "Known uncertain metadata must not reach the filesystem observer"
        );
        db.remove(registry, "missing-header").unwrap();
        finish(&service, request.clone());
        assert!(
            !published_policy(&service)
                .excluded
                .contains(&PathBuf::from("/"))
        );
    }
    service.shutdown().unwrap();
}

#[test]
fn catalog_boundary_guards_ram_setup_and_external_unmount_retires_its_registration() {
    let fixture = TestDrives::new(0);
    let source = fixture.temp.path().join("small");
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Catalog {
            drive: "part:small".into(),
        },
    );
    let mut catalog = status(&service).catalogs.pop().unwrap();
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    // A saved legacy boundary is protective even before/after the helper exists.
    catalog.mount = Some(source.clone());
    db.put("catalog", &catalog.id, &catalog).unwrap();
    db.put("catalog-mount", &catalog.id, &1_u64).unwrap();
    failed(
        &service,
        DriveRequest::CopyToRam {
            drive: "part:small".into(),
        },
        "managed",
    );
    assert!(status(&service).ram.is_empty());
    assert_eq!(fs::read(source.join("edit")).unwrap(), b"initial");
    db.0.execute_batch(
        "CREATE TRIGGER retirement_fault BEFORE UPDATE ON entities
        WHEN NEW.kind='catalog' BEGIN SELECT RAISE(FAIL,'catalog retirement fixture fault'); END;",
    )
    .unwrap();
    failed(
        &service,
        DriveRequest::UnmountCatalog {
            catalog: catalog.id.clone(),
        },
        "catalog retirement fixture fault",
    );
    assert!(
        db.get::<u64>("catalog-mount", &catalog.id)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        db.get::<Catalog>("catalog", &catalog.id)
            .unwrap()
            .unwrap()
            .mount
            .as_ref(),
        Some(&source)
    );
    db.0.execute_batch("DROP TRIGGER retirement_fault").unwrap();
    finish(
        &service,
        DriveRequest::UnmountCatalog {
            catalog: catalog.id.clone(),
        },
    );
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
    assert!(!policy::mounted_views(&db).unwrap().contains(&source));
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    assert!(
        !published_policy(&service)
            .excluded
            .contains(&PathBuf::from("/"))
    );
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires the disposable native-filesystem guest, catalog helper and real bind mounts"]
fn native_catalog_alias_remains_managed_after_its_original_mount_is_removed() {
    crate::require_disposable_guest();
    let native = crate::sync_edge_tests::native_fs::guest_drive();
    let fixture = TestDrives::new(0);
    let source = native
        .mount
        .unwrap()
        .join(format!("managed-view-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&source).unwrap();
    fs::write(source.join("edit"), b"portable data").unwrap();
    fixture.drives.lock().unwrap()[0].mount = Some(source.clone());
    let service = fixture.start_authority(None, fixture.image_authority());
    finish(
        &service,
        DriveRequest::Catalog {
            drive: "part:small".into(),
        },
    );
    let catalog = status(&service).catalogs.pop().unwrap();
    finish(
        &service,
        DriveRequest::MountCatalog {
            catalog: catalog.id.clone(),
        },
    );
    let view = status(&service).catalogs.pop().unwrap().mount.unwrap();
    let alias = fixture.temp.path().join("catalog-alias");
    fs::create_dir(&alias).unwrap();
    let bind = crate::sync_edge_tests::native_fs::GuestMount::attach(&view, &alias);
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let alias_root = Root::open(&alias).unwrap();
    assert!(policy::validate_view_target(&db, &alias_root).is_err());
    finish(
        &service,
        DriveRequest::UnmountCatalog {
            catalog: catalog.id,
        },
    );
    assert!(policy::mounted_views(&db).unwrap().is_empty());
    // The alias has no remaining saved mount boundary or registration. Its
    // deny-only kernel source label still prevents it becoming user storage.
    let storage = policy::Storage::capture(&db, &service.state, None).unwrap();
    assert!(storage.validate_source(&alias_root).is_err());
    assert!(policy::validate_view_target(&db, &alias_root).is_err());
    finish(
        &service,
        DriveRequest::Rule {
            drive: "part:small".into(),
            relative: "private".into(),
            catalog: true,
            index: false,
        },
    );
    let published = published_policy(&service);
    assert!(published.excluded.contains(&alias));
    assert!(!published.excluded.contains(&PathBuf::from("/")));
    assert_eq!(fs::read(source.join("edit")).unwrap(), b"portable data");
    drop(alias_root);
    drop(bind);
    service.shutdown().unwrap();
    fs::remove_dir_all(source).unwrap();
}
