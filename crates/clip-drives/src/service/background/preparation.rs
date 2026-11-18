//! Bulk roots are pinned and checked on the existing bounded scheduling workers.
use super::*;

pub(in crate::service) struct Input {
    state: PathBuf,
    backup: Option<PathBuf>,
    review: Option<preflight::Review>,
    catalog: bool,
}
impl Input {
    pub fn capture(owner: &Owner, envelope: &Envelope) -> Option<Self> {
        let catalog = matches!(
            envelope.request,
            DriveRequest::Catalog { .. } | DriveRequest::CatalogWithLimits { .. }
        );
        (catalog
            || matches!(
                envelope.request,
                DriveRequest::Backup {
                    kind: BackupKind::Files,
                    ..
                }
            ))
        .then(|| Self {
            state: owner.state.clone(),
            backup: owner.config.backup_directory.clone(),
            review: envelope.review.clone(),
            catalog,
        })
    }
    pub fn prepare(
        &self,
        source: Option<scheduling::Source>,
        inventory: &Inventory,
        cancel: &AtomicBool,
    ) -> Result<scheduling::Source> {
        cancelled(cancel)?;
        let mut source =
            source.ok_or("Mount the partition before saving a file backup or catalog")?;
        if let Some(review) = &self.review {
            review.validate(&source.drive)?;
            source.root = review
                .source
                .clone()
                .ok_or("Reviewed backup source is not mounted")?;
        }
        let destination = self.destination(&source)?;
        source.root.validate()?;
        destination.validate()?;
        cancelled(cancel)?;
        if discovery::select(inventory()?, &source.drive.id)? != source.drive {
            return Err("Drive changed while pinning bulk-operation roots".into());
        }
        source.destination = Some(destination);
        Ok(source)
    }
    fn destination(&self, source: &scheduling::Source) -> Result<Arc<Root>> {
        if self.catalog {
            Ok(Arc::new(Root::open(&self.state)?))
        } else {
            let db = Store::existing(&self.state.join("drives.sqlite"), false)?;
            let destination = preflight::destination(
                &db,
                &self.state,
                self.backup.as_deref(),
                self.review.as_ref(),
                Some(&source.root),
            )?;
            preflight::space(&destination, &source.drive, BackupKind::Files)?;
            Ok(destination)
        }
    }
}
