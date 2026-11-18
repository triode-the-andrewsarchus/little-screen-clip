//! Durable withdrawal and identity-bound cleanup of completed backup versions.
use super::*;
use serde::{Deserialize, Serialize};
#[cfg(test)]
mod tests;
mod tree;

const KIND: &str = "backup-prune";

#[derive(Clone, Serialize, Deserialize)]
struct Intent {
    version: u32,
    backup: Backup,
    parent: crate::StorageIdentity,
    quarantined: bool,
    paused: bool,
}

impl Intent {
    fn name(&self) -> String {
        format!(".pruning-{}", self.backup.id)
    }
    fn parent(&self) -> Result<Root> {
        if self.version != 1 {
            return Err("Unsupported backup retirement record".into());
        }
        artifact_name(&self.backup)?;
        if self.backup.manifest.is_some() && completed_manifest(&self.backup).is_none() {
            return Err("Retired manifest layout changed".into());
        }
        let root = Root::open(
            self.backup
                .path
                .parent()
                .ok_or("Missing retirement parent")?,
        )?;
        if root.storage_identity()? != self.parent {
            return Err("Retirement parent identity changed; contents retained".into());
        }
        Ok(root)
    }
}

/// A retained ordinary artifact and, for directories, its nonblocking reader lease.
struct Artifact {
    file: File,
    root: Option<Root>,
}
impl Artifact {
    fn open(parent: &Root, name: &std::ffi::OsStr, backup: &Backup) -> Result<Option<Self>> {
        let relative = Path::new(name);
        if parent.metadata(relative)?.is_none() {
            return Ok(None);
        }
        let artifact = if backup.kind == BackupKind::Files {
            // A Btrfs subvolume may report another st_dev without crossing a
            // mount. Keep NO_XDEV lookup and then require the recorded identity.
            let pin = parent.pin_child(&parent.directory(Path::new(""), false)?, name)?;
            if !pin.metadata()?.is_dir() {
                return Err("Backup directory became another file type".into());
            }
            let root = Root::open(&anchor(&pin))?;
            Self {
                file: provider_lease(&root, true)?,
                root: Some(root),
            }
        } else {
            let file = parent.input(relative)?;
            rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive)?;
            Self { file, root: None }
        };
        if Some(crate::StorageIdentity::pinned(&artifact.file)?) != backup.identity {
            return Err("Backup artifact identity changed; refusing retirement".into());
        }
        Ok(Some(artifact))
    }
}

/// Readers retain a shared directory lease; retirement needs exclusive ownership.
pub(crate) fn provider_lease(root: &Root, exclusive: bool) -> Result<File> {
    use rustix::fs::{FlockOperation, flock};
    let pin = root.directory(Path::new(""), false)?;
    let file = File::open(anchor(&pin))?;
    let operation = if exclusive {
        FlockOperation::NonBlockingLockExclusive
    } else {
        FlockOperation::NonBlockingLockShared
    };
    match flock(&file, operation) {
        Ok(()) => (),
        Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
            return Err("Backup provider is busy; release its open readers before pruning".into());
        }
        Err(error) => return Err(error.into()),
    }
    root.validate()?;
    Ok(file)
}

pub(super) fn check_readers(db: &Store, drive: &str) -> Result<()> {
    let mounts = crate::mounts::inventory()?;
    let snapshot = db.0.unchecked_transaction()?;
    db.visit::<crate::Catalog>("catalog", |catalog| {
        let source = format!("little-clip-catalog:{}", catalog.id);
        if catalog.drive == drive
            && (catalog.mount.is_some()
                || mounts
                    .iter()
                    .any(|mount| mount.source == Path::new(&source)))
        {
            return Err(
                "Unmount this drive's catalogs before pruning potential content providers".into(),
            );
        }
        Ok(())
    })?;
    snapshot.commit()?;
    Ok(())
}

fn protected(db: &Store, backup: &Backup) -> Result<bool> {
    let current: Option<String> = db.get_bounded(
        "backup-current",
        &publication::current_key(&backup.drive, backup.kind),
        128,
    )?;
    let publishing: bool = db.0.query_row(
        "SELECT EXISTS(SELECT 1 FROM entities WHERE kind IN ('backup-publication','image-control','image-loop') AND id=?1)",
        [&backup.id],
        |row| row.get(0),
    )?;
    Ok(current.as_ref() == Some(&backup.id)
        || publishing
        || backup.mount.is_some()
        || backup.loop_object.is_some())
}

