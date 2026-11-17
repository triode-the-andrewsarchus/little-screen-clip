//! Optional reuse of completed, verified fingerprints; never hashes source payloads.
use super::*;
use crate::cancelled;
mod sync;
use std::{os::unix::fs::MetadataExt, time::Instant};

pub(super) fn enrich(
    state: &Store,
    catalog: &Store,
    drive: &crate::Drive,
    source: &Root,
    config: &DriveConfig,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<()> {
    let reuse = Reuse {
        catalog,
        source,
        cancel,
        started,
        timeout: Duration::from_secs(config.catalog_timeout_seconds),
    };
    if !source.queried_change_tracking().unwrap_or(false) || reuse.remaining().is_zero() {
        return Ok(());
    }
    reuse.backup(state, drive, config.catalog_max_bytes)?;
    reuse.sync(state, drive)
}

fn current(state: &Store, drive: &crate::Drive) -> Result<Option<(Backup, PathBuf)>> {
    // A current-version lookup is bounded; it cannot scan unrelated history.
    let Some(version) = backup::current_version(state, &drive.id, crate::BackupKind::Files)? else {
        return Ok(None);
    };
    let Some(path) = version.manifest.clone() else {
        return Ok(None);
    };
    if version.drive != drive.id
        || version.evidence != drive.evidence
        || path.parent() != version.path.parent()
        || path.file_name() != Some(OsStr::new(&format!("{}.manifest.sqlite", version.id)))
    {
        return Ok(None);
    }
    Ok(Some((version, path)))
}

struct Reuse<'a> {
    catalog: &'a Store,
    source: &'a Root,
    cancel: &'a AtomicBool,
    started: Instant,
    timeout: Duration,
}
impl Reuse<'_> {
    fn backup(&self, state: &Store, drive: &crate::Drive, bytes: u64) -> Result<()> {
        let Ok(Some((version, path))) = current(state, drive) else {
            return Ok(());
        };
        let transaction = self.catalog.0.unchecked_transaction()?;
        let result = backup::read_manifest(
            &version,
            &path,
            self.cancel,
            bytes,
            self.remaining(),
            |manifest| self.merge(manifest, &format!("backup:{}", version.id)),
        );
        self.finish(transaction, result)
    }
    fn finish(&self, transaction: rusqlite::Transaction<'_>, result: Result<()>) -> Result<()> {
        match result {
            Ok(()) => transaction.commit()?,
            Err(_) => {
                // Enrichment is optional and atomic, including quota failures.
                transaction.rollback()?;
                cancelled(self.cancel)?;
            }
        }
        Ok(())
    }
    fn remaining(&self) -> Duration {
        self.timeout.saturating_sub(self.started.elapsed())
    }
    fn merge(&self, manifest: &Store, scan: &str) -> Result<()> {
        self.check_source(manifest)?;
        for entry in scan::Cursor::new(self.catalog, "catalog") {
            cancelled(self.cancel)?;
            if self.remaining().is_zero() {
                break;
            }
            self.import(manifest, scan, Path::new(""), None, entry?)?;
        }
        Ok(())
    }
    fn check_source(&self, manifest: &Store) -> Result<()> {
        if manifest.get::<bool>("collection", "source_queried_change_tracking")? != Some(true) {
            return Err("Backup has no queried-change tracking provenance".into());
        }
        let captured: Entry = manifest
            .get("source", "root")?
            .ok_or("Missing backup source root")?;
        let current = self
            .source
            .metadata(Path::new(""))?
            .ok_or("Missing source root")?;
        if captured.kind != "directory"
            || !captured.path.as_os_str().is_empty()
            || captured.device != current.dev()
            || captured.inode != current.ino()
        {
            return Err("Backup fingerprints belong to a different source root".into());
        }
        Ok(())
    }
    fn import(
        &self,
        manifest: &Store,
        scan: &str,
        prefix: &Path,
        generation: Option<uuid::Uuid>,
        mut entry: Entry,
    ) -> Result<bool> {
        if entry.hash.is_some()
            || entry.kind != "file"
            || !entry
                .changed
                .is_some_and(|(_, nanos)| nanos > 0 && nanos < 1_000_000_000)
        {
            return Ok(false);
        }
        let relative = entry
            .path
            .strip_prefix(prefix)
            .map_err(|error| error.to_string())?;
        // Bound optional row decoding even if the registry contains oversized data.
        use rusqlite::OptionalExtension;
        let row: Option<String> = manifest.0.query_row(
            "SELECT json FROM entries WHERE scan=?1 AND path=?2 AND length(CAST(json AS BLOB))<=65536",
            rusqlite::params![scan, relative.as_os_str().as_bytes()], |row| row.get(0),
        ).optional()?;
        let Some(saved) = row
            .map(|json| serde_json::from_str::<Entry>(&json))
            .transpose()?
        else {
            return Ok(false);
        };
        if generation.is_none_or(|generation| saved.hash_generation == Some(generation))
            && saved.path == relative
            && entry.same_revision(&saved)
            && let Some(hash) = saved.hash.filter(|hash| {
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            entry.hash = Some(hash.to_ascii_lowercase());
            scan::put(self.catalog, "catalog", &entry)?;
            return Ok(true);
        }
        Ok(false)
    }
}
