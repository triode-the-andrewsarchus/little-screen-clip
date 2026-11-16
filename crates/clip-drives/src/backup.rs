//! Versioned guarded filesystem backups and partition images with reflink deltas.
use crate::{
    BackupKind, Drive, PartitionPhase, Result, cancelled, now,
    root::{Root, anchor},
    scan::{self, Limits},
    store::Store,
    sync,
};
use little_clip_core::{kernel_interface, process::CommandRequest};
use std::{
    collections::HashMap,
    fs::{self, File},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::fs::{FileExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

mod publication;
pub(crate) use publication::recover as recover_publications;

pub(crate) use crate::DriveBackup as Backup;
fn btrfs(root: &Root) -> Result<bool> {
    let pin = root.directory(Path::new(""), false)?;
    Ok(rustix::fs::fstatfs(&pin)?.f_type as u32 == 0x9123683e)
}
fn command(args: &[&str], parent: &File, names: &[&std::ffi::OsStr]) -> Result<()> {
    let (mut request, fd) = CommandRequest::new("btrfs")
        .args(args.iter().copied())
        .timeout(Duration::from_secs(60))
        .max_output_bytes(64 * 1024)
        .own_process_group()
        .inherit_handle(parent)?;
    for name in names {
        request = request.arg(PathBuf::from(format!("/proc/self/fd/{fd}")).join(name));
    }
    let output = request.run().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Btrfs operation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}
fn last(db: &Store, drive: &Drive, kind: BackupKind) -> Result<Option<Backup>> {
    if drive.part_uuid.is_none() && drive.filesystem_uuid.is_none() {
        return Ok(None);
    }
    if let Some(current) = publication::current(db, &drive.id, kind)? {
        return Ok(Some(current));
    }
    let mut offset = 0;
    loop {
        let versions: Vec<Backup> = db.list("backup", offset, 128)?;
        if versions.is_empty() {
            return Ok(None);
        }
        for version in versions {
            if version.drive == drive.id && version.kind == kind {
                return Ok(Some(version));
            }
        }
        offset += 128;
    }
}
fn backup_parent(destination: &Root, drive: &Drive) -> Result<Root> {
    let key = drive
        .part_uuid
        .clone()
        .or_else(|| {
            drive
                .filesystem_uuid
                .as_ref()
                .map(|uuid| format!("fs-{uuid}"))
        })
        .unwrap_or_else(|| format!("unidentified-{}", uuid::Uuid::new_v4()));
    let uuid = &key;
    if uuid.is_empty() || !uuid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("Invalid partition UUID".into());
    }
    let pin = destination.directory(Path::new(uuid), true)?;
    Root::open(&anchor(&pin))
}
pub(crate) fn files(
    db: &Store,
    drive: &Drive,
    source: &Root,
    destination: &Root,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Backup> {
    source.validate()?;
    let parent = backup_parent(destination, drive)?;
    if source.overlap(&parent) {
        return Err("Backup destination overlaps the source".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let temporary = format!(".incomplete-{id}");
    let completed = format!("{}-{id}", now());
    let pin = parent.directory(Path::new(""), false)?;
    let prior = last(db, drive, BackupKind::Files)?;
    let incremental = btrfs(&parent)?;
    if incremental {
        if let Some(prior) = prior.as_ref() {
            let name = prior.path.file_name().ok_or("Invalid prior backup name")?;
            if prior.path.parent() != Some(parent.path.as_path()) {
                return Err("Prior backup moved; choose a fresh destination".into());
            }
            command(
                &["subvolume", "snapshot"],
                &pin,
                &[name, temporary.as_ref()],
            )?;
        } else {
            command(&["subvolume", "create"], &pin, &[temporary.as_ref()])?;
        }
    } else {
        parent.ensure_directory(Path::new(&temporary))?;
    }
    let staged = kernel_interface::pin(&anchor(&pin).join(&temporary), false)?;
    if !staged.metadata()?.is_dir() {
        return Err("Backup staging directory was replaced".into());
    }
    let target = Root::open(&anchor(&staged))?;
    // The same disk-backed scan becomes the durable completed manifest.
    let manifest = publication::Manifest::new(&parent)?;
    let root_metadata = scan::observe(source, Path::new(""), false, u64::MAX, cancel)?
        .ok_or("Missing source root metadata")?;
    manifest.db.put("source", "root", &root_metadata)?;
    manifest.db.put(
        "collection",
        "consistency",
        &"Per-file revision checks; live tree, not an atomic source snapshot",
    )?;
    let scan_id = format!("backup:{id}");
    let coverage = scan::scan(
        &manifest.db,
        source,
        &scan_id,
        &Limits::default(),
        cancel,
        &mut progress,
    )?;
    if !coverage.complete {
        return Err(format!("Incomplete source: {}", coverage.message).into());
    }
    let mut after = Vec::new();
    let mut count = 0;
    let mut written = 0;
    loop {
        let batch = scan::entries(&manifest.db, &scan_id, &after, 128)?;
        if batch.is_empty() {
            break;
        }
        for entry in batch {
            cancelled(cancel)?;
            prepare_entry(&manifest.db, &target, &entry, cancel)?;
            let existing = target.metadata(&entry.path)?;
            if entry.kind == "directory" {
                target.ensure_directory(&entry.path)?;
            } else {
                let old = if existing.is_some() {
                    scan::observe(&target, &entry.path, true, u64::MAX, cancel)?
                } else {
                    None
                };
                if old.as_ref().and_then(|e| e.hash.as_ref()) != entry.hash.as_ref()
                    || entry.kind != "file"
                {
                    written += sync::install(
                        source,
                        &target,
                        &entry,
                        &entry.path,
                        old.as_ref(),
                        cancel,
                        incremental,
                    )?;
                } else {
                    let observed = scan::observe(source, &entry.path, false, u64::MAX, cancel)?
                        .ok_or("Unchanged source disappeared")?;
                    if !entry.same_revision(&observed) {
                        return Err("Source changed before backup publication".into());
                    }
                    let file = target.input(&entry.path)?;
                    file.set_permissions(fs::Permissions::from_mode(entry.mode & 0o777))?;
                    if let Some(time) = time(entry.modified, entry.nanos) {
                        file.set_times(fs::FileTimes::new().set_modified(time))?;
                    }
                    file.sync_all()?;
                }
            }
            count += 1;
            progress(count, written);
            after = entry.path.as_os_str().as_encoded_bytes().to_vec();
        }
    }
    // Remove only entries absent from the complete source manifest in this private version.
    if prior.is_some() && incremental {
        remove_obsolete(&manifest.db, &scan_id, &target, cancel)?;
    }
    source.validate()?;
    target.validate()?;
    finish_directories(&manifest.db, &scan_id, &target, cancel)?;
    apply_directory_metadata(&target, &root_metadata)?;
    if incremental {
        // Property arguments follow the path, unlike subvolume commands.
        let (request, fd) = CommandRequest::new("btrfs")
            .args(["property", "set", "-ts"])
            .timeout(Duration::from_secs(30))
            .own_process_group()
            .inherit_handle(&pin)?;
        let output = request
            .arg(PathBuf::from(format!("/proc/self/fd/{fd}")).join(&temporary))
            .args(["ro", "true"])
            .run()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("Could not seal the completed Btrfs backup read-only".into());
        }
    }
    let backup = Backup {
        manifest: None,
        manifest_sha256: None,
        sha256: None,
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        kind: BackupKind::Files,
        path: parent.path.join(&completed),
        created: now(),
        incremental,
        bytes_written: written,
        source_size: drive.capacity,
        mount: None,
        loop_object: None,
        revision: None,
        identity: Some(target.storage_identity()?),
    };
    publication::publish(
        db,
        &parent,
        manifest,
        backup,
        temporary.as_ref(),
        drive,
        cancel,
    )
}
pub(crate) fn finish_directories(
    db: &Store,
    id: &str,
    target: &Root,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut statement=db.0.prepare("SELECT json FROM entries WHERE scan=?1 AND json_extract(json,'$.kind')='directory' ORDER BY path DESC")?;
    let rows = statement.query_map([id], |r| r.get::<_, String>(0))?;
    for row in rows {
        cancelled(cancel)?;
        let entry: scan::Entry = serde_json::from_str(&row?)?;
        apply_directory_metadata(target, &entry)?;
    }
    Ok(())
}
pub(crate) fn apply_directory_metadata(target: &Root, entry: &scan::Entry) -> Result<()> {
    let held = target.directory(&entry.path, false)?;
    let file = File::open(anchor(&held))?;
    file.set_permissions(fs::Permissions::from_mode(entry.mode & 0o777))?;
    if let Some(time) = time(entry.modified, entry.nanos) {
        file.set_times(fs::FileTimes::new().set_modified(time))?;
    }
    file.sync_all()?;
    Ok(())
}
fn prepare_entry(
    db: &Store,
    target: &Root,
    entry: &scan::Entry,
    cancel: &AtomicBool,
) -> Result<()> {
    let Some(metadata) = target.metadata(&entry.path)? else {
        return Ok(());
    };
    let compatible = match entry.kind.as_str() {
        "file" => metadata.is_file(),
        "directory" => metadata.is_dir(),
        "symlink" => metadata.file_type().is_symlink(),
        _ => false,
    };
    if !compatible {
        let (parent, path) = target.parent(&entry.path, false)?;
        if metadata.is_dir() {
            let old = Root::open(&path)?;
            let coverage = scan::scan(
                db,
                &old,
                "backup-type-check",
                &Limits {
                    hashes: false,
                    ..Default::default()
                },
                cancel,
                |_, _| {},
            )?;
            if !coverage.complete {
                return Err(
                    "Cannot validate old backup directory before replacing its type".into(),
                );
            }
            old.validate()?;
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
        File::open(anchor(&parent))?.sync_all()?;
    } else if metadata.is_dir() {
        use std::os::unix::fs::PermissionsExt;
        let held = target.directory(&entry.path, false)?;
        File::open(anchor(&held))?.set_permissions(fs::Permissions::from_mode(
            metadata.permissions().mode() | 0o700,
        ))?;
    }
    Ok(())
}
fn remove_obsolete(db: &Store, source: &str, target: &Root, cancel: &AtomicBool) -> Result<()> {
    let target_id = format!("{source}:old");
    let coverage = scan::scan(
        db,
        target,
        &target_id,
        &Limits::default(),
        cancel,
        |_, _| {},
    )?;
    if !coverage.complete {
        return Err("Could not validate the prior backup tree".into());
    }
    let mut statement=db.0.prepare("SELECT json FROM entries WHERE scan=?1 AND path NOT IN (SELECT path FROM entries WHERE scan=?2) ORDER BY path DESC")?;
    let rows = statement.query_map(rusqlite::params![target_id, source], |r| {
        r.get::<_, String>(0)
    })?;
    for row in rows {
        cancelled(cancel)?;
        let entry: scan::Entry = serde_json::from_str(&row?)?;
        let (parent, path) = target.parent(&entry.path, false)?;
        if entry.kind == "directory" {
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
        File::open(anchor(&parent))?.sync_all()?;
    }
    db.clear_scan(&target_id)?;
    Ok(())
}
pub(crate) fn time(seconds: i64, nanos: i64) -> Option<std::time::SystemTime> {
    if !(0..1_000_000_000).contains(&nanos) {
        return None;
    }
    if seconds >= 0 {
        std::time::UNIX_EPOCH.checked_add(Duration::new(seconds as u64, nanos as u32))
    } else {
        std::time::UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))?
            .checked_add(Duration::from_nanos(nanos as u64))
    }
}
fn system_connection() -> Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::system()
        .map_err(|e| e.to_string())?
        .method_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string().into())
}
fn udisks_path(drive: &Drive) -> Result<String> {
    let connection = system_connection()?;
    let manager = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.UDisks2",
        "/org/freedesktop/UDisks2",
        "org.freedesktop.UDisks2.Manager",
    )
    .map_err(|e| e.to_string())?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    let devices: Vec<zbus::zvariant::OwnedObjectPath> = manager
        .call("GetBlockDevices", &(options,))
        .map_err(|e| e.to_string())?;
    for path in devices {
        let proxy = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.UDisks2",
            path.as_str(),
            "org.freedesktop.UDisks2.Block",
        )
        .map_err(|e| e.to_string())?;
        let device: Vec<u8> = proxy.get_property("Device").map_err(|e| e.to_string())?;
        if device.strip_suffix(&[0]).unwrap_or(&device)
            == drive.device.as_os_str().as_encoded_bytes()
        {
            return Ok(path.to_string());
        }
    }
    Err("Partition no longer exists in UDisks".into())
}
pub(crate) type ImageOpener =
    Arc<dyn Fn(&Drive, &crate::device::Source) -> Result<File> + Send + Sync>;

