//! Optional reuse of completed, verified fingerprints; never hashes source payloads.
use super::*;
use crate::cancelled;
mod history;
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

struct Reuse<'a> {
    catalog: &'a Store,
    source: &'a Root,
    cancel: &'a AtomicBool,
    started: Instant,
    timeout: Duration,
}
impl Reuse<'_> {
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
    fn merge(&self, manifest: &Store, scan: &str, missing: u64) -> Result<u64> {
        self.check_source(manifest)?;
        let mut query = manifest.0.prepare("SELECT CASE WHEN length(CAST(json AS BLOB))<=65536 THEN json END FROM entries WHERE scan=?1 ORDER BY path")?;
        let rows = query.query_map([scan], |row| row.get::<_, Option<String>>(0))?;
        let mut imported = 0;
        for row in rows {
            cancelled(self.cancel)?;
            if self.remaining().is_zero() {
                break;
            }
            let saved: Entry =
                serde_json::from_str(&row?.ok_or("Oversized manifest fingerprint row")?)?;
            if saved.kind == "file"
                && let Some(entry) = scan::entry(self.catalog, "catalog", &saved.path)?
            {
                imported += u64::from(self.promote(saved, Path::new(""), None, entry)?);
                if imported == missing {
                    break;
                }
            }
        }
        Ok(imported)
    }
    fn check_source(&self, manifest: &Store) -> Result<()> {
        if manifest.get_bounded::<bool>("collection", "source_queried_change_tracking", 128)?
            != Some(true)
        {
            return Err("Backup has no queried-change tracking provenance".into());
        }
        let captured: Entry = manifest
            .get_bounded("source", "root", 64 * 1024)?
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
        entry: Entry,
    ) -> Result<bool> {
        let relative = entry
            .path
            .strip_prefix(prefix)
            .map_err(|error| error.to_string())?;
        let Some(saved) = scan::entry(manifest, scan, relative)? else {
            return Ok(false);
        };
        self.promote(saved, prefix, generation, entry)
    }
    fn promote(
        &self,
        saved: Entry,
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
