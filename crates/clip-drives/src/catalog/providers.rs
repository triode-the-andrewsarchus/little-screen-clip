//! Bind candidate contents to the captured revision and completed backup owner.
use super::*;

pub(super) enum ProviderIdentity {
    Original,
    Files(Backup),
    Image(backup::ImageMount),
}
impl ProviderIdentity {
    fn validate(&self, root: &Root) -> Result<()> {
        match self {
            Self::Original => root.validate(),
            Self::Files(version) if Some(root.storage_identity()?) == version.identity => Ok(()),
            Self::Files(_) => Err("Backup directory identity changed".into()),
            Self::Image(mount) => mount.validate(root),
        }
    }

    pub(super) fn open(
        &self,
        root: Root,
        view: &CatalogFs,
        entry: &Entry,
    ) -> Result<Option<OpenFile>> {
        let lease = match self {
            Self::Files(_) => Some(backup::provider_lease(&root, false)?),
            _ => None,
        };
        self.validate(&root)?;
        let expected = match self {
            Self::Files(version) if entry.hash.is_none() => view.backup_entry(version, entry)?,
            _ => entry.clone(),
        };
        let Ok(file) = root.input(&entry.path) else {
            return Ok(None);
        };
        let mut open = OpenFile::validated(root, file, &expected)?;
        open._provider_lease = lease;
        Ok(Some(open))
    }
}

impl CatalogFs {
    fn backup_entry(&self, version: &Backup, entry: &Entry) -> Result<Entry> {
        let path = backup::completed_manifest(version).ok_or("Backup manifest is unavailable")?;
        let mut bytes = u64::MAX;
        backup::read_manifest(
            version,
            path,
            &AtomicBool::new(false),
            &mut bytes,
            Duration::MAX,
            |manifest| {
                let source: Entry = manifest
                    .get_bounded("source", "root", 64 * 1024)?
                    .ok_or("Backup source identity is unknown")?;
                let details = self
                    .catalog
                    .details
                    .as_ref()
                    .ok_or("Catalog source identity is unknown")?;
                if source.kind != "directory"
                    || !source.path.as_os_str().is_empty()
                    || source.device != details.source_mount.device
                    || source.inode != details.source_mount.inode
                {
                    return Err("Backup belongs to a different captured root".into());
                }
                let saved = scan::entry(manifest, &format!("backup:{}", version.id), &entry.path)?
                    .ok_or("Backup does not contain the captured entry")?;
                if saved.path != entry.path || !saved.same_revision(entry) {
                    return Err("Backup source revision is stale".into());
                }
                let hash = saved
                    .hash
                    .filter(|hash| {
                        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .ok_or("Backup has no verified content fingerprint")?;
                let mut expected = entry.clone();
                expected.hash = Some(hash.to_ascii_lowercase());
                Ok(expected)
            },
        )
    }
}
