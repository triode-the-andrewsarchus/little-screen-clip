//! Enrollment checks keep managed storage out of synchronization feedback loops.
use super::*;
impl Owner {
    pub(super) fn validate_pair_target(&self, source: &Root, local: &Root) -> Result<()> {
        if source.overlap(local) {
            return Err("Sync roots overlap".into());
        }
        let mut managed = vec![self.state.clone()];
        if let Some(path) = &self.config.backup_directory {
            managed.push(path.clone());
        }
        managed.extend(
            self.db
                .list::<RamSession>("ram", 0, 1000)?
                .into_iter()
                .map(|s| s.path),
        );
        managed.extend(
            self.db
                .list::<Catalog>("catalog", 0, 1000)?
                .into_iter()
                .filter_map(|s| s.mount),
        );
        for path in managed {
            if let Ok(root) = Root::open(&path) {
                if source.overlap(&root) || local.overlap(&root) {
                    return Err("Sync roots overlap managed backups, RAM or catalog storage".into());
                }
            } else if local.path.starts_with(&path) || path.starts_with(&local.path) {
                return Err("Local folder overlaps managed storage".into());
            }
        }
        let pairs = self.db.list::<SyncPair>("pair", 0, 1000)?;
        if pairs.len() >= 256 {
            return Err(
                "Pause or remove an existing enrollment before adding more than 256 sync pairs"
                    .into(),
            );
        }
        for pair in pairs {
            if let Ok(root) = Root::open(&pair.local)
                && local.overlap(&root)
            {
                return Err("This local folder overlaps an existing sync enrollment".into());
            }
        }
        Ok(())
    }
    pub(super) fn validate_backup_directory(&self, target: &Root) -> Result<()> {
        if target.overlap(&Root::open(&self.state)?) {
            return Err("Backup directory overlaps drive-service state".into());
        }
        for pair in self.db.list::<SyncPair>("pair", 0, 1000)? {
            if let Ok(root) = Root::open(&pair.local)
                && root.overlap(target)
            {
                return Err("Backup directory overlaps a paired local folder or RAM copy".into());
            }
        }
        for catalog in self.db.list::<Catalog>("catalog", 0, 1000)? {
            if let Some(path) = catalog.mount
                && let Ok(root) = Root::open(&path)
                && root.overlap(target)
            {
                return Err("Backup directory overlaps a mounted catalog".into());
            }
        }
        Ok(())
    }
    pub(super) fn ram_size_check(
        &self,
        source: &Root,
        stats: &rustix::fs::StatVfs,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let id = "ram-preflight";
        let coverage = scan::scan(
            &self.db,
            source,
            id,
            &scan::Limits {
                hashes: false,
                entries: stats.f_favail.saturating_sub(1024),
                ..Default::default()
            },
            cancel,
            |_, _| {},
        )?;
        if !coverage.complete {
            return Err(
                "RAM preflight could not inventory every entry within available inode headroom"
                    .into(),
            );
        }
        let bytes: u64 = self.db.0.query_row("SELECT coalesce(sum(json_extract(json,'$.size')),0) FROM entries WHERE scan=?1 AND json_extract(json,'$.kind')='file'", [id], |r| r.get(0))?;
        self.db.clear_scan(id)?;
        if bytes.saturating_add(64 * 1024 * 1024) > stats.f_bavail.saturating_mul(stats.f_frsize)
            || bytes.saturating_add(1024 * 1024 * 1024) > available_memory()?
        {
            return Err(
                "Expanded file contents exceed safe tmpfs or available RAM headroom".into(),
            );
        }
        Ok(())
    }
}
pub(super) fn validate_config(config: &DriveConfig) -> Result<()> {
    if !config.ram_directory.is_absolute()
        || config
            .backup_directory
            .as_ref()
            .is_some_and(|p| !p.is_absolute())
    {
        return Err("Drive storage directories must be absolute paths".into());
    }
    if config.catalog_max_bytes < 128 * 1024
        || config.catalog_max_depth == 0
        || config.catalog_timeout_seconds == 0
        || config.sync_interval_seconds == 0
    {
        return Err("Catalogs require at least 128 KiB, a nonzero depth/time budget and nonzero sync interval".into());
    }
    if config.backup_keep_versions == Some(0) {
        return Err("Backup retention must keep at least one completed version".into());
    }
    Ok(())
}
