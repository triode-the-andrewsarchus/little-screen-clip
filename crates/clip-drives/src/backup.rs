//! Versioned guarded filesystem backups and partition images with reflink deltas.
use crate::{
    BackupKind, Drive, Result, cancelled, now,
    root::{Root, anchor},
    scan::{self, Limits},
    store::Store,
    sync,
};
use little_clip_core::{kernel_interface, process::CommandRequest};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::fs::FileExt,
    },
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Backup {
    pub id: String,
    pub drive: String,
    pub evidence: String,
    pub kind: BackupKind,
    #[serde(with = "little_clip_core::path_serde")]
    pub path: PathBuf,
    pub created: u64,
    pub incremental: bool,
    pub bytes_written: u64,
    pub source_size: u64,
}
fn btrfs(root: &Root) -> Result<bool> {
    let pin = kernel_interface::pin_content_directory(&root.path)?;
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
    let mut offset = 0;
    loop {
        let versions: Vec<Backup> = db.list("backup", offset, 128)?;
        if versions.is_empty() {
            return Ok(None);
        }
        for version in versions {
            if version.drive == drive.id
                && version.evidence == drive.evidence
                && version.kind == kind
            {
                return Ok(Some(version));
            }
        }
        offset += 128;
    }
}
fn backup_parent(destination: &Path, drive: &Drive) -> Result<Root> {
    let uuid = drive
        .part_uuid
        .as_ref()
        .ok_or("This partition has no PARTUUID; enroll a stable identity before backing it up")?;
    if uuid.is_empty() || !uuid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err("Invalid partition UUID".into());
    }
    let base = Root::create(destination)?;
    base.ensure_directory(Path::new(uuid))?;
    Root::open(&base.path.join(uuid))
}
pub(crate) fn files(
    db: &Store,
    drive: &Drive,
    destination: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Backup> {
    let source = Root::open(
        drive
            .mount
            .as_deref()
            .ok_or("Mount the partition before a file backup")?,
    )?;
    let parent = backup_parent(destination, drive)?;
    if source.overlap(&parent) {
        return Err("Backup destination overlaps the source".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let temporary = format!(".incomplete-{id}");
    let completed = format!("{}-{id}", now());
    let pin = kernel_interface::pin_content_directory(&parent.path)?;
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
    let target = Root::open(&parent.path.join(&temporary))?;
    let scan_id = format!("backup:{id}");
    let coverage = scan::scan(
        db,
        &source,
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
        let batch = scan::entries(db, &scan_id, &after, 128)?;
        if batch.is_empty() {
            break;
        }
        for entry in batch {
            cancelled(cancel)?;
            let existing = target.metadata(&entry.path)?;
            if entry.kind == "directory" {
                target.ensure_directory(&entry.path)?;
            } else {
                let old = if existing.is_some() {
                    Some(snapshot_entry(&target, &entry.path, cancel)?)
                } else {
                    None
                };
                if old.as_ref().and_then(|e| e.hash.as_ref()) != entry.hash.as_ref()
                    || entry.kind != "file"
                {
                    written += sync::install(
                        &source,
                        &target,
                        &entry,
                        &entry.path,
                        old.as_ref(),
                        cancel,
                        incremental,
                    )?;
                }
            }
            count += 1;
            progress(count, written);
            after = entry.path.as_os_str().as_encoded_bytes().to_vec();
        }
    }
    // Remove only entries absent from the complete source manifest in this private version.
    if prior.is_some() && incremental {
        remove_obsolete(db, &scan_id, &target, cancel)?;
    }
    source.validate()?;
    target.validate()?;
    for entry in reverse_entries(db, &scan_id)? {
        if entry.kind == "directory" {
            let held = target.directory(&entry.path, false)?;
            let file = File::open(anchor(&held))?;
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(entry.mode & 0o777))?;
            if let Some(time) = time(entry.modified, entry.nanos) {
                file.set_times(fs::FileTimes::new().set_modified(time))?;
            }
        }
    }
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
    little_clip_core::file_source::rename_noreplace(
        &anchor(&pin).join(&temporary),
        &anchor(&pin).join(&completed),
    )?;
    File::open(anchor(&pin))?.sync_all()?;
    let backup = Backup {
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        kind: BackupKind::Files,
        path: parent.path.join(completed),
        created: now(),
        incremental,
        bytes_written: written,
        source_size: drive.capacity,
    };
    db.put("backup", &id, &backup)?;
    db.clear_scan(&scan_id)?;
    Ok(backup)
}
fn snapshot_entry(root: &Root, path: &Path, cancel: &AtomicBool) -> Result<scan::Entry> {
    use std::os::unix::fs::MetadataExt;
    let m = root.metadata(path)?.ok_or("Backup entry disappeared")?;
    let kind = if m.is_file() {
        "file"
    } else if m.is_dir() {
        "directory"
    } else if m.file_type().is_symlink() {
        "symlink"
    } else {
        return Err("Special file in backup destination".into());
    };
    Ok(scan::Entry {
        path: path.into(),
        kind: kind.into(),
        size: m.len(),
        modified: m.mtime(),
        nanos: m.mtime_nsec(),
        mode: m.mode(),
        device: m.dev(),
        inode: m.ino(),
        hash: if m.is_file() {
            Some(root.digest(path, cancel)?)
        } else {
            None
        },
        link: if m.file_type().is_symlink() {
            let (_p, p) = root.parent(path, false)?;
            Some(fs::read_link(p)?)
        } else {
            None
        },
    })
}
fn reverse_entries(db: &Store, id: &str) -> Result<Vec<scan::Entry>> {
    // Only directory metadata is retained here, bounded independently from file count.
    let mut statement=db.0.prepare("SELECT json FROM entries WHERE scan=?1 AND json_extract(json,'$.kind')='directory' ORDER BY path DESC LIMIT 250001")?;
    let result = statement
        .query_map([id], |r| r.get::<_, String>(0))?
        .map(|j| Ok(serde_json::from_str(&j?)?))
        .collect::<Result<Vec<_>>>()?;
    if result.len() > 250000 {
        return Err("Directory metadata budget exceeded".into());
    }
    Ok(result)
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
fn udisks_path(drive: &Drive) -> Result<String> {
    let connection = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
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
pub(crate) fn image(
    db: &Store,
    drive: &Drive,
    destination: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Backup> {
    if drive.mount.is_some() {
        return Err(
            "Unmount the partition before imaging it; flush pending RAM/sync changes first".into(),
        );
    }
    let connection = zbus::blocking::Connection::system().map_err(|e| e.to_string())?;
    let path = udisks_path(drive)?;
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.UDisks2",
        path.as_str(),
        "org.freedesktop.UDisks2.Block",
    )
    .map_err(|e| e.to_string())?;
    let options: HashMap<&str, zbus::zvariant::Value<'_>> = HashMap::new();
    let descriptor: zbus::zvariant::OwnedFd = proxy
        .call("OpenForBackup", &(options,))
        .map_err(|e| format!("UDisks authorization or exclusive read failed: {e}"))?;
    let fd: OwnedFd = descriptor.into();
    let input = File::from(fd);
    image_from(db, drive, destination, &input, cancel, &mut progress)
}
pub(crate) fn image_from(
    db: &Store,
    drive: &Drive,
    destination: &Path,
    input: &File,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Backup> {
    let parent = backup_parent(destination, drive)?;
    let pin = kernel_interface::pin_content_directory(&parent.path)?;
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
    let written = copy_image(
        input,
        staged.as_file_mut(),
        drive.capacity,
        clone,
        cancel,
        &mut progress,
    )?;
    staged.as_file().sync_all()?;
    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("{}-{id}.img", now());
    let path = parent.path.join(&name);
    staged
        .persist_noclobber(anchor(&pin).join(&name))
        .map_err(|e| e.error)?;
    File::open(anchor(&pin))?.sync_all()?;
    let backup = Backup {
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        kind: BackupKind::Image,
        path,
        created: now(),
        incremental: clone,
        bytes_written: written,
        source_size: drive.capacity,
    };
    db.put("backup", &id, &backup)?;
    Ok(backup)
}
fn copy_image(
    input: &File,
    output: &mut File,
    size: u64,
    delta: bool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<u64> {
    let mut buffer = vec![0; 256 * 1024];
    let mut old = vec![0; buffer.len()];
    let mut offset = 0;
    let mut written = 0;
    while offset < size {
        cancelled(cancel)?;
        let n = (size - offset).min(buffer.len() as u64) as usize;
        input.read_exact_at(&mut buffer[..n], offset)?;
        let equal =
            delta && output.read_exact_at(&mut old[..n], offset).is_ok() && old[..n] == buffer[..n];
        if !equal {
            if buffer[..n].iter().any(|b| *b != 0) || delta {
                output.write_all_at(&buffer[..n], offset)?;
                written += n as u64;
            }
        }
        offset += n as u64;
        progress(0, offset);
    }
    output.set_len(size)?;
    // Verify every byte against the still-owned source descriptor before publication.
    offset = 0;
    while offset < size {
        cancelled(cancel)?;
        let n = (size - offset).min(buffer.len() as u64) as usize;
        input.read_exact_at(&mut buffer[..n], offset)?;
        output.read_exact_at(&mut old[..n], offset)?;
        if old[..n] != buffer[..n] {
            return Err("Image verification failed; source changed or output is damaged".into());
        }
        offset += n as u64;
    }
    Ok(written)
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
            removable: true,
            read_only: false,
            evidence: "fixture".into(),
            ambiguous: false,
        }
    }
    #[test]
    fn filesystem_backup_versions_are_complete_and_independent() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("source");
        fs::create_dir(&data).unwrap();
        fs::write(data.join("file"), b"first").unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let drive = drive(Some(data.clone()));
        let cancel = AtomicBool::new(false);
        let destination = temp.path().join("backups");
        let first = files(&db, &drive, &destination, &cancel, |_, _| {}).unwrap();
        fs::write(data.join("file"), b"second").unwrap();
        let second = files(&db, &drive, &destination, &cancel, |_, _| {}).unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(fs::read(first.path.join("file")).unwrap(), b"first");
        assert_eq!(fs::read(second.path.join("file")).unwrap(), b"second");
    }
    #[test]
    fn image_is_exact_including_nonzero_unused_sectors() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let bytes = vec![7; 1048576];
        fs::write(&source, &bytes).unwrap();
        let input = File::open(source).unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let result = image_from(
            &db,
            &drive(None),
            &temp.path().join("backups"),
            &input,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(fs::read(result.path).unwrap(), bytes);
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
            removable: true,
            read_only: false,
            evidence: "fixture".into(),
            ambiguous: false,
        };
        let db = Store::open(&fixture.0.path().join("db")).unwrap();
        let destination = fixture.0.path().join("backups");
        let cancel = AtomicBool::new(false);
        let first = files(&db, &drive, &destination, &cancel, |_, _| {}).unwrap();
        bytes[4096..8192].fill(42);
        fs::write(data.join("large"), &bytes).unwrap();
        let second = files(&db, &drive, &destination, &cancel, |_, _| {}).unwrap();
        assert!(first.incremental && second.incremental);
        assert!(second.bytes_written <= 256 * 1024);
        assert_ne!(fs::read(first.path.join("large")).unwrap(), bytes);
        assert_eq!(fs::read(second.path.join("large")).unwrap(), bytes);
        let source = File::open(data.join("large")).unwrap();
        let image1 = image_from(&db, &drive, &destination, &source, &cancel, |_, _| {}).unwrap();
        bytes[1024 * 1024..1024 * 1024 + 4096].fill(19);
        fs::write(data.join("large"), &bytes).unwrap();
        let source = File::open(data.join("large")).unwrap();
        let image2 = image_from(&db, &drive, &destination, &source, &cancel, |_, _| {}).unwrap();
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