fn withdraw(db: &Store, version: &Backup, parent: &Root, cancel: &AtomicBool) -> Result<Intent> {
    cancelled(cancel)?;
    artifact_name(version)?;
    publication::check_manifest(version, cancel)?;
    let intent = Intent {
        version: 1,
        backup: version.clone(),
        parent: parent.storage_identity()?,
        quarantined: false,
        paused: false,
    };
    intent.parent()?.validate()?;
    let transaction = db.0.unchecked_transaction()?;
    if protected(db, version)? {
        return Err(
            "A current, publishing, mounted or unresolved image-control backup cannot be retired"
                .into(),
        );
    }
    check_registered(db, version)?;
    cancelled(cancel)?;
    db.put(KIND, &version.id, &intent)?;
    db.remove("backup", &version.id)?;
    transaction.commit()?;
    Ok(intent)
}

fn check_registered(db: &Store, version: &Backup) -> Result<()> {
    let saved: Backup = db
        .get_bounded("backup", &version.id, 64 * 1024)?
        .ok_or("Backup was already withdrawn")?;
    if serde_json::to_value(saved)? != serde_json::to_value(version)?
        || db
            .get_bounded::<Intent>(KIND, &version.id, 64 * 1024)?
            .is_some()
    {
        return Err("Backup metadata changed before retirement".into());
    }
    Ok(())
}

fn artifact_name(backup: &Backup) -> Result<&std::ffi::OsStr> {
    uuid::Uuid::parse_str(&backup.id).map_err(|_| "Invalid backup identifier")?;
    let name = backup.path.file_name().ok_or("Missing backup name")?;
    let suffix = if backup.kind == BackupKind::Files {
        format!("-{}", backup.id)
    } else {
        format!("-{}.img", backup.id)
    };
    let prefix = name
        .as_encoded_bytes()
        .strip_suffix(suffix.as_bytes())
        .ok_or("Backup path does not match its recorded identifier")?;
    if prefix.is_empty() || !prefix.iter().all(u8::is_ascii_digit) {
        return Err("Invalid completed backup name".into());
    }
    Ok(name)
}

fn quarantine(
    db: &Store,
    intent: &mut Intent,
    parent: &Root,
    artifact: Artifact,
) -> Result<Artifact> {
    let pin = parent.directory(Path::new(""), false)?;
    let original = intent
        .backup
        .path
        .file_name()
        .ok_or("Missing backup name")?;
    little_clip_core::file_source::rename_noreplace(
        &anchor(&pin).join(original),
        &anchor(&pin).join(intent.name()),
    )?;
    intent.quarantined = true;
    parent.validate()?;
    File::open(anchor(&pin))?.sync_all()?;
    let root = artifact
        .root
        .as_ref()
        .map(|_| Root::open(&anchor(&artifact.file)))
        .transpose()?;
    if fs::read_link(anchor(&artifact.file))? != parent.path.join(intent.name()) {
        return Err("Quarantined backup identity changed".into());
    }
    db.put(KIND, &intent.backup.id, intent)?;
    Ok(Artifact {
        file: artifact.file,
        root,
    })
}

fn retained(
    db: &Store,
    intent: &mut Intent,
    parent: &Root,
    cancel: &AtomicBool,
) -> Result<Option<Artifact>> {
    let name = intent.name();
    if let Some(artifact) = Artifact::open(parent, name.as_ref(), &intent.backup)? {
        if !intent.quarantined {
            intent.quarantined = true;
            db.put(KIND, &intent.backup.id, intent)?;
        }
        return Ok(Some(artifact));
    }
    let original = intent
        .backup
        .path
        .file_name()
        .ok_or("Missing backup name")?;
    let Some(artifact) = Artifact::open(parent, original, &intent.backup)? else {
        return Ok(None);
    };
    cancelled(cancel)?;
    quarantine(db, intent, parent, artifact).map(Some)
}

