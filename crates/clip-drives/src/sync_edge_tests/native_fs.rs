//! Guest-only application checks on actual FAT32, exFAT and NTFS mounts.
use super::*;
use crate::{backup, catalog, scan};
use std::{
    fs::{File, FileTimes},
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

pub(crate) fn guest_drive() -> Drive {
    crate::require_disposable_guest();
    let source = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").unwrap());
    let device = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_DEVICE").unwrap());
    assert_eq!(source, Path::new("/work/media"));
    let filesystem = std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
    let root = Root::open(&source).unwrap();
    assert!(
        !root.reliable_revision_cache().unwrap(),
        "Removable filesystem comparisons must not reuse coarse or uncertain timestamps"
    );
    let identity = little_clip_core::kernel_interface::MountIdentity::pinned(
        &root.directory(Path::new(""), false).unwrap(),
    )
    .unwrap();
    let mount = crate::mounts::inventory()
        .unwrap()
        .into_iter()
        .find(|mount| mount.id == identity.mount)
        .unwrap();
    assert_eq!(mount.filesystem, filesystem);
    assert_eq!(mount.source, device);
    let uuid = std::env::var("LITTLE_CLIP_DRIVE_TEST_UUID").unwrap();
    let capacity = std::env::var("LITTLE_CLIP_DRIVE_TEST_CAPACITY")
        .unwrap()
        .parse()
        .unwrap();
    let part_uuid = std::env::var("LITTLE_CLIP_DRIVE_TEST_PARTUUID")
        .ok()
        .filter(|value| !value.is_empty());
    Drive {
        id: part_uuid
            .as_ref()
            .map(|uuid| format!("part:{uuid}"))
            .unwrap_or_else(|| format!("fs:{uuid}")),
        label: "Owned native filesystem".into(),
        device,
        mount: Some(source),
        part_uuid,
        filesystem_uuid: Some(uuid.clone()),
        filesystem,
        capacity,
        drive_capacity: capacity,
        disk_sequence: Some(
            std::env::var("LITTLE_CLIP_DRIVE_TEST_DISK_SEQUENCE")
                .unwrap()
                .parse()
                .unwrap(),
        ),
        partition_layout: None,
        removable: true,
        read_only: false,
        evidence: format!("owned-guest:{uuid}"),
        ambiguous: false,
    }
}

pub(crate) fn fixture() -> (tempfile::TempDir, Fixture, Drive) {
    let mut drive = guest_drive();
    let native = tempfile::tempdir_in(drive.mount.as_ref().unwrap()).unwrap();
    let mut fixture = Fixture::new();
    fixture.a = native.path().into();
    drive.mount = Some(fixture.a.clone());
    fixture.pair.drive = drive.id.clone();
    fixture.pair.evidence = drive.evidence.clone();
    fixture.pair.local = fixture.b.clone();
    fixture.pair.relative = fixture
        .a
        .strip_prefix(drive.mount.as_ref().unwrap())
        .unwrap()
        .into();
    (native, fixture, drive)
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_union_and_concurrent_edits_keep_both_variants() {
    let (_native, fixture, _) = fixture();
    fs::write(fixture.a.join("portable.txt"), b"portable").unwrap();
    fs::write(fixture.b.join("local.txt"), b"local").unwrap();
    fixture.run().unwrap();
    assert_eq!(
        fs::read(fixture.b.join("portable.txt")).unwrap(),
        b"portable"
    );
    assert_eq!(fs::read(fixture.a.join("local.txt")).unwrap(), b"local");
    fs::write(fixture.a.join("same.txt"), b"base").unwrap();
    fixture.run().unwrap();
    let observed = fs::metadata(fixture.a.join("same.txt"))
        .unwrap()
        .modified()
        .unwrap();
    for (root, bytes) in [(&fixture.a, b"port"), (&fixture.b, b"locl")] {
        fs::write(root.join("same.txt"), bytes).unwrap();
        File::open(root.join("same.txt"))
            .unwrap()
            .set_times(FileTimes::new().set_modified(observed))
            .unwrap();
    }
    fixture.run().unwrap();
    let conflicts = fixture
        .db
        .list::<DriveConflict>("conflict", 0, 100)
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    assert!(conflicts[0].complete);
    for root in [&fixture.a, &fixture.b] {
        assert_eq!(
            fs::read(root.join(&conflicts[0].portable)).unwrap(),
            b"port"
        );
        assert_eq!(fs::read(root.join(&conflicts[0].local)).unwrap(), b"locl");
    }
    fixture.run().unwrap();
    assert_eq!(
        fixture
            .db
            .list::<DriveConflict>("conflict", 0, 100)
            .unwrap()
            .len(),
        1
    );
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_deletion_policy_preserves_edits_and_requires_opt_in() {
    let (_native, mut fixture, _) = fixture();
    fs::write(fixture.a.join("keep.txt"), b"baseline").unwrap();
    fixture.run().unwrap();
    fs::remove_file(fixture.a.join("keep.txt")).unwrap();
    fixture.run().unwrap();
    assert_eq!(fs::read(fixture.a.join("keep.txt")).unwrap(), b"baseline");
    fixture.pair.deletions = true;
    fs::remove_file(fixture.a.join("keep.txt")).unwrap();
    fs::write(fixture.b.join("keep.txt"), b"edited while absent").unwrap();
    fixture.run().unwrap();
    assert_eq!(
        fs::read(fixture.a.join("keep.txt")).unwrap(),
        b"edited while absent"
    );
    fs::write(fixture.a.join("remove.txt"), b"unchanged").unwrap();
    fixture.run().unwrap();
    fs::remove_file(fixture.a.join("remove.txt")).unwrap();
    fixture.run().unwrap();
    assert!(!fixture.a.join("remove.txt").exists());
    assert!(!fixture.b.join("remove.txt").exists());
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_file_backups_keep_versions_and_observed_timestamps() {
    use sha2::{Digest, Sha256};
    let (_native, fixture, drive) = fixture();
    let relative = Path::new("nested/Unicode Ω name.txt");
    fs::create_dir(fixture.a.join("nested")).unwrap();
    fs::write(fixture.a.join(relative), b"first version").unwrap();
    File::open(fixture.a.join(relative))
        .unwrap()
        .set_times(
            FileTimes::new().set_modified(UNIX_EPOCH + Duration::new(1_600_000_000, 125_000_000)),
        )
        .unwrap();
    let observed = fs::metadata(fixture.a.join(relative)).unwrap();
    let source = Root::open(&fixture.a).unwrap();
    let destination = Root::create(&fixture._temp.path().join("backups")).unwrap();
    let first = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    assert!(!first.incremental);
    assert_eq!(
        fs::read(first.path.join(relative)).unwrap(),
        b"first version"
    );
    assert_eq!(
        fs::metadata(first.path.join(relative))
            .unwrap()
            .modified()
            .unwrap(),
        observed.modified().unwrap()
    );
    assert_eq!(
        first.path.parent().unwrap().file_name().unwrap(),
        drive
            .part_uuid
            .clone()
            .unwrap_or_else(|| format!("fs-{}", drive.filesystem_uuid.as_ref().unwrap()))
            .as_str()
    );
    let manifest = first.manifest.as_ref().unwrap();
    assert_eq!(
        first.manifest_sha256.as_deref().unwrap(),
        format!("{:x}", Sha256::digest(fs::read(manifest).unwrap()))
    );
    let database = Store::existing(manifest, true).unwrap();
    let entry = scan::entry(&database, &format!("backup:{}", first.id), relative)
        .unwrap()
        .unwrap();
    assert_eq!(
        (entry.modified, entry.nanos),
        (observed.mtime(), observed.mtime_nsec())
    );
    assert!(entry.hash.is_some());
    fs::write(fixture.a.join(relative), b"second version").unwrap();
    let second = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    assert!(!second.incremental);
    assert_eq!(
        fs::read(first.path.join(relative)).unwrap(),
        b"first version"
    );
    assert_eq!(
        fs::read(second.path.join(relative)).unwrap(),
        b"second version"
    );
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_live_backup_retries_preserved_timestamp_edits_and_bounds_unstable_sources() {
    let (_native, fixture, drive) = fixture();
    let path = fixture.a.join("changing.txt");
    fs::write(&path, b"false-0").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let source = Root::open(&fixture.a).unwrap();
    let destination = Root::create(&fixture._temp.path().join("backups")).unwrap();
    let mut current = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    for continuous in [false, true] {
        let mut changes = 0;
        let result = backup::files(
            &fixture.db,
            &drive,
            &source,
            &destination,
            &fixture.cancel,
            |count, _| {
                if count == 0 && (continuous || changes == 0) {
                    changes += 1;
                    fs::write(&path, format!("{continuous}-{changes}")).unwrap();
                    File::open(&path)
                        .unwrap()
                        .set_times(FileTimes::new().set_modified(modified))
                        .unwrap();
                }
            },
        );
        if continuous {
            let error = result.unwrap_err();
            assert_eq!(changes, 3);
            assert!(error.to_string().contains("after 3 attempt(s)"), "{error}");
            assert_eq!(
                fixture
                    .db
                    .list::<crate::DriveBackup>("backup", 0, 10)
                    .unwrap()
                    .len(),
                2
            );
            let reference: crate::DriveBackup = serde_json::from_slice(
                &fs::read(current.path.parent().unwrap().join("current-files.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(reference.id, current.id);
        } else {
            let version = result.unwrap();
            assert_eq!(changes, 1);
            assert_eq!(
                fs::read(current.path.join("changing.txt")).unwrap(),
                b"false-0"
            );
            assert_eq!(
                fs::read(version.path.join("changing.txt")).unwrap(),
                b"false-1"
            );
            let manifest = Store::existing(version.manifest.as_ref().unwrap(), true).unwrap();
            assert_eq!(
                manifest
                    .get::<u64>("collection", "retried_entries")
                    .unwrap(),
                Some(1)
            );
            let actual = source
                .digest(Path::new("changing.txt"), u64::MAX, &fixture.cancel)
                .unwrap();
            assert_eq!(
                scan::entry(
                    &manifest,
                    &format!("backup:{}", version.id),
                    Path::new("changing.txt")
                )
                .unwrap()
                .unwrap()
                .hash,
                Some(actual)
            );
            current = version;
        }
    }
    assert_eq!(
        fs::read(current.path.join("changing.txt")).unwrap(),
        b"false-1"
    );
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_catalog_exclusion_keeps_backup_and_sync_inclusion() {
    let (_native, fixture, drive) = fixture();
    fs::write(fixture.a.join("metadata.bin"), b"\x00\xffpayload").unwrap();
    fs::create_dir(fixture.a.join("excluded")).unwrap();
    fs::write(
        fixture.a.join("excluded/child.txt"),
        b"included in a full backup",
    )
    .unwrap();
    fixture
        .db
        .put(
            "rule",
            "native-excluded",
            &DriveRule {
                id: "native-excluded".into(),
                drive: drive.id.clone(),
                relative: "excluded".into(),
                catalog: false,
                index: false,
            },
        )
        .unwrap();
    let source = Root::open(&fixture.a).unwrap();
    let state = Root::create(&fixture._temp.path().join("catalog-state")).unwrap();
    let catalog = catalog::capture(
        &fixture.db,
        &state,
        &drive,
        &source,
        &DriveConfig::default(),
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    assert!(catalog.complete);
    let database = Store::existing(
        &state
            .path
            .join("catalogs")
            .join(format!("{}.sqlite", catalog.id)),
        true,
    )
    .unwrap();
    let entry = scan::entry(&database, "catalog", Path::new("metadata.bin"))
        .unwrap()
        .unwrap();
    assert_eq!(entry.size, 9);
    assert!(entry.hash.is_none());
    assert_eq!(
        scan::entry(&database, "catalog", Path::new("excluded"))
            .unwrap()
            .unwrap()
            .kind,
        "excluded"
    );
    assert!(
        scan::entry(&database, "catalog", Path::new("excluded/child.txt"))
            .unwrap()
            .is_none()
    );
    let destination = Root::create(&fixture._temp.path().join("backups")).unwrap();
    let version = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(version.path.join("excluded/child.txt")).unwrap(),
        b"included in a full backup"
    );
    fixture.run().unwrap();
    assert_eq!(
        fs::read(fixture.b.join("excluded/child.txt")).unwrap(),
        b"included in a full backup"
    );
}

fn portable_permissions(fixture: &Fixture) -> fs::Permissions {
    let path = fixture.a.join("permissions-probe.txt");
    fs::write(&path, []).unwrap();
    let permissions = fs::metadata(&path).unwrap().permissions();
    fs::remove_file(path).unwrap();
    permissions
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_case_collisions_preserve_both_local_originals() {
    let (_native, fixture, _) = fixture();
    let probe = fixture.a.join("CaseProbe");
    fs::write(&probe, b"probe").unwrap();
    let folds_case = fixture.a.join("caseprobe").exists();
    fs::remove_file(probe).unwrap();
    for (name, bytes) in [("Case.txt", b"upper"), ("case.txt", b"lower")] {
        fs::write(fixture.b.join(name), bytes).unwrap();
        fs::set_permissions(fixture.b.join(name), portable_permissions(&fixture)).unwrap();
    }
    let outcome = fixture.run();
    assert_eq!(fs::read(fixture.b.join("Case.txt")).unwrap(), b"upper");
    assert_eq!(fs::read(fixture.b.join("case.txt")).unwrap(), b"lower");
    if folds_case {
        assert!(
            outcome.is_err(),
            "A case-fold collision needs explicit resolution"
        );
        let retained = fs::read(fixture.a.join("Case.txt")).unwrap();
        assert!(retained == b"upper" || retained == b"lower");
    } else {
        outcome.unwrap();
        assert_eq!(fs::read(fixture.a.join("Case.txt")).unwrap(), b"upper");
        assert_eq!(fs::read(fixture.a.join("case.txt")).unwrap(), b"lower");
    }
    eprintln!("Native filesystem folds ASCII case: {folds_case}");
}

fn tricky_names() -> [String; 12] {
    [
        "unsupported:name?.txt".to_string(),
        "trailing-dot.".into(),
        "trailing-space ".into(),
        "CON".into(),
        "NUL.txt".into(),
        "question?.txt".into(),
        "wild*card.txt".into(),
        "back\\slash.txt".into(),
        "control_\u{1}.bin".into(),
        "unicode_é.txt".into(),
        "unicode_e\u{301}.txt".into(),
        "n".repeat(255),
    ]
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_unsupported_names_preserve_literal_names_or_report_failure() {
    for directory in [false, true] {
        for name in tricky_names() {
            let (_native, fixture, _) = fixture();
            let relative = if directory {
                fs::create_dir(fixture.b.join(&name)).unwrap();
                PathBuf::from(&name).join("child.txt")
            } else {
                PathBuf::from(&name)
            };
            fs::write(fixture.b.join(&relative), b"local original").unwrap();
            fs::set_permissions(fixture.b.join(&relative), portable_permissions(&fixture)).unwrap();
            let outcome = fixture.run();
            assert_eq!(
                fs::read(fixture.b.join(&relative)).unwrap(),
                b"local original"
            );
            let listed = fs::read_dir(&fixture.a)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            match outcome {
                Ok(_) => {
                    assert_eq!(
                        listed,
                        vec![std::ffi::OsString::from(&name)],
                        "A completed copy must retain the literal requested name: {name:?} (directory: {directory})"
                    );
                    assert_eq!(
                        fs::read(fixture.a.join(&relative)).unwrap(),
                        b"local original"
                    );
                }
                Err(error) => {
                    assert!(
                        listed.is_empty(),
                        "A refused name must not leave a renamed entry: {name:?} (directory: {directory}): {error}"
                    );
                    eprintln!("Refused native name {name:?} (directory: {directory}): {error}");
                }
            }
        }
    }
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_ntfs_stream_addresses_cannot_modify_a_base_file() {
    let (_native, fixture, drive) = fixture();
    if drive.filesystem != "ntfs3" {
        return;
    }
    fs::write(fixture.a.join("base.txt"), b"base original").unwrap();
    let root = Root::open(&fixture.a).unwrap();
    let error = root.metadata(Path::new("base.txt:stream")).unwrap_err();
    assert!(error.to_string().contains("NTFS alternate-stream"));
    assert!(root.input(Path::new("base.txt:stream")).is_err());
    assert!(
        root.ensure_directory(Path::new("folder:stream/child"))
            .is_err()
    );
    assert!(!fixture.a.join("folder").exists());
    assert_eq!(
        fs::read(fixture.a.join("base.txt")).unwrap(),
        b"base original"
    );
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_fat32_size_limit_is_refused_before_reading_sparse_payload() {
    let (_native, fixture, mut drive) = fixture();
    if drive.filesystem != "vfat" {
        return;
    }
    File::create(fixture.b.join("large.bin"))
        .unwrap()
        .set_len(u32::MAX as u64 + 1)
        .unwrap();
    let error = Root::open(&fixture.a)
        .unwrap()
        .copy(
            &Root::open(&fixture.b).unwrap(),
            crate::root::CopySpec {
                source: Path::new("large.bin"),
                destination: Path::new("large.bin"),
                source_hash: None,
                destination_hash: None,
                delta: false,
            },
            &fixture.cancel,
        )
        .unwrap_err();
    assert!(error.to_string().contains("FAT32"), "{error}");
    assert!(!fixture.a.join("large.bin").exists());
    drive.capacity = u32::MAX as u64 + 1;
    let error = backup::image_from(
        &fixture.db,
        &drive,
        &Root::open(&fixture.a).unwrap(),
        &File::open(fixture.b.join("large.bin")).unwrap(),
        &fixture.cancel,
        |_, _| panic!("oversized FAT32 images must fail before copying"),
        || panic!("oversized FAT32 images must fail before reading input"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("FAT32"), "{error}");
    assert!(fs::read_dir(&fixture.a).unwrap().next().is_none());
}
