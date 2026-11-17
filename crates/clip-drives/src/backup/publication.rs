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
fn current_key(drive: &str, kind: BackupKind) -> String {
    format!("{drive}:{kind:?}")
}
pub(crate) fn current(db: &Store, drive: &str, kind: BackupKind) -> Result<Option<Backup>> {
    let id = db.get::<String>("backup-current", &current_key(drive, kind))?;
    id.map(|id| db.get("backup", &id))
        .transpose()
        .map(Option::flatten)
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
    install(db, parent, pending)
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
fn validate_manifest(backup: &Backup, path: &Path) -> Result<()> {
    read_manifest(
        backup,
        path,
        &AtomicBool::new(false),
        u64::MAX,
        Duration::MAX,
        |_| Ok(()),
    )
}

/// Keeps checksum verification, SQLite queries and the final revision check on
/// one retained file. The callback cannot retarget a verified pathname.
pub(crate) fn read_manifest<T>(
    backup: &Backup,
    path: &Path,
    cancel: &AtomicBool,
    bytes: u64,
    timeout: Duration,
    read: impl FnOnce(&Store) -> Result<T>,
) -> Result<T> {
    let file = kernel_interface::open_content(path, false)?;
    let source = FileSource::capture_file(&file)?;
    if file.metadata()?.len() > bytes {
        return Err("Backup manifest exceeds the metadata reuse byte budget".into());
    }
    let hash = source.sha256(&file, cancel, timeout)?;
    if backup.manifest_sha256.as_deref() != Some(hash.as_str()) {
        return Err("Prepared backup manifest changed; publication suspended".into());
    }
    let manifest = Store::existing(&anchor(&file), true)?;
    validate_header(backup, &manifest)?;
    source.check_file(&file)?;
    let result = read(&manifest)?;
    source.check_file(&file)?;
    Ok(result)
}
fn validate_header(backup: &Backup, manifest: &Store) -> Result<()> {
    let header: Backup = manifest
        .get("backup", &backup.id)?
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
fn install(db: &Store, parent: &Root, mut pending: Pending) -> Result<Backup> {
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
    validate_manifest(backup, manifest_input)?;
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
    let pointer = parent.path.join(if backup.kind == BackupKind::Files {
        "current-files.json"
    } else {
        "current-image.json"
    });
    let bytes = serde_json::to_vec(backup)?;
    if parent
        .metadata(Path::new(
            pointer
                .file_name()
                .ok_or("Missing current reference name")?,
        ))?
        .is_some()
    {
        FileSource::capture(&pointer)?.write_in(&pointer, &bytes, &pin)?;
    } else {
        create_file_in(&pointer, &bytes, &pin)?;
    }
    // Both durable artifacts and the filesystem reference precede the atomic registry update.
    let transaction = db.0.unchecked_transaction()?;
    db.put("backup", &backup.id, backup)?;
    db.put(
        "backup-current",
        &current_key(&backup.drive, backup.kind),
        &backup.id,
    )?;
    db.remove("backup-publication", &backup.id)?;
    transaction.commit()?;
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
pub(super) fn check_manifest(backup: &Backup) -> Result<()> {
    if let Some(path) = &backup.manifest {
        validate_manifest(backup, path)?;
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
pub(crate) fn recover(db: &Store) -> Result<()> {
    let pending: Vec<Pending> = db.list("backup-publication", 0, 1000)?;
    for pending in pending {
        let id = pending.backup.id.clone();
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
            install(db, &parent, pending)
        })();
        if let Err(error) = outcome {
            let cause = error.recovery_cause();
            db.put(
                "recovery",
                &id,
                &crate::DriveRecovery {
                    id: id.clone(),
                    drive,
                    cause,
                    artifact,
                    message: error.to_string(),
                },
            )?;
        } else {
            db.remove("recovery", &id)?;
        }
    }
    Ok(())
}
