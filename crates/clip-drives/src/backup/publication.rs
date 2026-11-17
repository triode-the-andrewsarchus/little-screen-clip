//! Durable version metadata and recoverable publication of fully prepared backups.
use super::*;
use little_clip_core::file_source::{FileSource, create_file_in, rename_noreplace};
use serde::{Deserialize, Serialize};

pub(super) struct Manifest {
    pub db: Store,
    path: PathBuf,
    scratch: tempfile::TempDir,
    // Keep the descriptor-relative scratch address valid until it is sealed.
    _parent: File,
}
impl Manifest {
    pub fn new(parent: &Root) -> Result<Self> {
        let pin = parent.directory(Path::new(""), false)?;
        let scratch = tempfile::Builder::new()
            .prefix(".manifest-")
            .tempdir_in(anchor(&pin))?;
        let path = scratch.path().join("scan.sqlite");
        let db = Store::open(&path)?;
        Ok(Self {
            db,
            path,
            scratch,
            _parent: pin,
        })
    }
    fn seal(
        self,
        backup: &Backup,
        drive: &Drive,
        cancel: &AtomicBool,
    ) -> Result<(PathBuf, String)> {
        // The backup header deliberately omits its own manifest checksum.
        self.db.put("backup", &backup.id, backup)?;
        self.db.put("source", "drive", drive)?;
        self.db.put("format", "manifest", &1_u32)?;
        self.db.0.execute(
            "DELETE FROM entries WHERE scan<>?1",
            [format!("backup:{}", backup.id)],
        )?;
        self.db
            .0
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
        self.db.0.close().map_err(|(_, error)| error)?;
        let file = kernel_interface::open_content(&self.path, false)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
        let checksum = FileSource::capture_file(&file)?.sha256(&file, cancel, Duration::MAX)?;
        let path = FileSource::capture_file(&file)?.path().to_owned();
        File::open(self.scratch.path())?.sync_all()?;
        // Once prepared, metadata must survive an installation failure as well as a crash.
        let _ = self.scratch.keep();
        Ok((path, checksum))
    }
}
#[derive(Serialize, Deserialize)]
struct Pending {
    parent_identity: crate::StorageIdentity,
    backup: Backup,
    #[serde(with = "little_clip_core::path_serde")]
    staged: PathBuf,
    #[serde(with = "little_clip_core::path_serde")]
    manifest_staged: PathBuf,
}
pub(super) fn current_key(drive: &str, kind: BackupKind) -> String {
    format!("{drive}:{kind:?}")
}
pub(crate) fn current(db: &Store, drive: &str, kind: BackupKind) -> Result<Option<Backup>> {
    let Some(id) = db.get_bounded::<String>("backup-current", &current_key(drive, kind), 128)?
    else {
        return Ok(None);
    };
    Ok(db
        .get_bounded::<Backup>("backup", &id, 64 * 1024)?
        .filter(|version| version.id == id))
}
pub(super) fn publish(
    db: &Store,
    parent: &Root,
    manifest: Manifest,
    mut backup: Backup,
    staged_name: &std::ffi::OsStr,
    drive: &Drive,
    cancel: &AtomicBool,
) -> Result<Backup> {
    parent.validate()?;
    cancelled(cancel)?;
    backup.manifest = Some(parent.path.join(format!("{}.manifest.sqlite", backup.id)));
    let (manifest_staged, checksum) = manifest.seal(&backup, drive, cancel)?;
    backup.manifest_sha256 = Some(checksum);
    let pending = Pending {
        parent_identity: parent.storage_identity()?,
        backup,
        staged: parent.path.join(staged_name),
        manifest_staged,
    };
    db.put("backup-publication", &pending.backup.id, &pending)?;
    install(db, parent, pending, cancel)
}
fn artifact_identity(path: &Path, kind: BackupKind) -> Result<crate::StorageIdentity> {
    let held = kernel_interface::pin(path, false)?;
    let metadata = held.metadata()?;
    if (kind == BackupKind::Files && !metadata.is_dir())
        || (kind == BackupKind::Image && !metadata.is_file())
    {
        return Err("Prepared backup artifact has a different type".into());
    }
    Ok(crate::StorageIdentity::pinned(&held)?)
}
fn validate_manifest(backup: &Backup, path: &Path, cancel: &AtomicBool) -> Result<()> {
    let mut bytes = u64::MAX;
    read_manifest(backup, path, cancel, &mut bytes, Duration::MAX, |_| Ok(()))
}

/// Validates the locally owned completed manifest location.
pub(crate) fn completed_manifest(backup: &Backup) -> Option<&Path> {
    let path = backup.manifest.as_deref()?;
    (uuid::Uuid::parse_str(&backup.id).is_ok()
        && path.parent() == backup.path.parent()
        && path.file_name()
            == Some(std::ffi::OsStr::new(&format!(
                "{}.manifest.sqlite",
                backup.id
            ))))
    .then_some(path)
}