pub(crate) fn open_image(drive: &Drive, source: &crate::device::Source) -> Result<File> {
    let connection = system_connection()?;
    let path = udisks_path(drive)?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.UDisks2",
        path.as_str(),
        "org.freedesktop.UDisks2.Block",
    )
    .map_err(|e| e.to_string())?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    source.validate()?;
    let descriptor: zbus::zvariant::OwnedFd = proxy
        .call("OpenForBackup", &(options,))
        .map_err(|e| format!("UDisks authorization or exclusive read failed: {e}"))?;
    let fd: OwnedFd = descriptor.into();
    Ok(File::from(fd))
}
pub(crate) fn image(
    db: &Store,
    drive: &Drive,
    source: &crate::device::Source,
    destination: &Root,
    cancel: &AtomicBool,
    opener: &ImageOpener,
    mut progress: impl FnMut(PartitionPhase, u64) -> Result<()>,
) -> Result<Backup> {
    if drive.mount.is_some() {
        return Err(
            "Unmount the partition before imaging it; flush pending RAM/sync changes first".into(),
        );
    }
    source.validate()?;
    progress(PartitionPhase::OpeningImage, 0)?;
    let input = opener(drive, source)?;
    let validate = || {
        source.validate_input(&input)?;
        let mut capacity = 0_u64;
        // SAFETY: BLKGETSIZE64 writes one u64 through a live authorized device descriptor.
        if unsafe {
            libc::ioctl(
                input.as_raw_fd(),
                0x80081272_u64 as libc::c_ulong,
                &mut capacity,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if capacity != drive.capacity {
            return Err("Partition capacity changed before image publication".into());
        }
        Ok(())
    };
    image_from(
        db,
        drive,
        destination,
        &input,
        cancel,
        &mut progress,
        validate,
    )
}
pub(crate) fn image_from(
    db: &Store,
    drive: &Drive,
    destination: &Root,
    input: &File,
    cancel: &AtomicBool,
    mut progress: impl FnMut(PartitionPhase, u64) -> Result<()>,
    validate_source: impl Fn() -> Result<()>,
) -> Result<Backup> {
    if drive.capacity == 0 {
        return Err("Partition capacity is unknown; cannot create a complete image".into());
    }
    validate_source()?;
    let parent = backup_parent(destination, drive)?;
    let pin = parent.directory(Path::new(""), false)?;
    let mut staged = tempfile::NamedTempFile::new_in(anchor(&pin))?;
    let prior = last(db, drive, BackupKind::Image)?;
    let incremental = btrfs(&parent)?;
    let clone = if incremental {
        prior
            .as_ref()
            .map(|p| kernel_interface::open_content(&p.path, false))
            .transpose()?
            .is_some_and(|p| unsafe {
                libc::ioctl(
                    staged.as_file().as_raw_fd(),
                    0x40049409_u64 as libc::c_ulong,
                    p.as_raw_fd(),
                ) == 0
            })
    } else {
        false
    };
    if incremental && prior.is_some() && !clone {
        return Err("Unable to reflink prior partition image; old image retained".into());
    }
    let (written, digest) = copy_image(
        input,
        staged.as_file_mut(),
        drive.capacity,
        clone,
        cancel,
        &mut progress,
        &validate_source,
    )?;
    validate_source()?;
    parent.validate()?;
    staged.as_file().sync_all()?;
    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("{}-{id}.img", now());
    let path = parent.path.join(&name);
    let backup = Backup {
        manifest: None,
        manifest_sha256: None,
        sha256: Some(digest),
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        kind: BackupKind::Image,
        path: path.clone(),
        created: now(),
        incremental: clone,
        bytes_written: written,
        source_size: drive.capacity,
        mount: None,
        loop_object: None,
        revision: None,
        identity: Some(crate::StorageIdentity::pinned(staged.as_file())?),
    };
    let manifest = publication::Manifest::new(&parent)?;
    progress(PartitionPhase::PublishingImage, drive.capacity)?;
    publication::publish(
        db,
        &parent,
        manifest,
        backup,
        staged
            .path()
            .file_name()
            .ok_or("Missing image staging name")?,
        drive,
        cancel,
    )
}
fn copy_image(
    input: &File,
    output: &mut File,
    size: u64,
    delta: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(PartitionPhase, u64) -> Result<()>,
    validate_source: impl Fn() -> Result<()>,
) -> Result<(u64, String)> {
    let mut buffer = vec![0; 256 * 1024];
    let mut old = vec![0; buffer.len()];
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    let mut written = 0;
    output.set_len(size)?;
    // Both passes use the same bounded reads and lifetime checks. The second
    // pass checks every output byte against the still-owned source descriptor.
    for verify in [false, true] {
        let phase = if verify {
            PartitionPhase::VerifyingImage
        } else {
            PartitionPhase::CopyingImage
        };
        progress(phase, 0)?;
        let mut offset = 0;
        while offset < size {
            cancelled(cancel)?;
            if offset % (64 * 1024 * 1024) == 0 {
                validate_source()?;
            }
            let n = (size - offset).min(buffer.len() as u64) as usize;
            input.read_exact_at(&mut buffer[..n], offset)?;
            if verify {
                output.read_exact_at(&mut old[..n], offset)?;
                if old[..n] != buffer[..n] {
                    return Err(
                        "Image verification failed; source changed or output is damaged".into(),
                    );
                }
            } else {
                digest.update(&buffer[..n]);
                let equal = delta
                    && output.read_exact_at(&mut old[..n], offset).is_ok()
                    && old[..n] == buffer[..n];
                if !equal && (buffer[..n].iter().any(|b| *b != 0) || delta) {
                    output.write_all_at(&buffer[..n], offset)?;
                    written += n as u64;
                }
            }
            offset += n as u64;
            progress(phase, offset)?;
        }
    }
    Ok((written, format!("{:x}", digest.finalize())))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn drive(mount: Option<PathBuf>) -> Drive {
        Drive {
            id: "part:test".into(),
            label: "Test".into(),
            device: "/dev/not-real".into(),
            mount,
            part_uuid: Some("test".into()),
            filesystem_uuid: Some("fs".into()),
            filesystem: "vfat".into(),
            capacity: 1048576,
            drive_capacity: 1048576,
            disk_sequence: Some(1),
            removable: true,
            read_only: false,
            evidence: "fixture".into(),
            ambiguous: false,
        }
    }
    struct FileFixture {
        db: Store,
        source: Root,
        destination: Root,
        drive: Drive,
        cancel: AtomicBool,
        temp: tempfile::TempDir,
    }
    impl FileFixture {
        fn backup(&self, progress: impl FnMut(u64, u64)) -> Result<Backup> {
            files(
                &self.db,
                &self.drive,
                &self.source,
                &self.destination,
                &self.cancel,
                progress,
            )
        }
        fn new(contents: &[u8]) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let source = Root::create(&temp.path().join("source")).unwrap();
            let destination = Root::create(&temp.path().join("backups")).unwrap();
            fs::write(source.path.join("file"), contents).unwrap();
            let db = Store::open(&temp.path().join("db")).unwrap();
            let drive = drive(Some(source.path.clone()));
            Self {
                db,
                source,
                destination,
                drive,
                cancel: AtomicBool::new(false),
                temp,
            }
        }
    }
    #[test]
    fn bulk_backups_refuse_replacements_of_their_admitted_roots() {
        for replaced in ["source", "destination"] {
            let temp = tempfile::tempdir().unwrap();
            let source = Root::create(&temp.path().join("source")).unwrap();
            let destination = Root::create(&temp.path().join("destination")).unwrap();
            fs::write(source.path.join("file"), b"admitted source").unwrap();
            let drive = drive(Some(source.path.clone()));
            let db = Store::open(&temp.path().join("db")).unwrap();
            let path = temp.path().join(replaced);
            fs::rename(&path, path.with_extension("original")).unwrap();
            fs::create_dir(&path).unwrap();
            fs::write(path.join("canary"), b"unrelated replacement").unwrap();
            let cancel = AtomicBool::new(false);
            let error = files(&db, &drive, &source, &destination, &cancel, |_, _| {}).unwrap_err();
            assert!(
                error.to_string().contains("replaced or removed"),
                "{replaced}: {error}"
            );
            if replaced == "destination" {
                let input = File::open(source.path.join("file")).unwrap();
                assert!(
                    image_from(
                        &db,
                        &drive,
                        &destination,
                        &input,
                        &cancel,
                        |_, _| Ok(()),
                        || Ok(())
                    )
                    .is_err()
                );
            }
            assert!(!destination.path.join("test").exists());
            assert_eq!(
                fs::read(path.join("canary")).unwrap(),
                b"unrelated replacement"
            );
            assert!(db.list::<Backup>("backup", 0, 10).unwrap().is_empty());
        }
    }
    #[test]
    fn filesystem_backup_versions_are_complete_and_independent() {
        let fixture = FileFixture::new(b"first");
        let first = fixture.backup(|_, _| {}).unwrap();
        fs::write(fixture.source.path.join("file"), b"second").unwrap();
        let second = fixture.backup(|_, _| {}).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
        assert_eq!(fs::read(second.path.join("file")).unwrap(), b"second");
        let manifest = Store::existing(second.manifest.as_ref().unwrap(), true).unwrap();
        let entry = scan::entry(
            &manifest,
            &format!("backup:{}", second.id),
            Path::new("file"),
        )
        .unwrap()
        .unwrap();
        use sha2::{Digest, Sha256};
        assert_eq!(entry.hash, Some(format!("{:x}", Sha256::digest(b"second"))));
        assert_eq!(
            publication::current(&fixture.db, &fixture.drive.id, BackupKind::Files)
                .unwrap()
                .unwrap()
                .id,
            second.id
        );
        let reference: Backup = serde_json::from_slice(
            &fs::read(second.path.parent().unwrap().join("current-files.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(reference.id, second.id);
        publication::check_manifest(&first).unwrap();
        publication::check_manifest(&second).unwrap();
    }
    #[test]
    fn interrupted_reference_installation_is_recovered_without_recopying_or_advancing_early() {
        let fixture = FileFixture::new(b"first");
        let first = fixture.backup(|_, _| {}).unwrap();
        let pointer = first.path.parent().unwrap().join("current-files.json");
        let retained = fixture.temp.path().join("old-current.json");
        fs::rename(&pointer, &retained).unwrap();
        fs::create_dir(&pointer).unwrap();
        fs::write(fixture.source.path.join("file"), b"second").unwrap();
        assert!(fixture.backup(|_, _| {}).is_err());
        assert_eq!(
            publication::current(&fixture.db, &fixture.drive.id, BackupKind::Files)
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert_eq!(
            fixture.db.list::<Backup>("backup", 0, 100).unwrap().len(),
            1
        );
        fs::remove_dir(&pointer).unwrap();
        fs::rename(&retained, &pointer).unwrap();
        recover_publications(&fixture.db).unwrap();
        let second = publication::current(&fixture.db, &fixture.drive.id, BackupKind::Files)
            .unwrap()
            .unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
        assert_eq!(fs::read(second.path.join("file")).unwrap(), b"second");
        recover_publications(&fixture.db).unwrap();
        assert_eq!(
            fixture.db.list::<Backup>("backup", 0, 100).unwrap().len(),
            2
        );
        let recorded: Backup = serde_json::from_slice(&fs::read(pointer).unwrap()).unwrap();
        assert_eq!(recorded.id, second.id);
    }
    #[test]
    fn recovery_refuses_a_modified_manifest_and_exposes_the_retained_artifact() {
        let fixture = FileFixture::new(b"first");
        let first = fixture.backup(|_, _| {}).unwrap();
        let pointer = first.path.parent().unwrap().join("current-files.json");
        let retained = fixture.temp.path().join("old-current.json");
        fs::rename(&pointer, &retained).unwrap();
        fs::create_dir(&pointer).unwrap();
        fs::write(fixture.source.path.join("file"), b"second").unwrap();
        assert!(fixture.backup(|_, _| {}).is_err());
        let pending: serde_json::Value = fixture
            .db
            .list("backup-publication", 0, 1)
            .unwrap()
            .pop()
            .unwrap();
        let prepared: Backup = serde_json::from_value(pending["backup"].clone()).unwrap();
        let manifest = Store::existing(prepared.manifest.as_ref().unwrap(), false).unwrap();
        manifest.put("tampered", "test", &true).unwrap();
        drop(manifest);
        fs::remove_dir(&pointer).unwrap();
        fs::rename(&retained, &pointer).unwrap();
        recover_publications(&fixture.db).unwrap();
        assert_eq!(
            publication::current(&fixture.db, &fixture.drive.id, BackupKind::Files)
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        let failures: Vec<crate::DriveRecovery> = fixture.db.list("recovery", 0, 10).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            failures[0].cause,
            crate::RecoveryCause::ChangedIdentityOrContent
        ));
        assert_eq!(failures[0].artifact, prepared.path);
        assert_eq!(fs::read(prepared.path.join("file")).unwrap(), b"second");
    }
    #[test]
    fn pruning_removes_matching_manifests_and_preserves_current_across_clock_changes() {
        let fixture = FileFixture::new(b"one");
        let mut first = fixture.backup(|_, _| {}).unwrap();
        fs::write(fixture.source.path.join("file"), b"two").unwrap();
        let second = fixture.backup(|_, _| {}).unwrap();
        fs::write(fixture.source.path.join("file"), b"three").unwrap();
        let third = fixture.backup(|_, _| {}).unwrap();
        first.created = u64::MAX;
        fixture.db.put("backup", &first.id, &first).unwrap();
        prune(&fixture.db, &fixture.drive.id, 1, &fixture.cancel).unwrap();
        assert!(!second.path.exists());
        assert!(!second.manifest.unwrap().exists());
        assert!(third.path.exists());
        assert!(third.manifest.as_ref().unwrap().exists());
        assert_eq!(
            publication::current(&fixture.db, &fixture.drive.id, BackupKind::Files)
                .unwrap()
                .unwrap()
                .id,
            third.id
        );
        let reference: Backup = serde_json::from_slice(
            &fs::read(third.path.parent().unwrap().join("current-files.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(reference.id, third.id);
    }
    #[test]
    fn cancellation_after_a_completed_copy_keeps_old_current_and_retains_partial_data() {
        let fixture = FileFixture::new(b"first");
        let first = fixture.backup(|_, _| {}).unwrap();
        fs::write(fixture.source.path.join("file"), b"changed").unwrap();
        fs::write(fixture.source.path.join("later"), b"later").unwrap();
        let parent = first.path.parent().unwrap();
        let progress = |_, _| {
            let completed_partial = fs::read_dir(parent).unwrap().flatten().any(|entry| {
                entry
                    .file_name()
                    .as_encoded_bytes()
                    .starts_with(b".incomplete-")
                    && entry.path().join("file").is_file()
            });
            if completed_partial {
                fixture
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Release);
            }
        };
        assert!(fixture.backup(progress).is_err());
        assert!(fixture.cancel.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            publication::current(&fixture.db, &fixture.drive.id, BackupKind::Files)
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
        assert!(fs::read_dir(parent).unwrap().flatten().any(|e| {
            e.file_name()
                .as_encoded_bytes()
                .starts_with(b".incomplete-")
                && fs::read(e.path().join("file")).ok().as_deref() == Some(b"changed".as_slice())
        }));
    }
    #[test]
    fn image_is_exact_including_nonzero_unused_sectors() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let bytes = vec![7; 1048576];
        fs::write(&source, &bytes).unwrap();
        let input = File::open(source).unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let phases = std::cell::RefCell::new(Vec::new());
        let reject = std::cell::Cell::new(None);
        let make = |validate: &dyn Fn() -> Result<()>| {
            image_from(
                &db,
                &drive(None),
                &Root::create(&temp.path().join("backups")).unwrap(),
                &input,
                &AtomicBool::new(false),
                |phase, _| {
                    let mut phases = phases.borrow_mut();
                    if phases.last() != Some(&phase) {
                        phases.push(phase);
                    }
                    if reject.get() == Some(phase) {
                        Err("owned checkpoint write failure".into())
                    } else {
                        Ok(())
                    }
                },
                validate,
            )
        };
        let result = make(&|| Ok(())).unwrap();
        assert_eq!(fs::read(&result.path).unwrap(), bytes);
        use sha2::{Digest, Sha256};
        assert_eq!(result.sha256, Some(format!("{:x}", Sha256::digest(&bytes))));
        publication::check_manifest(&result).unwrap();
        assert_eq!(
            *phases.borrow(),
            [
                PartitionPhase::CopyingImage,
                PartitionPhase::VerifyingImage,
                PartitionPhase::PublishingImage
            ]
        );
        phases.borrow_mut().clear();
        let checks = std::cell::Cell::new(0);
        let error = make(&|| {
            checks.set(checks.get() + 1);
            if checks.get() == 3 {
                Err("owned disk lifetime changed during verification".into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("lifetime changed during verification")
        );
        assert_eq!(checks.get(), 3);
        assert_eq!(
            phases.borrow().last(),
            Some(&PartitionPhase::VerifyingImage)
        );
        reject.set(Some(PartitionPhase::PublishingImage));
        assert!(
            make(&|| Ok(()))
                .unwrap_err()
                .to_string()
                .contains("checkpoint write failure")
        );
        assert_eq!(
            publication::current(&db, &drive(None).id, BackupKind::Image)
                .unwrap()
                .unwrap()
                .id,
            result.id
        );
        assert_eq!(db.list::<Backup>("backup", 0, 10).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod btrfs_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    struct Fixture(tempfile::TempDir);
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(self.0.path().join("backups/test")) {
                for entry in entries.flatten() {
                    if fs::symlink_metadata(entry.path())
                        .is_ok_and(|m| m.is_dir() && m.ino() == 256)
                    {
                        let _ = CommandRequest::new("btrfs")
                            .args(["property", "set", "-ts"])
                            .arg(entry.path())
                            .args(["ro", "false"])
                            .run();
                        let _ = CommandRequest::new("btrfs")
                            .args(["subvolume", "delete"])
                            .arg(entry.path())
                            .run();
                    }
                }
            }
        }
    }
    #[test]
    #[ignore = "requires an explicitly selected writable Btrfs fixture directory"]
    fn btrfs_small_file_and_image_edits_preserve_old_versions_and_share_extents() {
        let base = std::env::var_os("LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR")
            .expect("set LITTLE_CLIP_DRIVE_TEST_BACKUP_DIR");
        let fixture = Fixture(
            tempfile::Builder::new()
                .prefix("little-clip-btrfs-test-")
                .tempdir_in(base)
                .unwrap(),
        );
        let data = fixture.0.path().join("source");
        fs::create_dir(&data).unwrap();
        let mut bytes = (0..8 * 1024 * 1024)
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(data.join("large"), &bytes).unwrap();
        let drive = Drive {
            id: "part:test".into(),
            label: "Fixture".into(),
            device: "/dev/not-real".into(),
            mount: Some(data.clone()),
            part_uuid: Some("test".into()),
            filesystem_uuid: None,
            filesystem: "vfat".into(),
            capacity: bytes.len() as u64,
            drive_capacity: bytes.len() as u64,
            disk_sequence: Some(1),
            removable: true,
            read_only: false,
            evidence: "fixture".into(),
            ambiguous: false,
        };
        let db = Store::open(&fixture.0.path().join("db")).unwrap();
        let destination = fixture.0.path().join("backups");
        let cancel = AtomicBool::new(false);
        let source_root = Root::open(&data).unwrap();
        let destination_root = Root::create(&destination).unwrap();
        let make_files = || {
            files(
                &db,
                &drive,
                &source_root,
                &destination_root,
                &cancel,
                |_, _| {},
            )
        };
        let make_image = |input: &File| {
            image_from(
                &db,
                &drive,
                &destination_root,
                input,
                &cancel,
                |_, _| Ok(()),
                || Ok(()),
            )
        };
        let first = make_files().unwrap();
        bytes[4096..8192].fill(42);
        fs::write(data.join("large"), &bytes).unwrap();
        let second = make_files().unwrap();
        assert!(first.incremental && second.incremental);
        assert!(second.bytes_written <= 256 * 1024);
        assert_ne!(fs::read(first.path.join("large")).unwrap(), bytes);
        assert_eq!(fs::read(second.path.join("large")).unwrap(), bytes);
        let source = File::open(data.join("large")).unwrap();
        let image1 = make_image(&source).unwrap();
        bytes[1024 * 1024..1024 * 1024 + 4096].fill(19);
        fs::write(data.join("large"), &bytes).unwrap();
        let source = File::open(data.join("large")).unwrap();
        let image2 = make_image(&source).unwrap();
        assert!(image2.incremental);
        assert!(image2.bytes_written <= 256 * 1024);
        assert_eq!(fs::read(&image2.path).unwrap(), bytes);
        assert_ne!(fs::read(&image1.path).unwrap(), bytes);
        let output = CommandRequest::new("btrfs")
            .args(["filesystem", "du", "--raw", "--summarize"])
            .arg(&first.path)
            .arg(&second.path)
            .arg(&image1.path)
            .arg(&image2.path)
            .run()
            .unwrap();
        assert!(output.status.success());
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    }
}

pub(crate) fn mount_image(db: &Store, id: &str) -> Result<String> {
    let mut backup: Backup = db.get("backup", id)?.ok_or("Unknown backup version")?;
    if backup.kind != BackupKind::Image {
        return Err("This backup is a file tree, not an image".into());
    }
    if backup.mount.is_some() || backup.loop_object.is_some() {
        return Err("Image already has a loop mapping; unmount it before retrying".into());
    }
    let file = kernel_interface::open_content(&backup.path, false)?;
    let source = little_clip_core::file_source::FileSource::capture_file(&file)?;
    verify_image(&backup, &file)?;
    let connection = system_connection()?;
    let manager = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.UDisks2",
        "/org/freedesktop/UDisks2",
        "org.freedesktop.UDisks2.Manager",
    )
    .map_err(|e| e.to_string())?;
    let mut options = HashMap::new();
    options.insert("read-only", zbus::zvariant::Value::from(true));
    let object: zbus::zvariant::OwnedObjectPath = manager
        .call("LoopSetup", &(zbus::zvariant::Fd::from(&file), options))
        .map_err(|e| e.to_string())?;
    backup.loop_object = Some(object.to_string());
    db.put("backup", id, &backup)?;
    let filesystem = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.UDisks2",
        object.as_str(),
        "org.freedesktop.UDisks2.Filesystem",
    )
    .map_err(|e| e.to_string())?;
    let mut options = HashMap::new();
    options.insert(
        "options",
        zbus::zvariant::Value::from("ro,nosuid,nodev,noexec"),
    );
    let result = (|| -> Result<String> {
        let mounted: String = filesystem
            .call("Mount", &(options,))
            .map_err(|e| e.to_string())?;
        let pin = kernel_interface::pin_content_directory(Path::new(&mounted))?;
        if !rustix::fs::fstatvfs(&pin)?
            .f_flag
            .contains(rustix::fs::StatVfsMountFlags::RDONLY)
        {
            return Err("Image mount did not become read-only".into());
        }
        source.check_file(&file)?;
        backup.mount = Some(mounted.clone().into());
        db.put("backup", id, &backup)?;
        Ok(format!("Image mounted read-only at {mounted}"))
    })();
    if result.is_err() {
        let _ = unmount_image(db, id);
    }
    result
}
pub(crate) fn unmount_image(db: &Store, id: &str) -> Result<String> {
    let mut backup: Backup = db.get("backup", id)?.ok_or("Unknown backup version")?;
    let object = backup
        .loop_object
        .clone()
        .ok_or("Image has no managed loop mapping")?;
    let connection = system_connection()?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    if backup.mount.is_some() {
        let filesystem = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.UDisks2",
            object.as_str(),
            "org.freedesktop.UDisks2.Filesystem",
        )
        .map_err(|e| e.to_string())?;
        filesystem
            .call::<_, _, ()>("Unmount", &(options,))
            .map_err(|e| e.to_string())?;
        backup.mount = None;
        db.put("backup", id, &backup)?;
    }
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.UDisks2",
        object.as_str(),
        "org.freedesktop.UDisks2.Loop",
    )
    .map_err(|e| e.to_string())?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    proxy
        .call::<_, _, ()>("Delete", &(options,))
        .map_err(|e| e.to_string())?;
    backup.loop_object = None;
    db.put("backup", id, &backup)?;
    Ok("Read-only backup image unmounted".into())
}
pub(crate) fn prune(db: &Store, drive: &str, keep: usize, cancel: &AtomicBool) -> Result<String> {
    if keep == 0 {
        return Err("Retain at least one completed backup version".into());
    }
    let catalogs: Vec<crate::Catalog> = db.list("catalog", 0, 1000)?;
    if catalogs
        .iter()
        .any(|c| c.drive == drive && c.mount.is_some())
    {
        return Err(
            "Unmount this drive's catalogs before pruning potential content providers".into(),
        );
    }
    let mut versions = Vec::new();
    let mut offset = 0;
    loop {
        let page: Vec<Backup> = db.list("backup", offset, 128)?;
        if page.is_empty() {
            break;
        }
        versions.extend(page.into_iter().filter(|v| v.drive == drive));
        offset += 128;
    }
    versions.sort_by_key(|v| std::cmp::Reverse(v.created));
    let current_files = publication::current(db, drive, BackupKind::Files)?.map(|b| b.id);
    let current_image = publication::current(db, drive, BackupKind::Image)?.map(|b| b.id);
    let mut files = 0;
    let mut images = 0;
    let mut removed = 0;
    for version in versions {
        let count = if version.kind == BackupKind::Files {
            &mut files
        } else {
            &mut images
        };
        *count += 1;
        if *count <= keep
            || current_files.as_ref() == Some(&version.id)
            || current_image.as_ref() == Some(&version.id)
        {
            continue;
        }
        cancelled(cancel)?;
        if version.mount.is_some() || version.loop_object.is_some() {
            continue;
        }
        let parent = Root::open(version.path.parent().ok_or("Missing backup parent")?)?;
        let name = version.path.file_name().ok_or("Missing backup name")?;
        if !name
            .as_encoded_bytes()
            .windows(version.id.len())
            .any(|w| w == version.id.as_bytes())
        {
            return Err("Backup version path no longer matches its recorded identity".into());
        }
        publication::check_manifest(&version)?;
        if version.kind == BackupKind::Image {
            let file = kernel_interface::open_content(&version.path, false)?;
            verify_image(&version, &file)?;
            let (_pin, path) = parent.parent(Path::new(name), false)?;
            fs::remove_file(path)?;
        } else {
            let root = Root::open(&version.path)?;
            if version.identity.as_ref() != Some(&root.storage_identity()?) {
                return Err("Backup directory identity changed; refusing prune".into());
            }
            let coverage = scan::scan(
                db,
                &root,
                "prune-check",
                &Limits {
                    hashes: false,
                    ..Default::default()
                },
                cancel,
                |_, _| {},
            )?;
            if !coverage.complete {
                return Err("Cannot validate backup tree for pruning".into());
            }
            root.validate()?;
            if btrfs(&root)? {
                let pin = parent.directory(Path::new(""), false)?;
                command(&["subvolume", "delete"], &pin, &[name])?;
            } else {
                fs::remove_dir_all(&root.path)?;
            }
        }
        publication::remove_manifest(&parent, &version, cancel)?;
        db.remove("backup", &version.id)?;
        removed += 1;
    }
    Ok(format!("Pruned {removed} old completed backup versions"))
}

fn verify_image(backup: &Backup, file: &File) -> Result<()> {
    let source = little_clip_core::file_source::FileSource::capture_file(file)?;
    let actual = source.revision_key();
    if backup.identity.as_ref() != Some(&crate::StorageIdentity::pinned(file)?)
        || backup
            .revision
            .as_ref()
            .is_none_or(|old| old.get(8..) != actual.get(8..))
    {
        return Err("Backup image identity or revision changed".into());
    }
    Ok(())
}