fn remove_artifact(
    parent: &Root,
    intent: &Intent,
    artifact: Artifact,
    cancel: &AtomicBool,
) -> Result<()> {
    cancelled(cancel)?;
    if let Some(root) = &artifact.root {
        remove_tree(parent, intent, root, cancel)?;
    } else {
        let (_pin, path) = parent.parent(Path::new(&intent.name()), false)?;
        if Some(crate::StorageIdentity::pinned(
            &parent.input(Path::new(&intent.name()))?,
        )?) != intent.backup.identity
        {
            return Err("Retired image identity changed before deletion".into());
        }
        fs::remove_file(path)?;
    }
    Ok(())
}

fn check_withdrawn(db: &Store, intent: &Intent) -> Result<()> {
    if protected(db, &intent.backup)?
        || db
            .get_bounded::<Backup>("backup", &intent.backup.id, 64 * 1024)?
            .is_some()
    {
        return Err("A published or current backup cannot be cleaned as retired data".into());
    }
    Ok(())
}

fn apply(
    db: &Store,
    intent: &mut Intent,
    cancel: &AtomicBool,
    initial: Option<Artifact>,
) -> Result<()> {
    cancelled(cancel)?;
    check_withdrawn(db, intent)?;
    check_readers(db, &intent.backup.drive)?;
    let parent = intent.parent()?;
    let artifact = match initial {
        Some(artifact) => Some(quarantine(db, intent, &parent, artifact)?),
        None => retained(db, intent, &parent, cancel)?,
    };
    if let Some(artifact) = artifact {
        publication::check_manifest(&intent.backup, cancel)?;
        remove_artifact(&parent, intent, artifact, cancel)?;
    }
    finish(db, intent, &parent, cancel)
}

fn finish(db: &Store, intent: &Intent, parent: &Root, cancel: &AtomicBool) -> Result<()> {
    if let Some(path) = &intent.backup.manifest {
        let name = path.file_name().ok_or("Missing retired manifest name")?;
        if parent.metadata(Path::new(name))?.is_some() {
            publication::remove_manifest(parent, &intent.backup, cancel)?;
        }
    }
    parent.validate()?;
    File::open(anchor(&parent.directory(Path::new(""), false)?))?.sync_all()?;
    let transaction = db.0.unchecked_transaction()?;
    remember_destination(
        db,
        parent
            .path
            .parent()
            .ok_or("Missing retired backup destination")?,
    )?;
    db.remove(KIND, &intent.backup.id)?;
    db.remove("recovery", &intent.backup.id)?;
    transaction.commit()?;
    Ok(())
}

fn attempt(
    db: &Store,
    intent: &mut Intent,
    cancel: &AtomicBool,
    initial: Option<Artifact>,
    pause_on_cancel: bool,
) -> Result<()> {
    if let Err(error) = apply(db, intent, cancel, initial) {
        intent.paused = pause_on_cancel && cancel.load(std::sync::atomic::Ordering::Relaxed);
        let transaction = db.0.unchecked_transaction()?;
        db.put(KIND, &intent.backup.id, intent)?;
        db.put(
            "recovery",
            &intent.backup.id,
            &crate::DriveRecovery {
                id: intent.backup.id.clone(),
                drive: intent.backup.drive.clone(),
                cause: error.recovery_cause(),
                artifact: if intent.quarantined {
                    intent
                        .backup
                        .path
                        .parent()
                        .ok_or("Missing retirement parent")?
                        .join(intent.name())
                } else {
                    intent.backup.path.clone()
                },
                message: format!("Backup withdrawn; retirement cleanup retained: {error}"),
            },
        )?;
        transaction.commit()?;
        return Err(error);
    }
    Ok(())
}

pub(super) fn resume_one(
    db: &Store,
    id: &str,
    drive: Option<&str>,
    cancel: &AtomicBool,
) -> Result<bool> {
    let Some(mut intent) = db.get_bounded::<Intent>(KIND, id, 64 * 1024)? else {
        return Ok(false);
    };
    if intent.backup.id != id {
        return Err("Retirement record identifier changed".into());
    }
    if drive.is_some_and(|drive| drive != intent.backup.drive) || (drive.is_none() && intent.paused)
    {
        return Ok(false);
    }
    if intent.paused {
        intent.paused = false;
        db.put(KIND, id, &intent)?;
    }
    attempt(db, &mut intent, cancel, None, drive.is_some())?;
    Ok(true)
}

