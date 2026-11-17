//! Enrollment checks keep managed storage out of synchronization feedback loops.
use super::*;
impl Owner {
    pub(super) fn backup_destination(&self, source: Option<&Root>) -> Result<Arc<Root>> {
        let path = self
            .config
            .backup_directory
            .as_ref()
            .ok_or("Choose a DriveBackupDir before starting a backup")?;
        let destination = Arc::new(Root::create(path)?);
        self.validate_backup_directory(&destination)?;
        if source.is_some_and(|source| source.overlap(&destination)) {
            return Err("Backup destination overlaps the source partition".into());
        }
        Ok(destination)
    }
    /// Every worker uses the same captured-root scheduling policy, including unmount.
    pub(super) fn claims_busy(&self, claims: &[crate::root::Scope]) -> bool {
        let overlaps = |scope: &crate::root::Scope| claims.iter().any(|claim| claim.overlap(scope));
        self.partition
            .as_ref()
            .is_some_and(|job| job.claims.iter().any(&overlaps))
            || self
                .ram_setup
                .as_ref()
                .is_some_and(|job| overlaps(&job.source.scope()))
            || self
                .sync_jobs
                .values()
                .any(|job| job.plan.scopes().iter().any(&overlaps))
            || self
                .background
                .values()
                .any(|job| job.scopes().any(|scope| overlaps(&scope)))
    }
    pub(super) fn validate_pair_target(&self, source: &Root, local: &Root) -> Result<()> {
        self.validate_sync_storage(source, local, None)?;
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
    pub(super) fn validate_sync_storage(
        &self,
        source: &Root,
        local: &Root,
        own_ram: Option<&str>,
    ) -> Result<()> {
        self.validate_ram_backing_target(source)?;
        if own_ram.is_none() {
            self.validate_ram_backing_target(local)?;
        }
        if source.overlap(local) {
            return Err("Sync roots overlap after a directory or mount change; pause and re-enroll the pair".into());
        }
        let mut managed = vec![(self.state.clone(), false)];
        managed.extend(
            preflight::managed_backup_paths(&self.db)?
                .into_iter()
                .map(|path| (path, false)),
        );
        managed.extend(
            self.config
                .backup_directory
                .clone()
                .map(|path| (path, false)),
        );
        managed.extend(
            self.db
                .list::<RamSession>("ram", 0, 1000)?
                .into_iter()
                .map(|session| (session.path, own_ram == Some(session.id.as_str()))),
        );
        managed.extend(
            self.db
                .list::<Catalog>("catalog", 0, 1000)?
                .into_iter()
                .filter_map(|catalog| catalog.mount)
                .map(|path| (path, false)),
        );
        for (path, own_view) in managed {
            let overlaps = if let Ok(root) = Root::open(&path) {
                source.overlap(&root) || (!own_view && local.overlap(&root))
            } else {
                source.path.starts_with(&path)
                    || path.starts_with(&source.path)
                    || (!own_view
                        && (local.path.starts_with(&path) || path.starts_with(&local.path)))
            };
            if overlaps {
                return Err("Sync roots overlap managed backups, RAM or catalog storage".into());
            }
        }
        Ok(())
    }
    pub(super) fn validate_ram_backing_target(&self, target: &Root) -> Result<()> {
        validate_ram_backing_storage(&self.db, target)
    }
    pub(super) fn validate_backup_directory(&self, target: &Root) -> Result<()> {
        validate_backup_storage(&self.db, &self.state, target)
    }
}
fn validate_ram_backing_storage(db: &Store, target: &Root) -> Result<()> {
    for session in db.list::<RamSession>("ram", 0, 1000)? {
        if let Some(backing) = session.backing {
            if let Ok(root) = Root::open(&backing.directory) {
                if target.overlap(&root) {
                    return Err("Destination overlaps private RAM backing storage".into());
                }
            } else if target.path.starts_with(&backing.directory)
                || backing.directory.starts_with(&target.path)
            {
                return Err("Destination overlaps unavailable RAM backing storage".into());
            }
        }
    }
    Ok(())
}
pub(super) fn validate_backup_storage(db: &Store, state: &Path, target: &Root) -> Result<()> {
    validate_ram_backing_storage(db, target)?;
    if target.overlap(&Root::open(state)?) {
        return Err("Backup directory overlaps drive-service state".into());
    }
    for pair in db.list::<SyncPair>("pair", 0, 1000)? {
        if let Ok(root) = Root::open(&pair.local)
            && root.overlap(target)
        {
            return Err("Backup directory overlaps a paired local folder or RAM copy".into());
        }
    }
    for catalog in db.list::<Catalog>("catalog", 0, 1000)? {
        if let Some(path) = catalog.mount
            && let Ok(root) = Root::open(&path)
            && root.overlap(target)
        {
            return Err("Backup directory overlaps a mounted catalog".into());
        }
    }
    Ok(())
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
    crate::CatalogLimits::from(config).validate()?;
    if !(1..=little_clip_core::drives::MAX_CATALOG_WORKERS).contains(&config.catalog_workers) {
        return Err(format!(
            "Catalog workers must be between 1 and {}",
            little_clip_core::drives::MAX_CATALOG_WORKERS
        )
        .into());
    }
    if config.sync_interval_seconds == 0 {
        return Err("Drive synchronization requires a nonzero interval".into());
    }
    if config.ram_session_max_bytes < 128 * 1024 * 1024
        || config.ram_session_max_entries < 4096
        || config.ram_total_max_bytes < config.ram_session_max_bytes
        || config.ram_total_max_entries < config.ram_session_max_entries
        || config.ram_max_sessions == 0
        || config.ram_max_sessions > 32
    {
        return Err("RAM quotas require at least 128 MiB/4096 entries, aggregate limits at least one session, and 1..32 sessions".into());
    }
    if config.backup_keep_versions == Some(0) {
        return Err("Backup retention must keep at least one completed version".into());
    }
    Ok(())
}
