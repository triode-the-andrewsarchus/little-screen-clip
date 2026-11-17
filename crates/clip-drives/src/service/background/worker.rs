//! One bulk operation owns its input pins until its result is delivered.
use super::*;

pub(super) struct Worker {
    pub state: PathBuf,
    pub id: String,
    pub request: DriveRequest,
    pub drive: Drive,
    pub config: DriveConfig,
    pub roots: Vec<Arc<Root>>,
    pub context: Option<partition::BackupContext>,
    pub cancel: Arc<AtomicBool>,
    pub authority: Arc<partition::Authority>,
    pub inventory: Inventory,
}
impl Worker {
    pub fn run(self) -> Result<String> {
        let validation = match self.validate() {
            Err(error) if self.context.is_none() => return Err(error),
            other => other,
        };
        let db = Store::existing(&self.state.join("drives.sqlite"), false);
        let mut progress = job_progress(self.state.clone(), self.id.clone());
        let result = validation.and_then(|()| match &db {
            Ok(db) => self.perform(db, &mut progress),
            Err(error) => Err(error.to_string().into()),
        });
        match &self.context {
            Some(context) if context.file_mode == FileBackupMode::Live => context.finish(
                db.as_ref().map_err(|error| error.to_string().into()),
                &self.id,
                &self.inventory,
                &self.authority,
                result,
            ),
            _ => result,
        }
    }
    fn validate(&self) -> Result<()> {
        cancelled(&self.cancel)?;
        for root in &self.roots {
            root.validate()?;
        }
        if self.read_only()
            && !self
                .roots
                .first()
                .is_some_and(|root| root.read_only_required())
        {
            return Err("Read-only collection has no guarded source admission".into());
        }
        Ok(())
    }
    fn read_only(&self) -> bool {
        matches!(self.request, DriveRequest::ReadOnlyBackup { .. })
    }
    fn perform(&self, db: &Store, progress: &mut impl FnMut(u64, u64)) -> Result<String> {
        match self.request {
            DriveRequest::Backup { kind, .. } => self.backup(db, kind, progress),
            DriveRequest::ReadOnlyBackup { .. } => self.backup(db, BackupKind::Files, progress),
            DriveRequest::Catalog { .. } => {
                let catalog = catalog::capture(
                    db,
                    self.roots
                        .last()
                        .ok_or("Missing admitted catalog state root")?,
                    &self.drive,
                    self.roots
                        .first()
                        .ok_or("Missing admitted partition root")?,
                    &self.config,
                    &self.cancel,
                    progress,
                )?;
                Ok(format!(
                    "Catalog saved: {} · {}",
                    catalog.id, catalog.coverage
                ))
            }
            _ => Err("Invalid bulk operation".into()),
        }
    }
    fn backup(
        &self,
        db: &Store,
        kind: BackupKind,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        backup::recover_publications(db)?;
        let destination = self
            .roots
            .last()
            .ok_or("Missing admitted backup destination")?;
        preflight::space(destination, &self.drive, kind)?;
        let version = match kind {
            BackupKind::Files => {
                if self.read_only() {
                    partition::phase(db, &self.id, PartitionPhase::CopyingFiles, None)?;
                }
                backup::files(
                    db,
                    &self.drive,
                    self.roots
                        .first()
                        .ok_or("Missing admitted partition root")?,
                    destination,
                    &self.cancel,
                    progress,
                )?
            }
            BackupKind::Image => self.image(db, destination, progress)?,
        };
        let warning = self
            .record_publication(db, &version)
            .err()
            .map(|error| format!("; completion journal update failed: {error}"))
            .unwrap_or_default();
        Ok(format!("{}{}", self.describe(&version), warning))
    }
    fn image(
        &self,
        db: &Store,
        destination: &Root,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<DriveBackup> {
        let context = self
            .context
            .as_ref()
            .ok_or("Missing original device admission context")?;
        let mut previous = None;
        backup::image(
            db,
            &self.drive,
            &context.source,
            destination,
            &self.cancel,
            &self.authority.open_image,
            |phase, bytes| {
                if previous != Some(phase) {
                    partition::phase(db, &self.id, phase, None)?;
                    previous = Some(phase);
                }
                progress(0, bytes);
                Ok(())
            },
        )
    }
    fn record_publication(&self, db: &Store, version: &DriveBackup) -> Result<()> {
        if self.context.is_some() {
            let phase = match version.kind {
                BackupKind::Files => PartitionPhase::FilesPublished,
                BackupKind::Image => PartitionPhase::ImagePublished,
            };
            partition::phase(db, &self.id, phase, Some(version))?;
        }
        Ok(())
    }
    fn describe(&self, version: &DriveBackup) -> String {
        let fidelity = version.fidelity.as_ref().map(|report| format!(
            "; metadata omissions: {} field(s) across {} entries; source attribute discovery unavailable/incomplete for {} entries; details: {}",
            report.omitted_fields, report.entries_with_omissions, report.source_attributes_unavailable,
            version.manifest.as_deref().unwrap_or(&version.path).display(),
        )).unwrap_or_default();
        let consistency = if self.read_only() {
            "; verified read-only filesystem collection"
        } else if version.kind == BackupKind::Files {
            "; per-file verified collection, not an atomic source snapshot"
        } else {
            ""
        };
        format!(
            "Backup completed: {} ({} bytes written){}{}",
            version.path.display(),
            version.bytes_written,
            consistency,
            fidelity
        )
    }
}
