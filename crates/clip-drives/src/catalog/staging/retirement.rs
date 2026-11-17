//! Retire only identity-bound files in a private staging directory, never a payload tree.
use super::*;

fn inventory(
    stage: &Root,
    pin: &File,
    intent: &Intent,
) -> Result<Vec<(OsString, File, StorageIdentity)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(anchor(pin))?.take(5) {
        let name = entry?.file_name();
        if ![
            FILE,
            "metadata.sqlite-journal",
            "metadata.sqlite-wal",
            "metadata.sqlite-shm",
        ]
        .iter()
        .any(|allowed| name == OsStr::new(allowed))
        {
            return Err("Unexpected capture-stage entry; metadata retained".into());
        }
        let file = stage.input(Path::new(&name))?;
        let identity = StorageIdentity::pinned(&file)?;
        if name == OsStr::new(FILE) && identity != intent.file {
            return Err("Capture-stage database was replaced; metadata retained".into());
        }
        files.push((name, file, identity));
    }
    Ok(files)
}
pub(super) fn remove_stage(parent: &Root, intent: &Intent) -> Result<()> {
    let Some(stage) = super::stage(parent, intent)? else {
        return Ok(());
    };
    let pin = stage.directory(Path::new(""), false)?;
    let files = inventory(&stage, &pin, intent)?;
    remove_files(&stage, &pin, files)?;
    stage.validate()?;
    let parent_pin = parent.directory(Path::new(""), false)?;
    fs::remove_dir(anchor(&parent_pin).join(&intent.stage))?;
    File::open(anchor(&parent_pin))?.sync_all()?;
    Ok(())
}
fn remove_files(
    stage: &Root,
    pin: &File,
    files: Vec<(OsString, File, StorageIdentity)>,
) -> Result<()> {
    // Validate the complete constant-size namespace before deleting any entry.
    for (name, _held, expected) in files {
        let current = stage.input(Path::new(&name))?;
        if StorageIdentity::pinned(&current)? != expected {
            return Err("Capture-stage entry changed during cleanup; metadata retained".into());
        }
        stage.validate()?;
        fs::remove_file(anchor(pin).join(name))?;
    }
    File::open(anchor(pin))?.sync_all()?;
    Ok(())
}
pub(super) fn forget(db: &Store, id: &str) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    db.remove(KIND, id)?;
    db.remove("recovery", id)?;
    transaction.commit()?;
    Ok(())
}
pub(super) fn retire(db: &Store, state: &Root, intent: &Intent) -> Result<()> {
    if db.get::<Catalog>("catalog", &intent.id)?.is_some() {
        return Err("Published catalog protects its metadata; capture retained".into());
    }
    remove_stage(&super::parent(state, intent)?, intent)?;
    forget(db, &intent.id)
}
