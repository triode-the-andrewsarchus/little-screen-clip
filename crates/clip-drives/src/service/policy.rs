//! Enrollment checks keep managed storage out of synchronization feedback loops.
use super::*;
mod views;
pub(super) use views::mounted_views;

pub(super) fn records<T: serde::de::DeserializeOwned>(db: &Store, kind: &str) -> Result<Vec<T>> {
    let snapshot =
        db.0.is_autocommit()
            .then(|| db.0.unchecked_transaction())
            .transpose()?;
    let mut records = Vec::new();
    db.visit::<T>(kind, |record| {
        records.push(record);
        Ok(())
    })?;
    if let Some(snapshot) = snapshot {
        snapshot.commit()?;
    }
    Ok(records)
}

/// Descriptor-free managed-storage inputs, compared again after worker preparation.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Storage {
    state: PathBuf,
    backups: Vec<PathBuf>,
    ram: Vec<(String, PathBuf, Option<PathBuf>)>,
    allocations: Vec<PathBuf>,
    views: Vec<PathBuf>,
}
impl Storage {
    pub fn capture(db: &Store, state: &Path, backup_directory: Option<&Path>) -> Result<Self> {
        let snapshot =
            db.0.is_autocommit()
                .then(|| db.0.unchecked_transaction())
                .transpose()?;
        let mut backups = preflight::managed_backup_paths(db)?;
        backups.extend(backup_directory.map(Path::to_path_buf));
        let ram = records::<RamSession>(db, "ram")?
            .into_iter()
            .map(|s| (s.id, s.path, s.backing.map(|b| b.directory)))
            .collect();
        let allocations = allocation_paths(db)?;
        let views = mounted_views(db)?;
        if let Some(snapshot) = snapshot {
            snapshot.commit()?;
        }
        Ok(Self {
            state: state.to_path_buf(),
            backups,
            ram,
            allocations,
            views,
        })
    }
    pub fn validate(&self, source: &Root, local: &Root, own_ram: Option<&str>) -> Result<()> {
        if source.overlap(local) {
            return Err("Sync roots overlap after a directory or mount change; pause and re-enroll the pair".into());
        }
        self.validate_roots(source, Some(local), own_ram)
    }
    pub fn validate_source(&self, source: &Root) -> Result<()> {
        self.validate_roots(source, None, None)
    }
    fn validate_roots(
        &self,
        source: &Root,
        local: Option<&Root>,
        own_ram: Option<&str>,
    ) -> Result<()> {
        validate_backing_paths(
            self.ram
                .iter()
                .filter_map(|(_, _, backing)| backing.as_ref())
                .chain(self.allocations.iter()),
            source,
        )?;
        if let Some(local) = local.filter(|_| own_ram.is_none()) {
            validate_backing_paths(
                self.ram
                    .iter()
                    .filter_map(|(_, _, backing)| backing.as_ref())
                    .chain(self.allocations.iter()),
                local,
            )?;
        }
        let mounts = crate::mounts::inventory()?;
        let aliases = managed_mounts(&mounts, |id| {
            self.ram.iter().any(|(known, _, _)| known == id)
        })
        .collect::<Vec<_>>();
        let managed = std::iter::once((&self.state, false))
            .chain(self.backups.iter().map(|p| (p, false)))
            .chain(
                self.ram
                    .iter()
                    .map(|(id, path, _)| (path, own_ram == Some(id.as_str()))),
            )
            .chain(self.views.iter().map(|p| (p, false)));
        for (path, own_view) in managed {
            if managed_overlap(path, source, local.filter(|_| !own_view), &aliases) {
                return Err("Sync roots overlap managed backups, RAM or catalog storage".into());
            }
        }
        for (mount, ram) in aliases {
            let own_view = ram.is_some_and(|id| own_ram == Some(id));
            if path_overlap(&mount.target, source)
                || local
                    .filter(|_| !own_view)
                    .is_some_and(|local| path_overlap(&mount.target, local))
            {
                return Err("Sync roots overlap a managed catalog or RAM filesystem".into());
            }
        }
        Ok(())
    }
}

