//! Confirmed common effects share a transaction with the comparison baseline.
use super::*;
use crate::{DriveOperation, SyncCompletion};

pub(super) fn begin(db: &Store, id: Option<&str>) -> Result<()> {
    if let Some(id) = id {
        let mut operation = operation(db, id)?;
        if operation.sync_completion.is_none() {
            operation.sync_completion = Some(SyncCompletion::default());
            db.put("operation", id, &operation)?;
        }
    }
    Ok(())
}

fn operation(db: &Store, id: &str) -> Result<DriveOperation> {
    let operation = db
        .get::<DriveOperation>("operation", id)?
        .ok_or("Reconciliation operation disappeared before its completion receipt")?;
    if operation.finished.is_some() {
        return Err("Reconciliation operation already finished; refusing new effects".into());
    }
    Ok(operation)
}

impl Reconcile<'_> {
    pub(super) fn commit(
        &self,
        path: &Path,
        hash: Option<&str>,
        effects: impl IntoIterator<Item = (bool, Option<u64>)>,
    ) -> Result<()> {
        let transaction = self.db.0.unchecked_transaction()?;
        save_baseline(self.db, &self.pair.id, path, hash)?;
        if let Some(id) = self.operation {
            let mut operation = operation(self.db, id)?;
            let summary = operation.sync_completion.get_or_insert_default();
            for (portable_target, installed) in effects {
                match installed {
                    Some(bytes) => {
                        summary.installed = summary.installed.saturating_add(1);
                        summary.bytes = summary.bytes.saturating_add(bytes);
                    }
                    None => summary.removed = summary.removed.saturating_add(1),
                }
                self.db.0.execute(
                "INSERT INTO sync_receipts(operation,pair,path,target,hash,bytes) VALUES(?1,?2,?3,?4,?5,?6)",
                params![id, self.pair.id, path.as_os_str().as_bytes(),
                    if portable_target { "portable" } else { "local" }, hash, installed],
                )?;
            }
            self.db.put("operation", id, &operation)?;
        }
        transaction.commit()?;
        Ok(())
    }
}
