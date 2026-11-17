//! Guest-only application checks on actual FAT32, exFAT and NTFS mounts.
use super::*;
use crate::{backup, catalog, scan};
use little_clip_core::{kernel_interface, process::CommandRequest};
use std::{
    fs::{File, FileTimes},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::{Duration, UNIX_EPOCH},
};

pub(crate) fn guest_partition() -> Drive {
    crate::require_disposable_guest();
    let source = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").unwrap());
    let device = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_DEVICE").unwrap());
    assert_eq!(source, Path::new("/work/media"));
    let filesystem = std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
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

pub(crate) fn guest_drive() -> Drive {
    let drive = guest_partition();
    let source = drive.mount.as_ref().unwrap();
    let filesystem = &drive.filesystem;
    let device = &drive.device;
    let root = Root::open(source).unwrap();
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
    assert_eq!(&mount.filesystem, filesystem);
    assert_eq!(&mount.source, device);
    drive
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

impl Fixture {
    fn file_backup(
        &self,
        drive: &Drive,
        progress: impl FnMut(u64, u64),
    ) -> crate::Result<crate::DriveBackup> {
        let source = Root::open(&self.a)?;
        let destination = Root::create(&self._temp.path().join("backups"))?;
        backup::files(
            &self.db,
            drive,
            &source,
            &destination,
            &self.cancel,
            progress,
        )
    }
}

fn verified_file_manifest(version: &crate::DriveBackup, retried: u64) -> Store {
    let held = kernel_interface::open_content(version.manifest.as_ref().unwrap(), false).unwrap();
    let checksum = little_clip_core::file_source::FileSource::capture_file(&held)
        .unwrap()
        .sha256(&held, &AtomicBool::new(false), Duration::MAX)
        .unwrap();
    assert_eq!(version.manifest_sha256.as_ref(), Some(&checksum));
    let manifest = Store::existing(&crate::root::anchor(&held), true).unwrap();
    let header: crate::DriveBackup = manifest.get("backup", &version.id).unwrap().unwrap();
    assert_eq!(header.fidelity, version.fidelity);
    let records: u64 = manifest
        .0
        .query_row("SELECT count(*) FROM backup_metadata", [], |row| row.get(0))
        .unwrap();
    assert_eq!(records, version.fidelity.as_ref().unwrap().entries);
    assert_eq!(
        manifest
            .get::<u64>("collection", "retried_entries")
            .unwrap(),
        Some(retried)
    );
    manifest
}

pub(crate) struct GuestMount {
    path: PathBuf,
    identity: kernel_interface::MountIdentity,
    cleanup: Option<MountCleanup>,
}
enum MountCleanup {
    Unmount,
    Writable,
}

fn assert_backup_refused(fixture: &Fixture, drive: &Drive, reason: impl std::fmt::Display) {
    let error = fixture.file_backup(drive, |_, _| {}).unwrap_err();
    assert!(error.to_string().contains(&reason.to_string()), "{error}");
    assert!(
        fixture
            .db
            .list::<crate::DriveBackup>("backup", 0, 10)
            .unwrap()
            .is_empty()
    );
}
impl GuestMount {
    fn read_only_filesystem() -> Self {
        crate::require_disposable_guest();
        let mount = Self::capture(Path::new("/work/media"), MountCleanup::Writable);
        mount.remount("remount,ro");
        mount
    }
    pub(crate) fn remount(&self, options: &str) {
        crate::require_disposable_guest();
        assert_eq!(
            kernel_interface::MountIdentity::directory(&self.path).unwrap(),
            self.identity
        );
        let result = CommandRequest::new("/usr/bin/mount")
            .arg("--internal-only")
            .args(["-o", options])
            .arg(&self.path)
            .run()
            .unwrap();
        assert!(result.status.success(), "{}", result.stderr_lossy());
    }
    fn writable(&mut self) {
        self.remount("remount,rw");
        self.cleanup = None;
    }
    pub(crate) fn attach(source: &Path, destination: &Path) -> Self {
        Self::start(
            CommandRequest::new("mount").arg("--bind").arg(source),
            destination,
        )
    }
    fn attach_link(source: &File, destination: &Path) -> Self {
        let (request, descriptor) = CommandRequest::new("/usr/bin/mount")
            .args(["--bind", "--no-canonicalize"])
            .inherit_handle(source)
            .unwrap();
        Self::start(
            request.arg(format!("/proc/self/fd/{descriptor}")),
            destination,
        )
    }
    pub(crate) fn start(request: CommandRequest, destination: &Path) -> Self {
        crate::require_disposable_guest();
        let result = request.arg(destination).run().unwrap();
        assert!(result.status.success(), "{}", result.stderr_lossy());
        Self::capture(destination, MountCleanup::Unmount)
    }
    fn capture(destination: &Path, cleanup: MountCleanup) -> Self {
        let file = kernel_interface::pin(destination, false).unwrap();
        Self {
            path: destination.into(),
            identity: kernel_interface::MountIdentity::pinned(&file).unwrap(),
            cleanup: Some(cleanup),
        }
    }
}

impl Drop for GuestMount {
    fn drop(&mut self) {
        let Some(cleanup) = self.cleanup.take() else {
            return;
        };
        let current = kernel_interface::pin(&self.path, false)
            .and_then(|file| kernel_interface::MountIdentity::pinned(&file));
        if current.ok() == Some(self.identity) {
            if matches!(cleanup, MountCleanup::Writable) {
                self.remount("remount,rw");
                return;
            }
            let path = std::ffi::CString::new(self.path.as_os_str().as_encoded_bytes()).unwrap();
            // The owned mount may itself be a symlink inode. Never follow its
            // referent when releasing this identity-checked guest fixture.
            let result = unsafe { libc::umount2(path.as_ptr(), libc::UMOUNT_NOFOLLOW) };
            assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        }
    }
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_read_only_collection_requires_a_read_only_filesystem_and_refuses_policy_loss() {
    let (_native, fixture, mut drive) = fixture();
    fs::write(fixture.a.join("payload"), b"stable source bytes").unwrap();
    let view = fixture._temp.path().join("read-only-view");
    fs::create_dir(&view).unwrap();
    let bind = GuestMount::attach(&fixture.a, &view);
    bind.remount("remount,bind,ro");
    let view_root = Root::open(&view).unwrap();
    let directory = view_root.directory(Path::new(""), false).unwrap();
    assert!(
        rustix::fs::fstatvfs(&directory)
            .unwrap()
            .f_flag
            .contains(rustix::fs::StatVfsMountFlags::RDONLY)
    );
    let error = Root::open_with_read_only(&view, true)
        .err()
        .expect("writable backing filesystem refused");
    assert!(
        error.to_string().contains("backing filesystem is writable"),
        "{error}"
    );
    // Ordinary collection is available through this view. The explicit stronger
    // policy alone requires a read-only backing filesystem.
    let ordinary_db = Store::open(&fixture._temp.path().join("ordinary.sqlite")).unwrap();
    let ordinary_destination =
        Root::create(&fixture._temp.path().join("ordinary-backups")).unwrap();
    let ordinary = backup::files(
        &ordinary_db,
        &drive,
        &view_root,
        &ordinary_destination,
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(ordinary.path.join("payload")).unwrap(),
        b"stable source bytes"
    );
    let ordinary_manifest = verified_file_manifest(&ordinary, 0);
    assert!(
        ordinary_manifest
            .get::<String>("collection", "consistency")
            .unwrap()
            .unwrap()
            .starts_with("Per-file revision checks")
    );
    drop(directory);
    drop(view_root);
    drop(bind);

    let mut mode = GuestMount::read_only_filesystem();
    drive.read_only = true;
    let source = Root::open_with_read_only(&fixture.a, true).unwrap();
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
    assert_eq!(
        fs::read(first.path.join("payload")).unwrap(),
        b"stable source bytes"
    );
    let manifest = verified_file_manifest(&first, 0);
    assert_eq!(
        manifest
            .get::<String>("collection", "consistency")
            .unwrap()
            .as_deref(),
        Some(
            "Read-only filesystem collection; source mount and superblock checked through publication"
        )
    );
    assert_eq!(
        manifest
            .get::<bool>("collection", "source_mount_read_only_at_start")
            .unwrap(),
        Some(true)
    );
    let mut changed = false;
    let error = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |count, _| {
            if count == 0 && !changed {
                mode.writable();
                changed = true;
            }
        },
    )
    .unwrap_err();
    assert!(changed);
    assert!(
        error.to_string().contains("Read-only collection requires"),
        "{error}"
    );
    assert!(source.validate().is_err());
    let current: crate::DriveBackup = serde_json::from_slice(
        &fs::read(first.path.parent().unwrap().join("current-files.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(current.id, first.id);
    assert_eq!(
        fixture
            .db
            .list::<crate::DriveBackup>("backup", 0, 10)
            .unwrap()
            .len(),
        1
    );
    verified_file_manifest(&first, 0);
    assert_eq!(
        fs::read(fixture.a.join("payload")).unwrap(),
        b"stable source bytes"
    );
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_leaf_file_and_symlink_binds_cannot_supply_metadata_or_payloads() {
    for (same_filesystem, symlink) in [(false, false), (true, false), (false, true)] {
        let (_native, fixture, drive) = fixture();
        let outside = tempfile::tempdir_in(if same_filesystem {
            Path::new("/work/media")
        } else {
            fixture._temp.path()
        })
        .unwrap();
        let foreign = outside.path().join("foreign");
        if symlink {
            std::os::unix::fs::symlink("outside referent", &foreign).unwrap();
        } else {
            fs::write(&foreign, b"outside the admitted root").unwrap();
        }
        let target = fixture.a.join("bound-file");
        fs::write(&target, b"original portable file").unwrap();
        let source = Root::open(&fixture.a).unwrap();
        let bind = if symlink {
            GuestMount::attach_link(&kernel_interface::pin(&foreign, false).unwrap(), &target)
        } else {
            GuestMount::attach(&foreign, &target)
        };
        assert_eq!(
            fs::symlink_metadata(&foreign).unwrap().dev()
                == fs::metadata(&fixture.a).unwrap().dev(),
            same_filesystem
        );
        assert_eq!(fs::symlink_metadata(&target).unwrap().is_symlink(), symlink);
        assert_eq!(
            crate::raw_errno(source.metadata(Path::new("bound-file")).unwrap_err()),
            Some(libc::EXDEV)
        );
        let error = source
            .digest(Path::new("bound-file"), u64::MAX, &fixture.cancel)
            .unwrap_err();
        assert_eq!(crate::raw_errno(error), Some(libc::EXDEV));
        assert_backup_refused(
            &fixture,
            &drive,
            std::io::Error::from_raw_os_error(libc::EXDEV),
        );
        drop(bind);
        assert_eq!(fs::read(&target).unwrap(), b"original portable file");
        if symlink {
            assert_eq!(
                fs::read_link(&foreign).unwrap(),
                Path::new("outside referent")
            );
        } else {
            assert_eq!(fs::read(&foreign).unwrap(), b"outside the admitted root");
        }
    }
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
    let first = fixture.file_backup(&drive, |_, _| {}).unwrap();
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
    let database = verified_file_manifest(&first, 0);
    let entry = scan::entry(&database, &format!("backup:{}", first.id), relative)
        .unwrap()
        .unwrap();
    assert_eq!(
        (entry.modified, entry.nanos),
        (observed.mtime(), observed.mtime_nsec())
    );
    assert!(entry.hash.is_some());
    fs::write(fixture.a.join(relative), b"second version").unwrap();
    let second = fixture.file_backup(&drive, |_, _| {}).unwrap();
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
            let manifest = verified_file_manifest(&version, 1);
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
fn native_attribute_only_source_changes_are_recollected_when_exposed() {
    let (_native, fixture, drive) = fixture();
    let path = fixture.a.join("stable.txt");
    fs::write(&path, b"stable").unwrap();
    match rustix::fs::setxattr(
        &path,
        "user.live",
        b"before",
        rustix::fs::XattrFlags::empty(),
    ) {
        Ok(()) => (),
        Err(error) => {
            assert!(
                matches!(
                    error,
                    rustix::io::Errno::OPNOTSUPP | rustix::io::Errno::PERM
                ),
                "{error}"
            );
            eprintln!(
                "{} does not expose writable user attributes: {error}",
                drive.filesystem
            );
            return;
        }
    }
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let source = Root::open(&fixture.a).unwrap();
    let destination = Root::create(&fixture._temp.path().join("backups")).unwrap();
    let mut attempts = 0;
    let version = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |count, _| {
            if count == 0 {
                attempts += 1;
                if attempts == 1 {
                    rustix::fs::setxattr(
                        &path,
                        "user.live",
                        b"after",
                        rustix::fs::XattrFlags::empty(),
                    )
                    .unwrap();
                    File::open(&path)
                        .unwrap()
                        .set_times(FileTimes::new().set_modified(modified))
                        .unwrap();
                }
            }
        },
    )
    .unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(version.bytes_written, 6);
    let mut value = [0; 64];
    let count =
        rustix::fs::getxattr(version.path.join("stable.txt"), "user.live", &mut value[..]).unwrap();
    assert_eq!(&value[..count], b"after");
    assert_eq!(
        fs::read(version.path.join("stable.txt")).unwrap(),
        b"stable"
    );
    verified_file_manifest(&version, 1);
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_backup_destinations_disclose_coerced_modes_owner_times_and_missing_attributes() {
    let (_native, fixture, mut drive) = fixture();
    let destination = Root::open(&fixture.a).unwrap();
    let source = Root::open(&fixture.b).unwrap();
    let path = fixture.b.join("file");
    fs::write(&path, b"complete file contents").unwrap();
    fs::hard_link(&path, fixture.b.join("alias")).unwrap();
    std::os::unix::fs::chown(&path, Some(1001), Some(1002)).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    File::open(&path)
        .unwrap()
        .set_times(
            FileTimes::new().set_modified(UNIX_EPOCH + Duration::new(1_600_000_000, 123_456_789)),
        )
        .unwrap();
    rustix::fs::setxattr(
        &path,
        "user.source",
        b"source attribute",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    let format = drive.filesystem.clone();
    drive.mount = Some(fixture.b.clone());
    drive.filesystem = "tmpfs".into();
    drive.evidence = "owned guest tmpfs source for native destination checks".into();
    let version = backup::files(
        &fixture.db,
        &drive,
        &source,
        &destination,
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    let target = version.path.join("file");
    let actual = fs::metadata(&target).unwrap();
    let expected = fs::metadata(&path).unwrap();
    let manifest = verified_file_manifest(&version, 0);
    let json: String = manifest
        .0
        .query_row(
            "SELECT json FROM backup_metadata WHERE path=?1",
            [b"file".as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    let facts: serde_json::Value = serde_json::from_str(&json).unwrap();
    let omitted = |field: &str| {
        facts["omissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["field"] == field)
    };
    if (actual.uid(), actual.gid()) != (1001, 1002) {
        assert!(omitted("ownership"));
    }
    if actual.mode() & 0o7777 != 0o640 {
        assert!(omitted("mode"));
    }
    if (actual.mtime(), actual.mtime_nsec()) != (expected.mtime(), expected.mtime_nsec()) {
        assert!(omitted("modification_time"));
    }
    let mut bytes = [0; 64];
    match rustix::fs::getxattr(&target, "user.source", &mut bytes[..]) {
        Ok(count) if &bytes[..count] == b"source attribute" => (),
        _ => assert!(omitted("extended_attribute")),
    }
    if format == "vfat" || format == "exfat" {
        assert_ne!(actual.mode() & 0o7777, 0o640);
        assert!(omitted("mode"));
        assert!(omitted("extended_attribute"));
        assert!(version.fidelity.as_ref().unwrap().omitted_fields > 0);
        assert!(omitted("hard_links"));
        assert_eq!(
            version.bytes_written,
            2 * b"complete file contents".len() as u64
        );
        assert_ne!(
            actual.ino(),
            fs::metadata(version.path.join("alias")).unwrap().ino()
        );
    } else {
        assert!(!omitted("hard_links"));
        assert_eq!(
            version.bytes_written,
            b"complete file contents".len() as u64
        );
        assert_eq!(
            actual.ino(),
            fs::metadata(version.path.join("alias")).unwrap().ino()
        );
        assert_eq!(actual.nlink(), 2);
    }
    assert_eq!(fs::read(target).unwrap(), b"complete file contents");
    assert_eq!(
        fs::read(version.path.join("alias")).unwrap(),
        b"complete file contents"
    );
    let alias_json: String = manifest
        .0
        .query_row(
            "SELECT json FROM backup_metadata WHERE path=?1",
            [b"alias".as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    let alias_facts: serde_json::Value = serde_json::from_str(&alias_json).unwrap();
    if format == "vfat" || format == "exfat" {
        assert!(
            alias_facts["omissions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["field"] == "hard_links")
        );
        assert!(
            facts["omissions"]
                .as_array()
                .unwrap()
                .iter()
                .chain(alias_facts["omissions"].as_array().unwrap())
                .any(|item| item["field"] == "hard_links" && item["errno"].is_number())
        );
    }
    assert_eq!(
        facts["attributes"][0]["value"],
        serde_json::json!(b"source attribute".as_slice())
    );
    eprintln!(
        "{format} destination: {:?}; per-path report {}",
        version.fidelity, facts["omissions"]
    );
}

#[test]
#[ignore = "requires the isolated removable-filesystem guest"]
fn native_hard_link_sources_retain_aliases_and_refuse_divergent_captured_attributes() {
    let (_native, fixture, drive) = fixture();
    let file = fixture.a.join("file");
    fs::write(&file, b"shared bytes").unwrap();
    if let Err(error) = fs::hard_link(&file, fixture.a.join("alias")) {
        assert!(
            matches!(error.raw_os_error(), Some(libc::EPERM | libc::EOPNOTSUPP)),
            "{error}"
        );
        assert!(matches!(drive.filesystem.as_str(), "vfat" | "exfat"));
        eprintln!(
            "{} does not support source hard links: {error}",
            drive.filesystem
        );
        return;
    }
    rustix::fs::setxattr(
        &file,
        "user.live",
        b"before",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    let first = fixture.file_backup(&drive, |_, _| {}).unwrap();
    assert_eq!(first.bytes_written, b"shared bytes".len() as u64);
    let actual = fs::metadata(first.path.join("file")).unwrap();
    assert_eq!(actual.nlink(), 2);
    assert_eq!(
        actual.ino(),
        fs::metadata(first.path.join("alias")).unwrap().ino()
    );
    verified_file_manifest(&first, 0);
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let mut edited = false;
    let error = fixture
        .file_backup(&drive, |count, _| {
            if count == 1 && !edited {
                rustix::fs::setxattr(
                    &file,
                    "user.live",
                    b"after",
                    rustix::fs::XattrFlags::empty(),
                )
                .unwrap();
                File::open(&file)
                    .unwrap()
                    .set_times(FileTimes::new().set_modified(modified))
                    .unwrap();
                edited = true;
            }
        })
        .unwrap_err();
    assert!(edited);
    assert!(
        error
            .to_string()
            .contains("Hard-linked source revisions differ"),
        "{error}"
    );
    let versions = fixture
        .db
        .list::<crate::DriveBackup>("backup", 0, 10)
        .unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].id, first.id);
    let reference: crate::DriveBackup = serde_json::from_slice(
        &fs::read(first.path.parent().unwrap().join("current-files.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(reference.id, first.id);
    for name in ["file", "alias"] {
        assert_eq!(fs::read(first.path.join(name)).unwrap(), b"shared bytes");
        let mut value = [0; 64];
        let count =
            rustix::fs::getxattr(first.path.join(name), "user.live", &mut value[..]).unwrap();
        assert_eq!(&value[..count], b"before");
    }
    verified_file_manifest(&first, 0);
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
    let facts = catalog.details.as_ref().unwrap();
    assert_eq!(facts.filesystem.as_deref(), Some(drive.filesystem.as_str()));
    assert_eq!(
        facts.mtime_resolution_ns,
        Some(match drive.filesystem.as_str() {
            "vfat" => 2_000_000_000,
            "exfat" => 10_000_000,
            "ntfs3" => 100,
            other => panic!("unexpected native source {other}"),
        })
    );
    assert!(facts.directory.listing_complete);
    assert!(!facts.directory.subtree_complete);
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
    let observed = fs::metadata(fixture.a.join("metadata.bin")).unwrap();
    assert_eq!(
        (entry.modified, entry.nanos),
        (observed.mtime(), observed.mtime_nsec())
    );
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
    // A saved digest exists, but these source drivers cannot certify unchanged
    // revisions for metadata-only hash reuse. Do not import it by mtime/size.
    let manifest = Store::existing(version.manifest.as_ref().unwrap(), true).unwrap();
    assert!(
        scan::entry(
            &manifest,
            &format!("backup:{}", version.id),
            Path::new("metadata.bin")
        )
        .unwrap()
        .unwrap()
        .hash
        .is_some()
    );
    let later = catalog::capture(
        &fixture.db,
        &state,
        &drive,
        &source,
        &DriveConfig::default(),
        &fixture.cancel,
        |_, _| {},
    )
    .unwrap();
    let later_db = Store::existing(
        &state
            .path
            .join("catalogs")
            .join(format!("{}.sqlite", later.id)),
        true,
    )
    .unwrap();
    assert!(
        scan::entry(&later_db, "catalog", Path::new("metadata.bin"))
            .unwrap()
            .unwrap()
            .hash
            .is_none()
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
                basic_metadata: true,
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