fn resume(db: &Store, drive: Option<&str>, cancel: &AtomicBool) -> Result<usize> {
    let mut after = String::new();
    let mut removed = 0;
    loop {
        cancelled(cancel)?;
        let page = db.ids(KIND, &after, 128)?;
        let Some(last) = page.last() else {
            return Ok(removed);
        };
        after = last.clone();
        for id in page {
            cancelled(cancel)?;
            match resume_one(db, &id, drive, cancel) {
                Ok(done) => removed += usize::from(done),
                Err(error) if drive.is_some() => {
                    return Err(format!(
                        "Pruned {removed} retiring versions before stopping: {error}"
                    )
                    .into());
                }
                Err(error) => tracing::warn!("Backup retirement recovery retained {id}: {error}"),
            }
        }
    }
}

pub(super) fn recover(db: &Store, cancel: &AtomicBool) -> Result<()> {
    resume(db, None, cancel).map(|_| ())
}

pub(crate) fn prune(db: &Store, drive: &str, keep: usize, cancel: &AtomicBool) -> Result<String> {
    cancelled(cancel)?;
    if keep == 0 {
        return Err("Retain at least one completed backup version".into());
    }
    check_readers(db, drive)?;
    let mut removed = resume(db, Some(drive), cancel)?;
    let versions = planned_versions(db, drive)?;
    let mut files = 0;
    let mut images = 0;
    for version in versions {
        let count = if version.kind == BackupKind::Files {
            &mut files
        } else {
            &mut images
        };
        *count += 1;
        if *count <= keep || protected(db, &version)? {
            continue;
        }
        let result = prune_version(db, &version, cancel);
        if let Err(error) = result {
            return Err(
                format!("Pruned {removed} completed versions before stopping: {error}").into(),
            );
        }
        removed += 1;
    }
    Ok(format!("Pruned {removed} old completed backup versions"))
}

fn prune_version(db: &Store, version: &Backup, cancel: &AtomicBool) -> Result<()> {
    cancelled(cancel)?;
    let parent = Root::open(version.path.parent().ok_or("Missing backup parent")?)?;
    let _admission = parent.writable_directory()?;
    let artifact = Artifact::open(
        &parent,
        version.path.file_name().ok_or("Missing backup name")?,
        version,
    )?
    .ok_or("Backup artifact is unavailable")?;
    if let Some(root) = &artifact.root {
        tree::validate(root, cancel)?;
    } else {
        verify_image(version, &artifact.file)?;
    }
    let mut intent = withdraw(db, version, &parent, cancel)?;
    // Keep the exclusive lease across withdrawal and the first cleanup attempt.
    attempt(db, &mut intent, cancel, Some(artifact), true)
}

fn planned_versions(db: &Store, drive: &str) -> Result<Vec<Backup>> {
    let mut versions = Vec::new();
    let snapshot = db.0.unchecked_transaction()?;
    db.visit::<Backup>("backup", |version| {
        if version.drive == drive {
            versions.push(version);
        }
        Ok(())
    })?;
    snapshot.commit()?;
    // Visit walks ascending rowids; equal wall-clock dates retain the newest
    // registration first, matching the existing paginated listing semantics.
    versions.reverse();
    versions.sort_by_key(|version| std::cmp::Reverse(version.created));
    Ok(versions)
}

fn remove_tree(parent: &Root, intent: &Intent, root: &Root, cancel: &AtomicBool) -> Result<()> {
    if intent.backup.incremental && btrfs(root)? {
        let pin = parent.directory(Path::new(""), false)?;
        if command(&["subvolume", "delete"], &pin, &[intent.name().as_ref()]).is_ok() {
            return Ok(());
        }
        if parent.metadata(Path::new(&intent.name()))?.is_none() {
            return Ok(());
        }
        // Local policy may deny the deletion ioctl to an ordinary owner.
        // The withdrawn owned tree can still be removed through normal VFS calls.
        root.validate()?;
        subvolume_read_only(root, false)?;
    }
    tree::remove(root, cancel)?;
    let (_pin, path) = parent.parent(Path::new(&intent.name()), false)?;
    root.validate()?;
    fs::remove_dir(path)?;
    Ok(())
}
