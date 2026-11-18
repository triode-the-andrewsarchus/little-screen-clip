//! Enrollment checks keep managed storage out of synchronization feedback loops.
use super::*;

/// Descriptor-free managed-storage inputs, compared again after worker preparation.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Storage {
    state: PathBuf,
    backups: Vec<PathBuf>,
    ram: Vec<(String, PathBuf, Option<PathBuf>)>,
    catalogs: Vec<PathBuf>,
}
impl Storage {
    pub fn capture(db: &Store, state: &Path, backup_directory: Option<&Path>) -> Result<Self> {
        let snapshot =
            db.0.is_autocommit()
                .then(|| db.0.unchecked_transaction())
                .transpose()?;
        let mut backups = preflight::managed_backup_paths(db)?;
        backups.extend(backup_directory.map(Path::to_path_buf));
        let ram = db
            .list::<RamSession>("ram", 0, 1000)?
            .into_iter()
            .map(|s| (s.id, s.path, s.backing.map(|b| b.directory)))
            .collect();
        let catalogs = db
            .list::<Catalog>("catalog", 0, 1000)?
            .into_iter()
            .filter_map(|c| c.mount)
            .collect();
        if let Some(snapshot) = snapshot {
            snapshot.commit()?;
        }
        Ok(Self {
            state: state.to_path_buf(),
            backups,
            ram,
            catalogs,
        })
    }
    pub fn validate(&self, source: &Root, local: &Root, own_ram: Option<&str>) -> Result<()> {
        validate_backing_paths(
            self.ram
                .iter()
                .filter_map(|(_, _, backing)| backing.as_ref()),
            source,
        )?;
        if own_ram.is_none() {
            validate_backing_paths(
                self.ram
                    .iter()
                    .filter_map(|(_, _, backing)| backing.as_ref()),
                local,
            )?;
        }
        if source.overlap(local) {
            return Err("Sync roots overlap after a directory or mount change; pause and re-enroll the pair".into());
        }
        let managed = std::iter::once((&self.state, false))
            .chain(self.backups.iter().map(|p| (p, false)))
            .chain(
                self.ram
                    .iter()
                    .map(|(id, path, _)| (path, own_ram == Some(id.as_str()))),
            )
            .chain(self.catalogs.iter().map(|p| (p, false)));
        for (path, own_view) in managed {
            if managed_overlap(path, source, (!own_view).then_some(local)) {
                return Err("Sync roots overlap managed backups, RAM or catalog storage".into());
            }
        }
        Ok(())
    }
}

// Missing managed roots retain their lexical boundary; observed roots also
// compare filesystem coordinates, covering bind aliases without granting writes.
fn managed_overlap(path: &Path, source: &Root, local: Option<&Root>) -> bool {
    let managed = Root::open(path).ok();
    std::iter::once(source)
        .chain(local)
        .any(|target| match &managed {
            Some(root) => target.overlap(root),
            None => target.path.starts_with(path) || path.starts_with(&target.path),
        })
}

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
        Storage::capture(
            &self.db,
            &self.state,
            self.config.backup_directory.as_deref(),
        )?
        .validate(source, local, own_ram)
    }
    pub(super) fn validate_ram_backing_target(&self, target: &Root) -> Result<()> {
        validate_ram_backing_storage(&self.db, target)
    }
    pub(super) fn validate_backup_directory(&self, target: &Root) -> Result<()> {
        validate_backup_storage(&self.db, &self.state, target)
    }
}
fn validate_ram_backing_storage(db: &Store, target: &Root) -> Result<()> {
    let sessions = db.list::<RamSession>("ram", 0, 1000)?;
    validate_backing_paths(
        sessions
            .iter()
            .filter_map(|session| session.backing.as_ref().map(|backing| &backing.directory)),
        target,
    )
}
fn validate_backing_paths<'a>(
    paths: impl Iterator<Item = &'a PathBuf>,
    target: &Root,
) -> Result<()> {
    for path in paths {
        if let Ok(root) = Root::open(path) {
            if target.overlap(&root) {
                return Err("Destination overlaps private RAM backing storage".into());
            }
        } else if target.path.starts_with(path) || path.starts_with(&target.path) {
            return Err("Destination overlaps unavailable RAM backing storage".into());
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

/// Reads only headers referenced by the existing mounted-view registries.
pub(super) fn mounted_views(db: &Store) -> Result<Vec<PathBuf>> {
    #[derive(serde::Deserialize)]
    struct Location {
        #[serde(default, with = "little_clip_core::path_serde::optional")]
        mount: Option<PathBuf>,
    }
    let mut paths = Vec::new();
    for (registry, kind) in [("catalog-mount", "catalog"), ("image-mount", "backup")] {
        let mut after = String::new();
        loop {
            let ids = db.ids(registry, &after, 128)?;
            let Some(last) = ids.last() else {
                break;
            };
            after = last.clone();
            for id in ids {
                if let Some(location) = db.get::<Location>(kind, &id)? {
                    paths.extend(location.mount);
                }
            }
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod mounted_tests {
    use super::*;
    #[test]
    fn mounted_view_lookup_pages_only_registered_headers_and_preserves_raw_paths() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        let transaction = db.0.unchecked_transaction().unwrap();
        // Invalid historical header payloads must not enter this bounded lookup.
        for number in 0..1100 {
            db.put("catalog", &format!("history-{number}"), &())
                .unwrap();
            db.put("backup", &format!("history-{number}"), &()).unwrap();
        }
        let mut expected = Vec::new();
        for (registry, kind) in [("catalog-mount", "catalog"), ("image-mount", "backup")] {
            for number in 0..130 {
                let id = uuid::Uuid::new_v4().to_string();
                let path = temp.path().join(format!("{kind}-{number}"));
                db.put(registry, &id, &1_u64).unwrap();
                db.put(kind, &id, &serde_json::json!({ "mount": path }))
                    .unwrap();
                expected.push(path);
            }
        }
        use std::os::unix::ffi::OsStringExt;
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/owned/raw_\xff".to_vec()));
        let id = uuid::Uuid::new_v4().to_string();
        db.put("catalog-mount", &id, &1_u64).unwrap();
        db.put(
            "catalog",
            &id,
            &serde_json::json!({ "mount": raw.as_os_str().as_encoded_bytes() }),
        )
        .unwrap();
        expected.push(raw);
        transaction.commit().unwrap();
        let mut observed = mounted_views(&db).unwrap();
        observed.sort();
        expected.sort();
        assert_eq!(observed, expected);
    }
}
