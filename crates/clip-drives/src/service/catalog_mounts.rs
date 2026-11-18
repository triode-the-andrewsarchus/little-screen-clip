//! Catalog controls retain actual filesystem and helper lifetimes off the owner.
use super::*;
use little_clip_core::jobs::task::Task;

pub(super) struct Server(pub std::process::Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub(super) struct Controls {
    pending: Option<Pending>,
    after: Option<String>,
    jobs: little_clip_core::jobs::BackgroundJobs,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            pending: None,
            after: Some(String::new()),
            jobs: little_clip_core::jobs::BackgroundJobs::new_named(1, 1, "drive-catalog-mount"),
        }
    }
}
impl Controls {
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }
}
struct Pending {
    id: String,
    envelope: Option<Envelope>,
    task: Task<Result<Outcome>>,
}
struct Outcome {
    server: Option<Server>,
    result: Result<String>,
}
#[derive(Clone, Copy)]
enum Action {
    Mount,
    Unmount,
    Recover,
    Stop,
}
struct Work {
    state: PathBuf,
    id: String,
    action: Action,
    server: Option<Server>,
    cancel: Arc<AtomicBool>,
    operation: Option<String>,
}
impl Work {
    fn run(mut self) -> Outcome {
        let result = (|| {
            let db = Store::existing(&self.state.join("drives.sqlite"), false)?;
            if let Some(id) = &self.operation {
                let mut record: DriveOperation = db
                    .get("operation", id)?
                    .ok_or("Accepted catalog control is missing")?;
                record.state = "running".into();
                record.message = "Waiting for catalog filesystem".into();
                db.put("operation", id, &record)?;
            }
            let saved = db
                .get::<serde_json::Value>("catalog", &self.id)?
                .ok_or("Unknown catalog")?;
            let catalog: Catalog = serde_json::from_value(saved.clone())?;
            if catalog.id != self.id || !single_name(&self.id) {
                return Err(
                    "Catalog registry address is invalid; retaining its mount boundary".into(),
                );
            }
            match self.action {
                Action::Mount => self.mount(&db, catalog, saved),
                _ => self.unmount(&db, catalog),
            }
        })();
        if matches!(self.action, Action::Stop) {
            // Shutdown retains failed/busy mount metadata, then closes its server.
            // Killing/reaping is part of this retained task, never owner polling.
            drop(self.server.take());
        }
        Outcome {
            server: self.server,
            result,
        }
    }
    fn mount(&mut self, db: &Store, catalog: Catalog, saved: serde_json::Value) -> Result<String> {
        cancelled(&self.cancel)?;
        let mounts = prepare_target(&self.state, &self.id, &catalog)?;
        let path = mounts.path.join(&self.id);
        cancelled(&self.cancel)?;
        self.server = Some(Server::spawn(&self.state, &self.id, &path)?);
        let ready = self
            .server
            .as_mut()
            .expect("owned catalog server")
            .wait_ready(&path, &self.id);
        let mount = match ready {
            Ok(mount) => mount,
            Err(error) => {
                drop(self.server.take());
                return Err(error);
            }
        };
        publish_mount(db, catalog, saved, mount)?;
        Ok(format!("Catalog mounted at {}", path.display()))
    }
    fn unmount(&mut self, db: &Store, catalog: Catalog) -> Result<String> {
        if matches!(self.action, Action::Unmount) {
            cancelled(&self.cancel)?;
        }
        let expected = self.state.join("mounts").join(&self.id);
        let path = catalog.mount.as_deref().unwrap_or(&expected);
        if matches!(self.action, Action::Recover) && path != expected {
            return Err("Saved catalog mount path differs from its owned startup location; retaining boundary for explicit cleanup".into());
        }
        let message = if owned_target(db, &self.id, path)?.is_some() {
            remove_mount(path)?;
            "Catalog unmounted"
        } else {
            "Catalog is no longer mounted"
        };
        // Completion after actual unmount/confirmed absence survives cancellation.
        drop(self.server.take());
        retire_mount(db, &catalog)?;
        Ok(message.into())
    }
}
impl Server {
    fn spawn(state: &Path, id: &str, path: &Path) -> Result<Self> {
        let helper = std::env::var_os("LITTLE_CLIP_DRIVE_MOUNT_HELPER")
            .map(PathBuf::from)
            .unwrap_or(
                std::env::current_exe()?
                    .parent()
                    .ok_or("Missing executable directory")?
                    .join("clip-drive-mount"),
            );
        if !helper.is_absolute() {
            return Err("Catalog helper override must be an absolute path".into());
        }
        Ok(Self(
            CommandRequest::new(helper)
                .arg(state)
                .arg(id)
                .arg(path)
                .own_process_group()
                .die_with_parent()
                .build_command()
                .spawn()?,
        ))
    }
    fn wait_ready(&mut self, path: &Path, id: &str) -> Result<crate::mounts::Mount> {
        let started = Instant::now();
        loop {
            if let Some(mount) = crate::mounts::catalog_mount(path, id)? {
                return Ok(mount);
            }
            if let Some(status) = self.0.try_wait()? {
                return Err(format!("Catalog mount helper exited: {status}").into());
            }
            if started.elapsed() >= Duration::from_secs(5) {
                return Err("Catalog helper did not publish a ready FUSE mount".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
fn prepare_target(state: &Path, id: &str, catalog: &Catalog) -> Result<Root> {
    if !Path::new("/dev/fuse").exists() {
        return Err("FUSE is unavailable: /dev/fuse is missing".into());
    }
    if catalog.pending_redactions != 0 {
        return Err("Catalog metadata cleanup is pending; unmount its existing view and wait for cleanup before mounting again".into());
    }
    let mounts = Root::create(&state.join("mounts"))?;
    mounts.ensure_directory(Path::new(id))?;
    if crate::mounts::inventory()?
        .iter()
        .any(|mount| mount.target == mounts.path.join(id))
    {
        return Err("Catalog mountpoint is already occupied; unmount it first".into());
    }
    Ok(mounts)
}
fn publish_mount(
    db: &Store,
    mut catalog: Catalog,
    saved: serde_json::Value,
    mount: crate::mounts::Mount,
) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    if db
        .get::<serde_json::Value>("catalog", &catalog.id)?
        .as_ref()
        != Some(&saved)
    {
        return Err("Catalog changed during mounting; server retained for explicit unmount".into());
    }
    db.put("catalog-mount", &catalog.id, &mount.id)?;
    catalog.mount = Some(mount.target);
    db.put("catalog", &catalog.id, &catalog)?;
    transaction.commit()?;
    Ok(())
}
fn owned_target(db: &Store, id: &str, path: &Path) -> Result<Option<crate::mounts::Mount>> {
    let observed = crate::mounts::catalog_mount(path, id)?;
    if let Some(mount) = &observed
        && db
            .get::<u64>("catalog-mount", id)?
            .is_some_and(|recorded| recorded != mount.id)
    {
        return Err("Catalog mount identity changed; refusing unmount".into());
    }
    if observed.is_none()
        && crate::mounts::inventory()?
            .iter()
            .any(|mount| mount.target == path)
    {
        return Err(
            "Catalog mount ownership changed; refusing to unmount another filesystem".into(),
        );
    }
    Ok(observed)
}
fn remove_mount(path: &Path) -> Result<()> {
    let output = CommandRequest::new("fusermount3")
        .arg("-u")
        .arg(path)
        .timeout(Duration::from_secs(10))
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("Could not unmount catalog: {}", output.stderr_lossy()).into());
    }
    Ok(())
}
fn retire_mount(db: &Store, catalog: &Catalog) -> Result<()> {
    let transaction = db.0.unchecked_transaction()?;
    let mut current: Catalog = db
        .get("catalog", &catalog.id)?
        .ok_or("Catalog disappeared during unmount; retained registry needs repair")?;
    if current.mount != catalog.mount {
        return Err("Catalog mount path changed during unmount; retaining current boundary".into());
    }
    db.remove("catalog-mount", &catalog.id)?;
    current.mount = None;
    db.put("catalog", &catalog.id, &current)?;
    transaction.commit()?;
    Ok(())
}
fn single_name(id: &str) -> bool {
    let mut parts = Path::new(id).components();
    matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
}
impl Owner {
    pub(super) fn start_catalog_control(&mut self, envelope: Envelope) {
        if self.catalog_mounts.busy() || self.maintenance.busy() {
            self.defer_operation(envelope);
            return;
        }
        let (id, action) = match &envelope.request {
            DriveRequest::MountCatalog { catalog } => (catalog.clone(), Action::Mount),
            DriveRequest::UnmountCatalog { catalog } => (catalog.clone(), Action::Unmount),
            _ => unreachable!("catalog control request"),
        };
        let refusal = if matches!(action, Action::Mount) && self.children.contains_key(&id) {
            Some("Catalog is already mounted")
        } else if matches!(action, Action::Mount) && self.children.len() >= 8 {
            Some("Eight catalog mounts are already active; unmount one before adding another")
        } else {
            None
        };
        if let Some(error) = refusal {
            self.finish_operation(&envelope.id, &envelope.cancel, Err(error.into()));
            return;
        }
        self.launch_catalog_control(id, action, Some(envelope));
    }
    fn launch_catalog_control(&mut self, id: String, action: Action, envelope: Option<Envelope>) {
        let work = Work {
            state: self.state.clone(),
            id: id.clone(),
            action,
            operation: envelope.as_ref().map(|envelope| envelope.id.clone()),
            server: self.children.remove(&id),
            cancel: envelope
                .as_ref()
                .map_or_else(|| self.closing.clone(), |e| e.cancel.clone()),
        };
        let task = write_task(&self.catalog_mounts.jobs, move || Ok(work.run()));
        self.catalog_mounts.pending = Some(Pending { id, envelope, task });
    }
    fn finish_catalog_control(&mut self) {
        let Some(delivered) = self
            .catalog_mounts
            .pending
            .as_ref()
            .and_then(|job| job.task.poll())
        else {
            return;
        };
        let job = self
            .catalog_mounts
            .pending
            .take()
            .expect("delivered catalog control");
        let result = write_result(delivered).and_then(|outcome| {
            if let Some(server) = outcome.server {
                self.children.insert(job.id.clone(), server);
            }
            outcome.result
        });
        if let Some(envelope) = job.envelope {
            self.finish_operation(&envelope.id, &envelope.cancel, result);
        } else {
            if let Err(error) = result {
                tracing::warn!(catalog = job.id, %error, "Catalog mount cleanup remains incomplete");
            }
            self.request_refresh(false);
            let _ = self.publish_policy(false);
        }
    }
    pub(super) fn poll_catalog_controls(&mut self) {
        self.finish_catalog_control();
        if self.catalog_mounts.busy()
            || self.maintenance.busy()
            || self.closing.load(Ordering::Acquire)
        {
            return;
        }
        if let Some(id) = self
            .children
            .iter_mut()
            .find_map(|(id, server)| matches!(server.0.try_wait(), Ok(Some(_))).then(|| id.clone()))
        {
            self.launch_catalog_control(id, Action::Recover, None);
            return;
        }
        if let Err(error) = self.recover_catalog_mount() {
            tracing::warn!(%error, "Catalog startup cleanup remains incomplete");
        }
    }
    fn recover_catalog_mount(&mut self) -> Result<()> {
        if self.catalog_mounts.after.is_none() {
            return Ok(());
        }
        // Mount inventory is descriptor-free namespace metadata. Never probe a
        // saved target here: a dead FUSE target may block even a metadata call.
        let mounts = crate::mounts::inventory()?;
        for _ in 0..32 {
            let Some(after) = &self.catalog_mounts.after else {
                return Ok(());
            };
            let ids = match self.db.ids("catalog", after, 1) {
                Ok(ids) => ids,
                Err(error) => {
                    self.catalog_mounts.after = None;
                    return Err(error);
                }
            };
            let Some(id) = ids.into_iter().next() else {
                self.catalog_mounts.after = None;
                return Ok(());
            };
            self.catalog_mounts.after = Some(id.clone());
            let catalog = self
                .db
                .get::<Catalog>("catalog", &id)?
                .ok_or("Catalog disappeared during startup cleanup")?;
            let expected = self.state.join("mounts").join(&id);
            let registered = self
                .db
                .get::<serde_json::Value>("catalog-mount", &id)?
                .is_some();
            if !self.children.contains_key(&id)
                && (catalog.mount.is_some()
                    || registered
                    || mounts.iter().any(|mount| mount.target == expected))
            {
                self.launch_catalog_control(id, Action::Recover, None);
                return Ok(());
            }
        }
        Ok(())
    }
    pub(super) fn drain_catalog_controls(&mut self) {
        while self.catalog_mounts.busy() {
            self.finish_catalog_control();
            std::thread::sleep(Duration::from_millis(20));
        }
        for id in self.children.keys().cloned().collect::<Vec<_>>() {
            self.launch_catalog_control(id, Action::Stop, None);
            while self.catalog_mounts.busy() {
                self.finish_catalog_control();
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