/// Keeps checksum verification, SQLite queries and the final revision check on
/// one retained file. The callback cannot retarget a verified pathname.
/// Reserves its observed length from the remaining verification allowance before
/// hashing; failed checks retain that reservation. Pre-admission cancellation or
/// a zero deadline leaves the allowance untouched.
pub(crate) fn read_manifest<T>(
    backup: &Backup,
    path: &Path,
    cancel: &AtomicBool,
    bytes: &mut u64,
    timeout: Duration,
    read: impl FnOnce(&Store) -> Result<T>,
) -> Result<T> {
    cancelled(cancel)?;
    if timeout.is_zero() {
        return Err("Backup manifest read timed out before admission".into());
    }
    // A manifest may be used by a single-threaded catalog server. Refuse leaf
    // mount crossings before opening a foreign FUSE inode, including our own.
    let parent = Root::open(path.parent().ok_or("Missing manifest parent")?)?;
    let file = parent.input(Path::new(path.file_name().ok_or("Missing manifest name")?))?;
    let (source, manifest) = verified_manifest(backup, &file, cancel, bytes, timeout)?;
    let result = read(&manifest)?;
    source.check_file(&file)?;
    parent.validate()?;
    Ok(result)
}

/// Verifies the retained input before admitting any caller's SQLite queries.
fn verified_manifest(
    backup: &Backup,
    file: &File,
    cancel: &AtomicBool,
    bytes: &mut u64,
    timeout: Duration,
) -> Result<(FileSource, Store)> {
    let source = FileSource::capture_file(file)?;
    let length = source.observed_size();
    if length > *bytes {
        return Err("Backup manifest exceeds the metadata reuse byte budget".into());
    }
    // Reserve the exact revision length before hashing, including failed checks.
    // FileSource hashes only that observed length and rejects a changed revision.
    *bytes -= length;
    let hash = source.sha256(file, cancel, timeout)?;
    if backup.manifest_sha256.as_deref() != Some(hash.as_str()) {
        return Err("Prepared backup manifest changed; publication suspended".into());
    }
    let manifest = Store::existing(&anchor(file), true)?;
    validate_header(backup, &manifest)?;
    source.check_file(file)?;
    Ok((source, manifest))
}
fn validate_header(backup: &Backup, manifest: &Store) -> Result<()> {
    let header: Backup = manifest
        .get_bounded("backup", &backup.id, 64 * 1024)?
        .ok_or("Missing manifest backup header")?;
    if header.id != backup.id
        || header.path != backup.path
        || header.identity != backup.identity
        || header.kind != backup.kind
        || header.drive != backup.drive
        || header.evidence != backup.evidence
        || header.sha256 != backup.sha256
        || header.source_size != backup.source_size
        || header.fidelity != backup.fidelity
    {
        return Err("Manifest does not describe the prepared backup".into());
    }
    Ok(())
}
fn install(db: &Store, parent: &Root, mut pending: Pending, cancel: &AtomicBool) -> Result<Backup> {
    let backup = &mut pending.backup;
    let manifest = backup.manifest.as_ref().ok_or("Missing manifest path")?;
    if parent.storage_identity()? != pending.parent_identity
        || backup.path.parent() != Some(parent.path.as_path())
        || pending.staged.parent() != Some(parent.path.as_path())
        || manifest.parent() != Some(parent.path.as_path())
        || pending.manifest_staged.file_name() != Some(std::ffi::OsStr::new("scan.sqlite"))
        || pending.manifest_staged.parent().and_then(Path::parent) != Some(parent.path.as_path())
        || !pending
            .manifest_staged
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b".manifest-"))
    {
        return Err("Backup publication directory identity or layout changed".into());
    }
    parent.validate()?;
    let pin = parent.directory(Path::new(""), false)?;
    let manifest_ready = parent
        .metadata(
            manifest
                .strip_prefix(&parent.path)
                .map_err(|e| e.to_string())?,
        )?
        .is_some();
    let manifest_input = if manifest_ready {
        manifest
    } else {
        &pending.manifest_staged
    };
    validate_manifest(backup, manifest_input, cancel)?;
    if !manifest_ready {
        rename_noreplace(
            manifest_input,
            &anchor(&pin).join(manifest.file_name().ok_or("Missing manifest name")?),
        )?;
    }
    File::open(anchor(&pin))?.sync_all()?;
    let final_name = backup.path.file_name().ok_or("Missing version name")?;
    let final_path = anchor(&pin).join(final_name);
    if parent.metadata(Path::new(final_name))?.is_none() {
        if Some(artifact_identity(&pending.staged, backup.kind)?) != backup.identity {
            return Err("Prepared backup artifact changed".into());
        }
        let stage_name = pending.staged.file_name().ok_or("Missing staging name")?;
        rename_noreplace(&anchor(&pin).join(stage_name), &final_path)?;
    }
    if Some(artifact_identity(&final_path, backup.kind)?) != backup.identity {
        return Err("Published backup artifact identity changed".into());
    }
    File::open(anchor(&pin))?.sync_all()?;
    if backup.kind == BackupKind::Image {
        let file = kernel_interface::open_content(&final_path, false)?;
        backup.revision = Some(FileSource::capture_file(&file)?.revision_key());
    }
    parent.validate()?;
    publish_record(db, parent, &pin, backup)?;
    if let Some(directory) = pending.manifest_staged.parent() {
        // Remove only our now-empty scratch directory; never recursively delete recovery metadata.
        if directory
            .file_name()
            .is_some_and(|n| n.as_encoded_bytes().starts_with(b".manifest-"))
        {
            let _ = fs::remove_dir(directory);
        }
    }
    Ok(pending.backup)
}
fn publish_record(db: &Store, parent: &Root, pin: &File, backup: &Backup) -> Result<()> {
    let advance = advance_current(db, backup)?;
    if advance {
        write_current(parent, pin, backup)?;
    }
    // Artifacts and any head update precede atomic registry publication.
    let transaction = db.0.unchecked_transaction()?;
    db.put("backup", &backup.id, backup)?;
    if advance {
        db.put(
            "backup-current",
            &current_key(&backup.drive, backup.kind),
            &backup.id,
        )?;
    }
    db.remove("backup-publication", &backup.id)?;
    transaction.commit()?;
    Ok(())
}
fn write_current(parent: &Root, pin: &File, backup: &Backup) -> Result<()> {
    let name = if backup.kind == BackupKind::Files {
        "current-files.json"
    } else {
        "current-image.json"
    };
    let pointer = parent.path.join(name);
    let bytes = serde_json::to_vec(backup)?;
    if parent.metadata(Path::new(name))?.is_some() {
        FileSource::capture(&pointer)?.write_in(&pointer, &bytes, pin)?;
    } else {
        create_file_in(&pointer, &bytes, pin)?;
    }
    Ok(())
}

