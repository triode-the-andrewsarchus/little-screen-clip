//! Optional reuse of completed, verified fingerprints; never hashes source payloads.
use super::*;
use crate::cancelled;
use std::{os::unix::fs::MetadataExt, time::Instant};

pub(super) fn from_backup(
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
    let Ok(Some((version, path))) = current(state, drive) else {
        return Ok(());
    };
    let transaction = catalog.0.unchecked_transaction()?;
    let result = backup::read_manifest(
        &version,
        &path,
        cancel,
        config.catalog_max_bytes,
        reuse.remaining(),
        |manifest| reuse.merge(manifest, &format!("backup:{}", version.id)),
    );
    match result {
        Ok(()) => transaction.commit()?,
        Err(_) => {
            // Optional enrichment is atomic: unavailable/corrupt/oversized input
            // or database headroom failure cannot publish unverified fingerprints.
            transaction.rollback()?;
            cancelled(cancel)?;
        }
    }
    Ok(())
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
            self.import(manifest, scan, entry?)?;
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
    fn import(&self, manifest: &Store, scan: &str, mut entry: Entry) -> Result<()> {
        if entry.kind != "file"
            || !entry
                .changed
                .is_some_and(|(_, nanos)| nanos > 0 && nanos < 1_000_000_000)
        {
            return Ok(());
        }
        let Some(saved) = scan::entry(manifest, scan, &entry.path)? else {
            return Ok(());
        };
        if entry.same_revision(&saved)
            && let Some(hash) = saved.hash.filter(|hash| {
                hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            entry.hash = Some(hash.to_ascii_lowercase());
            scan::put(self.catalog, "catalog", &entry)?;
        }
        Ok(())
    }
}