pub(super) fn managed_mounts<'a>(
    mounts: &'a [crate::mounts::Mount],
    known_ram: impl Fn(&str) -> bool + 'a,
) -> impl Iterator<Item = (&'a crate::mounts::Mount, Option<&'a str>)> {
    mounts
        .iter()
        .filter(|mount| mount.filesystem.starts_with("fuse"))
        .filter_map(move |mount| {
            let source = mount.source.as_os_str().as_encoded_bytes();
            if source.starts_with(b"little-clip-catalog:") {
                Some((mount, None))
            } else {
                let id = std::str::from_utf8(source.strip_prefix(b"little-clip-ram:")?).ok()?;
                known_ram(id).then_some((mount, Some(id)))
            }
        })
}
fn path_overlap(path: &Path, root: &Root) -> bool {
    root.path.starts_with(path) || path.starts_with(&root.path)
}

pub(super) fn validate_view_target(db: &Store, target: &Root) -> Result<()> {
    let snapshot =
        db.0.is_autocommit()
            .then(|| db.0.unchecked_transaction())
            .transpose()?;
    let ram = records::<RamSession>(db, "ram")?;
    let views = mounted_views(db)?;
    if let Some(snapshot) = snapshot {
        snapshot.commit()?;
    }
    let mounts = crate::mounts::inventory()?;
    let aliases = managed_mounts(&mounts, |id| ram.iter().any(|session| session.id == id))
        .collect::<Vec<_>>();
    if aliases
        .iter()
        .any(|(mount, _)| path_overlap(&mount.target, target))
        || views
            .iter()
            .any(|path| managed_overlap(path, target, None, &aliases))
    {
        return Err("Destination overlaps a managed catalog or image view, or RAM view".into());
    }
    Ok(())
}

// Missing managed roots retain their lexical boundary; observed roots also
// compare filesystem coordinates, covering bind aliases without granting writes.
fn managed_overlap(
    path: &Path,
    source: &Root,
    local: Option<&Root>,
    aliases: &[(&crate::mounts::Mount, Option<&str>)],
) -> bool {
    let mut opaque = false;
    for (origin, _) in aliases
        .iter()
        .filter(|(mount, _)| path.starts_with(&mount.target))
    {
        opaque = true;
        // Resolving a nested managed path could itself call a stopped server.
        // Retain its fence across the observed FUSE superblock and every alias.
        if aliases
            .iter()
            .filter(|(mount, _)| mount.device == origin.device)
            .any(|(mount, _)| {
                path_overlap(&mount.target, source)
                    || local.is_some_and(|root| path_overlap(&mount.target, root))
            })
        {
            return true;
        }
    }
    if opaque {
        return false;
    }
    let managed = Root::open(path).ok();
    std::iter::once(source)
        .chain(local)
        .any(|target| match &managed {
            Some(root) => target.overlap(root),
            None => target.path.starts_with(path) || path.starts_with(&target.path),
        })
}

impl Owner {
    /// Every worker uses the same captured-root scheduling policy, including unmount.
    pub(super) fn claims_busy(&self, claims: &[crate::root::Scope]) -> bool {
        let overlaps = |scope: &crate::root::Scope| claims.iter().any(|claim| claim.overlap(scope));
        self.preferences
            .pending
            .as_ref()
            .is_some_and(|job| job.claims.iter().any(&overlaps))
            || self
                .partition
                .as_ref()
                .is_some_and(|job| job.claims.iter().any(&overlaps))
            || self
                .partition_preparing
                .as_ref()
                .is_some_and(|job| job.claims.iter().any(&overlaps))
            || self
                .ram_access
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
                .sync_preparing
                .values()
                .any(|job| job.claims.iter().any(&overlaps))
            || self
                .background
                .values()
                .any(|job| job.scopes().any(|scope| overlaps(&scope)))
    }
}
pub(super) fn validate_pair_targets(
    pairs: &[SyncPair],
    local: &Root,
    own: Option<&str>,
) -> Result<()> {
    if own.is_none() && pairs.len() >= 256 {
        return Err(
            "Pause or remove an existing enrollment before adding more than 256 sync pairs".into(),
        );
    }
    if overlapping_pair_roots(pairs, local, own)? {
        return Err("This local folder overlaps an existing sync enrollment".into());
    }
    Ok(())
}
fn overlapping_pair_roots(pairs: &[SyncPair], local: &Root, own: Option<&str>) -> Result<bool> {
    let mounts = crate::mounts::inventory()?;
    let aliases = managed_mounts(&mounts, |id| {
        pairs.iter().any(|pair| pair.ram && pair.id == id)
    })
    .collect::<Vec<_>>();
    Ok(pairs
        .iter()
        .filter(|pair| own != Some(pair.id.as_str()))
        .any(|pair| managed_overlap(&pair.local, local, None, &aliases)))
}