fn advance_current(db: &Store, backup: &Backup) -> Result<bool> {
    let Some(current) = db.get_bounded::<String>(
        "backup-current",
        &current_key(&backup.drive, backup.kind),
        128,
    )?
    else {
        return Ok(true);
    };
    // Retry may finish an older prepared version after a newer version was
    // published. Registry registration order, not wall-clock time, owns the head.
    let superseded: bool = db.0.query_row("SELECT EXISTS(SELECT 1 FROM entities published JOIN entities pending ON pending.kind='backup-publication' AND pending.id=?1 WHERE published.kind='backup' AND published.id=?2 AND published.rowid>pending.rowid)", rusqlite::params![backup.id, current], |row| row.get(0))?;
    Ok(!superseded)
}

pub(super) fn check_manifest(backup: &Backup, cancel: &AtomicBool) -> Result<()> {
    if let Some(path) = &backup.manifest {
        validate_manifest(backup, path, cancel)?;
    }
    Ok(())
}
pub(super) fn remove_manifest(parent: &Root, backup: &Backup, cancel: &AtomicBool) -> Result<()> {
    if let Some(path) = &backup.manifest {
        if path.parent() != Some(parent.path.as_path()) {
            return Err("Backup manifest moved outside its recorded parent".into());
        }
        parent.delete_file(
            Path::new(path.file_name().ok_or("Missing manifest name")?),
            backup
                .manifest_sha256
                .as_deref()
                .ok_or("Missing manifest checksum")?,
            cancel,
        )?;
    }
    Ok(())
}
/// Reconcile only fully prepared publications; copying-stage data is never advertised as complete.
pub(crate) fn recover(db: &Store, cancel: &AtomicBool) -> Result<()> {
    let mut after = String::new();
    loop {
        cancelled(cancel)?;
        let page = db.ids("backup-publication", &after, 128)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = last.clone();
        for id in page {
            cancelled(cancel)?;
            if let Err(error) = recover_one(db, &id, cancel) {
                tracing::warn!("Prepared backup {id} remains pending: {error}");
            }
        }
    }
}

pub(super) fn recover_one(db: &Store, id: &str, cancel: &AtomicBool) -> Result<bool> {
    cancelled(cancel)?;
    let Some(pending) = db.get_bounded::<Pending>("backup-publication", id, 64 * 1024)? else {
        return Ok(false);
    };
    if pending.backup.id != id {
        return Err("Prepared backup record identifier changed".into());
    }
    let drive = pending.backup.drive.clone();
    let artifact = pending.backup.path.clone();
    let outcome = (|| {
        let parent = Root::open(
            pending
                .backup
                .path
                .parent()
                .ok_or("Missing publication parent")?,
        )?;
        install(db, &parent, pending, cancel)
    })();
    if let Err(error) = outcome {
        db.put(
            "recovery",
            id,
            &crate::DriveRecovery {
                id: id.into(),
                drive,
                cause: error.recovery_cause(),
                artifact,
                message: error.to_string(),
            },
        )?;
        return Err(error);
    }
    db.remove("recovery", id)?;
    Ok(true)
}
