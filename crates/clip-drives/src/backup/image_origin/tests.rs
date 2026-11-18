use super::*;

#[test]
fn host_fingerprints_are_scoped_and_unknown_hosts_cannot_retire_other_boots() {
    let id = uuid::Uuid::new_v4();
    let host = machine_fingerprint(id.simple().to_string().as_bytes()).unwrap();
    assert_eq!(
        Some(host),
        machine_fingerprint(format!("{}\n", id.simple()).as_bytes())
    );
    assert_ne!(&host[..16], id.as_bytes());
    for unknown in [
        "",
        "uninitialized",
        "00000000000000000000000000000000",
        "invalid-host-id",
    ] {
        assert!(machine_fingerprint(unknown.as_bytes()).is_none());
    }
    let old = Origin {
        host: Some(host),
        boot: uuid::Uuid::new_v4(),
        namespace: Namespace::current(),
    };
    assert!(!old.previous_boot(&old).unwrap());
    let mut current = old.clone();
    current.boot = uuid::Uuid::new_v4();
    assert!(old.previous_boot(&current).unwrap());
    current.host = None;
    assert!(old.previous_boot(&current).is_err());
    current.host = Some([0; 32]);
    assert!(old.previous_boot(&current).is_err());
    current.host = old.host;
    current.namespace.mount = None;
    assert!(old.previous_boot(&current).is_err());
}

fn image() -> (super::super::tests::FileFixture, Backup, Origin, Origin) {
    let fixture = super::super::tests::FileFixture::new(b"owned image bytes");
    let input = File::open(fixture.source.path.join("file")).unwrap();
    let mut drive = fixture.drive.clone();
    drive.capacity = input.metadata().unwrap().len();
    drive.mount = None;
    let image = image_from(
        &fixture.db,
        &drive,
        &fixture.destination,
        &input,
        &fixture.cancel,
        |_, _| Ok(()),
        || Ok(()),
    )
    .unwrap();
    let old = Origin {
        host: Some([7; 32]),
        boot: uuid::Uuid::new_v4(),
        namespace: Namespace::current(),
    };
    let mut current = old.clone();
    current.boot = uuid::Uuid::new_v4();
    (fixture, image, old, current)
}

fn mappings(fixture: &super::super::tests::FileFixture, image: &Backup, origin: &Origin) -> Backup {
    let mut image = image.clone();
    // Deliberately not a directory: retirement must not open the old view path.
    image.mount = Some(image.path.clone());
    image.loop_object = Some("/owned-loop".into());
    let header = serde_json::to_value(&image).unwrap();
    let mut node = serde_json::to_value(
        crate::mounts::Lifetime::capture(&fixture.source.directory(Path::new(""), false).unwrap())
            .unwrap(),
    )
    .unwrap();
    node["boot"] = serde_json::to_value(origin.boot).unwrap();
    node["namespace"] = serde_json::to_value(&origin.namespace).unwrap();
    let mut view = node.clone();
    view["backup"] = image.id.clone().into();
    view["image"] = header["path"].clone();
    view["path"] = header["mount"].clone();
    view["object"] = "/owned-loop".into();
    let attachment = serde_json::json!({"host": origin.host, "backup":image.id,
        "image":header["path"], "identity":header["identity"], "object":"/owned-loop",
        "device":header["path"], "node":node, "rdev":libc::makedev(7,1),
        "sequence":17, "backing_device":1, "backing_inode":image.identity.as_ref().unwrap().inode});
    fixture.db.put("backup", &image.id, &image).unwrap();
    fixture.db.put("image-mount", &image.id, &view).unwrap();
    fixture
        .db
        .put("image-loop", &image.id, &attachment)
        .unwrap();
    image
}

