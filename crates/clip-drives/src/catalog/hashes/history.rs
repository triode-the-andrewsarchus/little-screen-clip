//! Bounded, current-first discovery of completed file-backup fingerprints.
use super::*;

impl Reuse<'_> {
    fn candidate(
        &self,
        version: &Backup,
        drive: &crate::Drive,
        bytes: &mut u64,
        missing: &mut Option<u64>,
    ) -> Result<()> {
        cancelled(self.cancel)?;
        if self.remaining().is_zero() || *bytes == 0 {
            return Ok(());
        }
        if version.kind != crate::BackupKind::Files
            || version.drive != drive.id
            || version.evidence != drive.evidence
        {
            return Ok(());
        }
        let Some(path) = backup::completed_manifest(version) else {
            return Ok(());
        };
        let pending = match *missing {
            Some(count) => count,
            None => {
                let count = self.catalog.0.query_row("SELECT count(*) FROM entries WHERE scan='catalog' AND json_extract(json,'$.kind')='file' AND json_extract(json,'$.hash') IS NULL AND json_extract(json,'$.changed[1]')>0 AND json_extract(json,'$.changed[1]')<1000000000", [], |row| row.get::<_, u64>(0))?;
                *missing = Some(count);
                count
            }
        };
        if pending == 0 {
            return Ok(());
        }
        let transaction = self.catalog.0.unchecked_transaction()?;
        let mut imported = 0;
        let result =
            backup::read_manifest(version, path, self.cancel, bytes, self.remaining(), |db| {
                imported = self.merge(db, &format!("backup:{}", version.id), pending)?;
                Ok(())
            });
        let accepted = result.is_ok();
        self.finish(transaction, result)?;
        if accepted {
            *missing = Some(pending.saturating_sub(imported));
        }
        Ok(())
    }

    pub(super) fn backup(&self, state: &Store, drive: &crate::Drive, mut bytes: u64) -> Result<()> {
        if self.remaining().is_zero() {
            return Ok(());
        }
        let snapshot = state.0.unchecked_transaction()?;
        let mut missing = None;
        let current = backup::current_version(state, &drive.id, crate::BackupKind::Files)
            .ok()
            .flatten();
        if let Some(version) = &current {
            self.candidate(version, drive, &mut bytes, &mut missing)?;
        }
        self.history(
            state,
            drive,
            current.as_ref().map(|version| version.id.as_str()),
            &mut bytes,
            &mut missing,
        )?;
        snapshot.commit()?;
        Ok(())
    }
    fn history(
        &self,
        state: &Store,
        drive: &crate::Drive,
        current: Option<&str>,
        bytes: &mut u64,
        missing: &mut Option<u64>,
    ) -> Result<()> {
        // The kind-only index carries rowid order: no history-sized sort or
        // unrelated operation records before the first bounded row is delivered.
        let Ok(mut query) = state.0.prepare("SELECT CASE WHEN length(id)<=128 THEN id END, CASE WHEN length(CAST(json AS BLOB))<=65536 THEN json END FROM entities INDEXED BY entity_kind WHERE kind='backup' ORDER BY rowid DESC") else {
            // Standalone callers may still open pre-index state without daemon
            // startup migration. Retain the current-manifest result in that case.
            return Ok(());
        };
        let mut rows = query.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })?;
        while !self.remaining().is_zero() && *bytes > 0 && *missing != Some(0) {
            cancelled(self.cancel)?;
            let Some(row) = rows.next() else {
                break;
            };
            let (Some(id), Some(json)) = row? else {
                continue;
            };
            if current == Some(id.as_str()) {
                continue;
            }
            if let Ok(version) = serde_json::from_str::<Backup>(&json)
                && version.id == id
            {
                self.candidate(&version, drive, bytes, missing)?;
            }
        }
        drop(rows);
        drop(query);
        Ok(())
    }
}
