//! Read-only collection owns its mount until source readers retire and cleanup exits.
use super::*;
use little_clip_core::kernel_interface::{MountIdentity, Namespace};

#[derive(Clone)]
pub(in crate::service) struct OwnedMount {
    drive: Drive,
    identity: MountIdentity,
    namespace: Namespace,
}
impl OwnedMount {
    fn capture(drive: &Drive) -> Result<Self> {
        let path = drive
            .mount
            .as_ref()
            .ok_or("Read-only mount was not observed")?;
        let root = Root::open(path)?;
        let directory = root.directory(Path::new(""), false)?;
        let namespace = Namespace::current();
        if !namespace.known() {
            return Err("Read-only mount namespace is unknown".into());
        }
        Ok(Self {
            drive: drive.clone(),
            identity: MountIdentity::pinned(&directory)?,
            namespace,
        })
    }
    fn validate(&self, actual: &Drive) -> Result<()> {
        if actual.mount != self.drive.mount || Namespace::current() != self.namespace {
            return Err("Collection mount was replaced; refusing to unmount another view".into());
        }
        let root = Root::open(
            actual
                .mount
                .as_ref()
                .ok_or("Collection mount disappeared")?,
        )?;
        if MountIdentity::pinned(&root.directory(Path::new(""), false)?)? != self.identity {
            return Err(
                "Collection mount identity changed; original state needs manual inspection".into(),
            );
        }
        Ok(())
    }
}

pub(super) struct Prepared {
    pub context: BackupContext,
    pub result: Result<(Drive, Arc<Root>)>,
}

impl BackupContext {
    fn mount_read_only(
        &mut self,
        db: &Store,
        id: &str,
        inventory: &Inventory,
        authority: &Authority,
        cancel: &AtomicBool,
    ) -> Result<(Drive, Arc<Root>)> {
        self.source.validate()?;
        let before = self.observed(inventory)?;
        if before.mount.is_some() {
            return Err(
                "Partition mounted before read-only admission; review its current state".into(),
            );
        }
        cancelled(cancel)?;
        phase(db, id, PartitionPhase::MountingReadOnly, None)?;
        let mut desired = self.original.clone();
        desired.mount = None;
        desired.read_only = true;
        (authority.mount)(&desired)?;
        self.source.validate()?;
        let actual = self.observed(inventory)?;
        let root = self.confirm_read_only_mount(db, id, &actual)?;
        cancelled(cancel)?;
        Ok((actual, root))
    }
    fn confirm_read_only_mount(
        &mut self,
        db: &Store,
        id: &str,
        actual: &Drive,
    ) -> Result<Arc<Root>> {
        self.read_only_mount = Some(OwnedMount::capture(actual)?);
        let mut operation = db
            .get::<DriveOperation>("operation", id)?
            .ok_or("Missing read-only operation")?;
        operation
            .partition
            .as_mut()
            .ok_or("Missing read-only journal")?
            .collection_mount = actual.mount.clone();
        db.put("operation", id, &operation)?;
        let root = Arc::new(Root::open_with_read_only(
            actual.mount.as_ref().ok_or("Read-only mount missing")?,
            true,
        )?);
        if !actual.read_only {
            return Err("Inventory did not confirm read-only access".into());
        }
        phase(db, id, PartitionPhase::ReadOnlyMounted, None)?;
        Ok(root)
    }

    pub(super) fn restore_read_only(
        &self,
        inventory: &Inventory,
        authority: &Authority,
    ) -> Result<Drive> {
        self.source.validate()?;
        let before = self.observed(inventory)?;
        if let Some(mounted) = &self.read_only_mount {
            mounted.validate(&before)?;
            if before.read_only == self.original.read_only {
                return Ok(before);
            }
            // Accepted cleanup retains its authority reply even after cancellation.
            unmount_checked(
                &before,
                &before.id,
                inventory,
                &authority.unmount,
                &AtomicBool::new(false),
                Some(&self.source),
            )?;
        } else if before.mount.is_some() {
            if before.read_only == self.original.read_only {
                return Ok(before);
            }
            return Err("An unconfirmed mount remains; inspect it before restoring access".into());
        }
        self.restore_mounted(inventory, authority)
    }
}

impl Owner {
    pub(super) fn start_read_only(&mut self, mut job: Pending, context: BackupContext) {
        let state = self.state.clone();
        let id = job.envelope.id.clone();
        let inventory = self.inventory.clone();
        let authority = self.authority.clone();
        let cancel = job.cancel.clone();
        let mut worker_context = context.clone();
        let task = write_task(&self.partition_jobs, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let result = worker_context.mount_read_only(&db, &id, &inventory, &authority, &cancel);
            Ok(Box::new(Prepared {
                context: worker_context,
                result,
            }))
        });
        job.stage = Stage::ReadOnly(task, Box::new(context));
        self.partition = Some(job);
    }

    pub(super) fn read_only_ready(
        &mut self,
        job: Pending,
        result: Result<Box<Prepared>>,
        fallback: Box<BackupContext>,
    ) {
        let Prepared { context, result } = match result {
            Ok(prepared) => *prepared,
            Err(error) => {
                self.finish_partition_backup(job, *fallback, Err(error));
                return;
            }
        };
        match result {
            Ok((drive, root))
                if !job.cancel.load(Ordering::Acquire) && !self.closing.load(Ordering::Acquire) =>
            {
                let mut roots = vec![root];
                roots.extend(job.destination);
                self.launch_bulk(job.envelope, drive, job.config, roots, Some(context));
            }
            other => {
                // Retire the successful source pin before cleanup can unmount it.
                let error = match other {
                    Err(error) => error,
                    Ok((_, root)) => {
                        drop(root);
                        "Read-only mount completed; collection cancelled before copying".into()
                    }
                };
                self.finish_partition_backup(job, context, Err(error));
            }
        }
    }
}
