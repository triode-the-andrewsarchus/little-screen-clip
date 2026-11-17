//! Reader admission for retiring completed backup versions.
use super::*;

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