pub(super) fn same_pair_roots(db: &Store, prior: &[SyncPair]) -> Result<bool> {
    let current = records::<SyncPair>(db, "pair")?;
    Ok(prior
        .iter()
        .map(|pair| (&pair.id, &pair.local, &pair.local_identity))
        .eq(current
            .iter()
            .map(|pair| (&pair.id, &pair.local, &pair.local_identity))))
}
fn allocation_paths(db: &Store) -> Result<Vec<PathBuf>> {
    Ok(records::<ram_setup::Allocation>(db, "ram-allocation")?
        .into_iter()
        .filter_map(|allocation| allocation.backing.map(|backing| backing.directory))
        .collect())
}

pub(super) fn validate_ram_backing_storage(db: &Store, target: &Root) -> Result<()> {
    let sessions = records::<RamSession>(db, "ram")?;
    let allocations = allocation_paths(db)?;
    validate_backing_paths(
        sessions
            .iter()
            .filter_map(|session| session.backing.as_ref().map(|backing| &backing.directory))
            .chain(allocations.iter()),
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
    if overlapping_pair_roots(&records::<SyncPair>(db, "pair")?, target, None)? {
        return Err("Backup directory overlaps a paired local folder or RAM copy".into());
    }
    validate_view_target(db, target)
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

#[cfg(test)]
mod mounted_tests {
    use super::*;

    #[test]
    fn registered_ram_mounts_remain_managed_beyond_the_status_page() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        let identity = kernel_interface::MountIdentity::directory(temp.path()).unwrap();
        let transaction = db.0.unchecked_transaction().unwrap();
        for number in 0..1101 {
            let id = format!("session-{number:04}");
            db.put("ram", &id, &serde_json::json!({
                "id": id, "drive": "owned", "path": temp.path().join(&id),
                "identity": identity, "ready": true, "connected": false, "status": "retained legacy record",
            })).unwrap();
        }
        transaction.commit().unwrap();
        assert!(
            db.list::<RamSession>("ram", 0, 1000)
                .unwrap()
                .iter()
                .all(|session| session.id != "session-0000")
        );
        let records = records::<RamSession>(&db, "ram").unwrap();
        assert_eq!(records.len(), 1101);
        let mount = crate::mounts::Mount {
            id: 1,
            device: 1,
            target: "/owned/alias".into(),
            source: "little-clip-ram:session-0000".into(),
            filesystem: "fuse.clip-ram".into(),
            root: "/".into(),
            read_only: false,
            super_read_only: false,
        };
        assert_eq!(
            managed_mounts(std::slice::from_ref(&mount), |id| records
                .iter()
                .any(|s| s.id == id))
            .count(),
            1
        );
        assert_eq!(
            managed_mounts(std::slice::from_ref(&mount), |_| false).count(),
            0
        );
        let ordinary = crate::mounts::Mount {
            filesystem: "ext4".into(),
            ..mount
        };
        assert_eq!(managed_mounts(&[ordinary], |_| true).count(), 0);
    }

    fn header(db: &Store, kind: &str, id: &str, mount: Option<&Path>) {
        db.put(kind, id, &serde_json::json!({
            "id": id, "drive": "owned", "evidence": "owned", "label": "owned",
            "captured": 0, "entries": 0, "complete": true, "coverage": "complete",
            "path": "/owned/backups/bucket/version", "mount": mount.map(|p| p.as_os_str().as_encoded_bytes()),
        })).unwrap();
    }

    #[test]
    fn old_registered_and_legacy_views_protect_sync_and_backup_admission() {
        for kind in ["catalog", "backup"] {
            for registered in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let state = Root::create(&temp.path().join("state")).unwrap();
                let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
                let view = Root::create(&temp.path().join("view")).unwrap();
                let local = Root::create(&temp.path().join("local")).unwrap();
                fs::write(view.path.join("keep"), b"original view bytes").unwrap();
                header(&db, kind, "old-mounted", Some(&view.path));
                if registered {
                    db.put(
                        if kind == "catalog" {
                            "catalog-mount"
                        } else {
                            "image-mount"
                        },
                        "old-mounted",
                        &serde_json::json!({"path": view.path}),
                    )
                    .unwrap();
                }
                let transaction = db.0.unchecked_transaction().unwrap();
                for index in 0..1100 {
                    header(&db, kind, &format!("history-{index:04}"), None);
                }
                transaction.commit().unwrap();
                let storage = Storage::capture(&db, &state.path, None).unwrap();
                assert!(storage.validate_source(&view).is_err());
                assert!(validate_view_target(&db, &view).is_err());
                assert!(
                    storage.validate(&view, &local, None).is_err(),
                    "{kind}, registered={registered}: managed source was accepted"
                );
                assert!(
                    storage.validate(&local, &view, None).is_err(),
                    "{kind}, registered={registered}: managed target was accepted"
                );
                assert!(
                    validate_backup_storage(&db, &state.path, &view).is_err(),
                    "{kind}, registered={registered}: backup target was accepted"
                );
                assert_eq!(
                    fs::read(view.path.join("keep")).unwrap(),
                    b"original view bytes"
                );
            }
        }
    }

    #[test]
    fn unregistered_mounted_headers_remain_visible_beyond_the_history_page() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        let path = temp.path().join("legacy");
        header(&db, "catalog", "legacy", Some(&path));
        for index in 0..1100 {
            header(&db, "catalog", &format!("history-{index}"), None);
        }
        assert_eq!(mounted_views(&db).unwrap(), vec![path]);
    }

    #[test]
    fn uncertain_view_metadata_refuses_admission_without_discarding_the_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        for (kind, registry) in [("catalog", "catalog-mount"), ("backup", "image-mount")] {
            db.put(registry, "owned", &1_u64).unwrap();
            assert!(
                mounted_views(&db)
                    .unwrap_err()
                    .to_string()
                    .contains("no saved header")
            );
            for mount in [
                None,
                Some(Path::new("relative")),
                Some(Path::new("/root/../view")),
                Some(Path::new("/view\0bad")),
            ] {
                header(&db, kind, "owned", mount);
                assert!(mounted_views(&db).is_err(), "{kind}: {mount:?}");
            }
            header(&db, kind, "owned", Some(&temp.path().join("view")));
            db.0.execute(
                "UPDATE entities SET json='broken json' WHERE kind=?1 AND id='owned'",
                [kind],
            )
            .unwrap();
            assert!(mounted_views(&db).is_err());
            db.put(
                kind,
                "owned",
                &serde_json::json!({"mount": temp.path(), "extra": "x".repeat(65536)}),
            )
            .unwrap();
            assert!(
                mounted_views(&db)
                    .unwrap_err()
                    .to_string()
                    .contains("decoding budget")
            );
            db.remove(kind, "owned").unwrap();
            db.remove(registry, "owned").unwrap();
            assert!(mounted_views(&db).unwrap().is_empty());
        }
    }

    #[test]
    fn image_header_and_runtime_disagreement_preserves_both_deny_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        let first = Root::create(&temp.path().join("first")).unwrap();
        let second = Root::create(&temp.path().join("second")).unwrap();
        header(&db, "backup", "image", Some(&first.path));
        db.put(
            "image-mount",
            "image",
            &serde_json::json!({"path": second.path}),
        )
        .unwrap();
        assert_eq!(
            mounted_views(&db).unwrap(),
            vec![first.path.clone(), second.path.clone()]
        );
        for root in [&first, &second] {
            assert!(validate_view_target(&db, root).is_err());
        }
    }

    #[test]
    fn mounted_view_lookup_pages_candidates_and_registries_and_preserves_raw_paths() {
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