#[test]
fn old_boot_retirement_is_metadata_only_atomic_and_rechecks_its_evidence() {
    let (fixture, image, old, current) = image();
    let before = fs::read(&image.path).unwrap();

    let image = mappings(&fixture, &image, &old);
    let evidence = Evidence::capture(&fixture.db, &image.id).unwrap();
    assert!(evidence.previous_boot(&old, &current).unwrap());
    // A failed metadata write rolls back the whole retirement.
    fixture.db.0.execute_batch("CREATE TRIGGER fail_old_image BEFORE DELETE ON entities WHEN OLD.kind='image-loop' BEGIN SELECT RAISE(FAIL,'old image fault'); END;").unwrap();
    {
        let transaction = fixture.db.0.unchecked_transaction().unwrap();
        assert!(evidence.retire(&fixture.db).is_err());
        drop(transaction);
    }
    evidence.check_current(&fixture.db).unwrap();
    fixture
        .db
        .0
        .execute_batch("DROP TRIGGER fail_old_image")
        .unwrap();
    let mut changed = image.clone();
    changed.bytes_written += 1;
    fixture.db.put("backup", &image.id, &changed).unwrap();
    assert!(evidence.retire(&fixture.db).is_err());
    fixture.db.put("backup", &image.id, &image).unwrap();
    let transaction = fixture.db.0.unchecked_transaction().unwrap();
    evidence.retire(&fixture.db).unwrap();
    transaction.commit().unwrap();
    assert_eq!(fs::read(&image.path).unwrap(), before);
}

#[test]
fn a_current_or_inconsistent_mapping_keeps_all_old_boot_records() {
    let (fixture, image, old, current) = image();

    let image = mappings(&fixture, &image, &old);
    let original = Evidence::capture(&fixture.db, &image.id).unwrap();
    for field in [
        "attachment-boot",
        "attachment-host",
        "view-boot",
        "malformed-attachment",
    ] {
        let kind = if field == "view-boot" {
            "image-mount"
        } else {
            "image-loop"
        };
        let mut row = fixture
            .db
            .get::<serde_json::Value>(kind, &image.id)
            .unwrap()
            .unwrap();
        match field {
            "attachment-boot" => row["node"]["boot"] = serde_json::to_value(current.boot).unwrap(),
            "attachment-host" => row["host"] = serde_json::to_value([9_u8; 32]).unwrap(),
            "view-boot" => row["boot"] = serde_json::to_value(current.boot).unwrap(),
            "malformed-attachment" => row = serde_json::json!(1),
            _ => unreachable!(),
        }
        fixture.db.put(kind, &image.id, &row).unwrap();
        let changed = Evidence::capture(&fixture.db, &image.id).unwrap();
        assert!(changed.previous_boot(&old, &current).is_err(), "{field}");
        changed.check_current(&fixture.db).unwrap();
        fixture
            .db
            .put(
                kind,
                &image.id,
                if kind == "image-mount" {
                    original.observation.as_ref().unwrap()
                } else {
                    original.attachment.as_ref().unwrap()
                },
            )
            .unwrap();
    }
}

#[test]
fn an_unmounted_old_view_is_retired_with_its_matching_partial_attachment() {
    let (fixture, image, old, current) = image();

    let mut image = mappings(&fixture, &image, &old);
    for (mount, permitted) in [
        (None, true),
        (Some(image.path.with_extension("foreign")), false),
    ] {
        image.mount = mount;
        fixture.db.put("backup", &image.id, &image).unwrap();
        let proof = Evidence::capture(&fixture.db, &image.id)
            .unwrap()
            .previous_boot(&old, &current);
        if permitted {
            assert!(proof.unwrap());
        } else {
            assert!(proof.is_err());
        }
    }
}

#[test]
fn old_attachment_admission_preserves_the_origin_until_recovery() {
    let (fixture, image, old, current) = image();
    let image = mappings(&fixture, &image, &old);
    let evidence = Evidence::capture(&fixture.db, &image.id).unwrap();
    assert!(evidence.check_control_boot(old.boot).is_ok());
    assert!(evidence.check_control_boot(current.boot).is_err());
    evidence.check_current(&fixture.db).unwrap();
}
