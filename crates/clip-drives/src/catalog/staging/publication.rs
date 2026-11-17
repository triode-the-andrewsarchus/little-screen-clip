//! Install a checksummed, closed database before atomically advertising its catalog.
use super::*;

pub(super) fn owned_file(root: &Root, name: &Path, expected: &StorageIdentity) -> Result<File> {
    let file = root.input(name)?;
    if StorageIdentity::pinned(&file)? != *expected {
        return Err("Catalog capture database was replaced; metadata retained".into());
    }
    Ok(file)
}
fn ready(intent: &Intent) -> Result<&Ready> {
    let ready = intent.ready.as_ref().ok_or("Capture is not sealed")?;
    if ready.catalog.id != intent.id || ready.catalog.drive != intent.drive {
        return Err("Catalog publication differs from its capture owner".into());
    }
    Ok(ready)
}
pub(super) fn published(db: &Store, intent: &Intent) -> Result<Option<Catalog>> {
    let ready = ready(intent)?;
    if let Some(published) = db.get::<Catalog>("catalog", &intent.id)? {
        if published.drive != ready.catalog.drive
            || published.evidence != ready.catalog.evidence
            || published.captured != ready.catalog.captured
            || published.limits != ready.catalog.limits
        {
            return Err("Another catalog owns this identifier; metadata retained".into());
        }
        return Ok(Some(published));
    }
    Ok(None)
}
fn install(parent: &Root, intent: &Intent) -> Result<File> {
    let final_name = PathBuf::from(format!("{}.sqlite", intent.id));
    let pin = parent.directory(Path::new(""), false)?;
    if parent.metadata(&final_name)?.is_none() {
        let stage = super::stage(parent, intent)?.ok_or("Sealed capture stage is unavailable")?;
        let _file = owned_file(&stage, Path::new(FILE), &intent.file)?;
        let stage_pin = stage.directory(Path::new(""), false)?;
        little_clip_core::file_source::rename_noreplace(
            &anchor(&stage_pin).join(FILE),
            &anchor(&pin).join(&final_name),
        )?;
    }
    let file = owned_file(parent, &final_name, &intent.file)?;
    File::open(anchor(&pin))?.sync_all()?;
    Ok(file)
}
fn verify(file: &File, ready: &Ready, cancel: &AtomicBool) -> Result<FileSource> {
    let budget = ready
        .catalog
        .limits
        .ok_or("Missing sealed catalog budgets")?
        .bytes;
    if file.metadata()?.len() != ready.bytes || ready.bytes > budget {
        return Err(
            "Sealed catalog size differs from its recorded bounded metadata; retained".into(),
        );
    }
    let source = FileSource::capture_file(file)?;
    if source.sha256(file, cancel, Duration::MAX)? != ready.checksum {
        return Err("Sealed catalog checksum differs; metadata retained".into());
    }
    source.check_file(file)?;
    file.sync_all()?;
    Ok(source)
}
pub(super) fn publish(
    db: &Store,
    state: &Root,
    intent: &Intent,
    cancel: &AtomicBool,
) -> Result<()> {
    let ready = ready(intent)?;
    let parent = super::parent(state, intent)?;
    if published(db, intent)?.is_some() {
        // Policy cleanup may already have changed published metadata. Never
        // replace it with the older sealed snapshot or delete its database.
        retirement::remove_stage(&parent, intent)?;
        return retirement::forget(db, &intent.id);
    }
    crate::cancelled(cancel)?;
    let file = install(&parent, intent)?;
    let source = verify(&file, ready, cancel)?;
    retirement::remove_stage(&parent, intent)?;
    state.validate()?;
    source.check_file(&owned_file(
        &parent,
        Path::new(&format!("{}.sqlite", intent.id)),
        &intent.file,
    )?)?;
    redaction::publish(db, &ready.catalog, &ready.exclusions)
}
