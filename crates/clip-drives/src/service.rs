//! One retained storage owner independent from GUI clients and clipboard workers.
use crate::{backup, catalog, root::Root, scan, store::Store, sync, watch::Watch, *};
use little_clip_core::{kernel_interface, process::CommandRequest};
use std::{
    collections::HashMap,
    fs::{self, File},
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

type Inventory = Arc<dyn Fn() -> Result<Vec<Drive>> + Send + Sync>;
type Tokens = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;
struct Envelope {
    id: String,
    request: DriveRequest,
    cancel: Arc<AtomicBool>,
    review: Option<preflight::Review>,
}
#[derive(Clone)]
pub struct Service {
    sender: mpsc::SyncSender<Envelope>,
    shared: Arc<Mutex<DriveStatus>>,
    state: PathBuf,
    runtime: Arc<lifetime::Runtime>,
    inventory: Inventory,
    previews: preflight::Previews,
}
mod admission;
mod background;
mod catalog_mounts;
mod configuration;
mod image_controls;
mod lifetime;
mod maintenance;
mod partition;
mod policy;
mod preference;
mod preflight;
mod publication;
mod ram_access;
mod ram_pending;
mod ram_save;
mod ram_sessions;
mod ram_setup;
mod reconciliation;
mod refresh;
mod rules;
mod scheduling;

struct Owner {
    closing: Arc<AtomicBool>,
    sync_pool: little_clip_core::jobs::BackgroundJobs,
    sync_jobs: HashMap<String, reconciliation::Pending>,
    sync_preparing: HashMap<String, reconciliation::preparation::Pending>,
    sync_waiting: std::collections::VecDeque<reconciliation::Intent>,
    sync_auto: std::collections::BTreeMap<String, reconciliation::Intent>,
    deferred: std::collections::VecDeque<Envelope>,
    sync_epoch: u64,
    background: HashMap<String, background::Pending>,
    maintenance: maintenance::Cleanup,
    publication: publication::Publication,
    refresh: refresh::Refresh,
    scheduling: scheduling::Probes,
    configuration: configuration::Configuration,
    preferences: preference::Preferences,
    settings_generation: u64,
    policy_waiting: std::collections::VecDeque<publication::Waiter>,
    partition: Option<partition::Pending>,
    partition_preparing: Option<partition::preparation::Pending>,
    partition_jobs: little_clip_core::jobs::BackgroundJobs,
    authority: Arc<partition::Authority>,
    previews: preflight::Previews,
    backup_jobs: little_clip_core::jobs::BackgroundJobs,
    inventory: Inventory,
    unidentified_ram: HashMap<String, Arc<Root>>,
    sync_roots: HashMap<String, (Arc<Root>, Arc<Root>)>,
    db: Store,
    state: PathBuf,
    config: DriveConfig,
    config_path: Option<PathBuf>,
    shared: Arc<Mutex<DriveStatus>>,
    tokens: Tokens,
    watch: Option<Watch>,
    _lock: File,
    children: HashMap<String, catalog_mounts::Server>,
    ram_children: HashMap<String, crate::ram::Mounted>,
    ram_setup: Option<ram_setup::Pending>,
    ram_access: Option<ram_access::Pending>,
    ram_recovery: ram_sessions::recovery::Recovery,
    catalog_mounts: catalog_mounts::Controls,
    // Keep the creating worker alive until every transferred RAM server has exited.
    ram_setup_jobs: little_clip_core::jobs::BackgroundJobs,
}

impl Service {
    /// Starts the bounded storage coordinator and recovers unfinished operation records.
    ///
    /// # Errors
    /// Returns directory, database, singleton-lock or thread startup failures.
    pub fn start(config: DriveConfig, state: PathBuf) -> Result<Self> {
        let path = std::path::absolute(little_clip_core::config::find_config_file_path())?;
        Self::start_with_settings(config, state, Arc::new(discovery::discover), Some(path))
    }
    /// Starts an owned directory fixture without discovering real block devices.
    #[cfg(feature = "test-support")]
    pub fn start_fixture(config: DriveConfig, state: PathBuf, source: PathBuf) -> Result<Self> {
        let source = Root::open(&source)?.path;
        Self::start_with_inventory(
            config,
            state,
            Arc::new(move || {
                if !source.exists() {
                    return Ok(Vec::new());
                }
                Ok(vec![Drive {
                    id: "part:portable-test".into(),
                    label: "Portable test drive".into(),
                    device: "/dev/little-clip-owned-fixture".into(),
                    mount: Some(source.clone()),
                    part_uuid: Some("portable-test".into()),
                    filesystem_uuid: Some("fixture".into()),
                    filesystem: "vfat".into(),
                    capacity: 2 * 1024 * 1024,
                    drive_capacity: 2 * 1024 * 1024,
                    disk_sequence: Some(1),
                    partition_layout: None,
                    removable: true,
                    read_only: false,
                    evidence: "owned-directory-fixture".into(),
                    ambiguous: false,
                }])
            }),
        )
    }
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn start_with_inventory(
        config: DriveConfig,
        state: PathBuf,
        inventory: Inventory,
    ) -> Result<Self> {
        Self::start_with_settings(config, state, inventory, None)
    }
    fn start_with_settings(
        config: DriveConfig,
        state: PathBuf,
        inventory: Inventory,
        config_path: Option<PathBuf>,
    ) -> Result<Self> {
        Self::start_with_authority(
            config,
            state,
            inventory,
            config_path,
            Arc::new(partition::Authority::default()),
            publication::Publication::default(),
        )
    }
    fn start_with_authority(
        config: DriveConfig,
        state: PathBuf,
        inventory: Inventory,
        config_path: Option<PathBuf>,
        authority: Arc<partition::Authority>,
        publication: publication::Publication,
    ) -> Result<Self> {
        policy::validate_config(&config)?;
        let root = Root::create(&state)?;
        fs::set_permissions(&root.path, fs::Permissions::from_mode(0o700))?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.path.join("service.lock"))?;
        // SAFETY: flock operates on a live owned descriptor, without pointers.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another drive coordinator owns this state directory".into());
        }
        let db = Store::open(&root.path.join("drives.sqlite"))?;
        let mut config = config;
        // In production the settings file is authoritative, including an unset
        // destination. Fixture hosts have no settings file and retain SQLite preferences.
        if config_path.is_none() && config.backup_directory.is_none() {
            config.backup_directory = db
                .get::<DriveConfig>("settings", "config")?
                .and_then(|c| c.backup_directory);
        }
        let shared = Arc::new(Mutex::new(DriveStatus {
            config: config.clone(),
            backup_directory: config.backup_directory.clone(),
            discovery_error: Some(refresh::INITIAL_DISCOVERY.into()),
            ram_unavailable: Some("Checking RAM storage availability".into()),
            ..Default::default()
        }));
        let tokens = Arc::new(Mutex::new(HashMap::new()));
        let (sender, receiver) = mpsc::sync_channel(16);
        let closing = Arc::new(AtomicBool::new(false));
        let previews = Arc::new(Mutex::new(HashMap::new()));
        let mut owner = Owner {
            closing: closing.clone(),
            sync_pool: little_clip_core::jobs::BackgroundJobs::new_named(4, 4, "drive-sync"),
            sync_jobs: HashMap::new(),
            sync_preparing: HashMap::new(),
            sync_waiting: Default::default(),
            sync_auto: Default::default(),
            deferred: Default::default(),
            sync_epoch: 0,
            background: HashMap::new(),
            maintenance: Default::default(),
            publication: publication.with_inventory(inventory.clone()),
            refresh: refresh::Refresh::new(inventory.clone()),
            scheduling: scheduling::Probes::default(),
            configuration: Default::default(),
            preferences: Default::default(),
            settings_generation: 0,
            policy_waiting: Default::default(),
            partition: None,
            partition_preparing: None,
            partition_jobs: little_clip_core::jobs::BackgroundJobs::new_named(
                1,
                1,
                "drive-partition",
            ),
            authority,
            previews: previews.clone(),
            backup_jobs: little_clip_core::jobs::BackgroundJobs::new_named(
                little_clip_core::drives::MAX_CATALOG_WORKERS,
                little_clip_core::drives::MAX_CATALOG_WORKERS,
                "drive-backup",
            ),
            inventory: inventory.clone(),
            unidentified_ram: HashMap::new(),
            sync_roots: HashMap::new(),
            db,
            state: root.path.clone(),
            config,
            config_path,
            shared: shared.clone(),
            tokens: tokens.clone(),
            watch: Watch::new().ok(),
            _lock: lock,
            children: HashMap::new(),
            ram_children: HashMap::new(),
            ram_setup: None,
            ram_access: None,
            ram_recovery: Default::default(),
            catalog_mounts: Default::default(),
            ram_setup_jobs: little_clip_core::jobs::BackgroundJobs::new_named(
                1,
                1,
                "drive-ram-setup",
            ),
        };
        rules::migrate(&owner.db)?;
        catalog::redaction::migrate(&owner.db)?;
        owner.recover()?;
        owner.request_refresh(true);
        owner.publish_policy(false)?;
        let thread = std::thread::Builder::new()
            .name("portable-drive-owner".into())
            .spawn(move || owner.run(receiver))?;
        Ok(Self {
            sender,
            shared,
            state: root.path,
            inventory,
            previews,
            runtime: Arc::new(lifetime::Runtime {
                closing,
                tokens,
                thread: Mutex::new(Some(thread)),
            }),
        })
    }
    /// Admits an operation or reads status, policy and backup-review metadata.
    ///
    /// # Errors
    /// Returns queue, database or validation failures. Accepted writes retain ownership.
    pub fn request(&self, request: DriveRequest) -> Result<DriveReply> {
        if let DriveRequest::PreviewBackup {
            drive,
            kind,
            destination,
        } = request
        {
            return self.preview_backup(&drive, kind, destination);
        }
        if let DriveRequest::DiscardBackupReview { token } = request {
            self.previews
                .lock()
                .map_err(|_| "Backup review storage unavailable")?
                .remove(&token);
            return Ok(DriveReply::Done(
                "Backup review cancelled; no backup started".into(),
            ));
        }
        if let DriveRequest::Status { offset } = request {
            return self.status(offset).map(|s| DriveReply::Status(Box::new(s)));
        }
        if let DriveRequest::GetRule { drive, relative } = request {
            let drive = rules::drive((self.inventory)()?, &drive)?;
            let db = Store::existing(&self.state.join("drives.sqlite"), true)?;
            return Ok(DriveReply::Rule(rules::get(&db, &drive, &relative)?));
        }
        if let DriveRequest::Cancel { operation } = request {
            let tokens = self
                .runtime
                .tokens
                .lock()
                .map_err(|_| "Drive cancellation state unavailable")?;
            tokens
                .get(&operation)
                .ok_or("Operation has already finished")?
                .store(true, Ordering::Release);
            return Ok(DriveReply::Done(
                "Cancellation requested; completed writes will be retained".into(),
            ));
        }
        self.admit(uuid::Uuid::new_v4(), request)
    }

    fn status(&self, offset: usize) -> Result<DriveStatus> {
        let mut status = self
            .shared
            .lock()
            .map_err(|_| "Drive status unavailable")?
            .clone();
        let db = Store::existing(&self.state.join("drives.sqlite"), true)?;
        let transaction = db.0.unchecked_transaction()?;
        status.pairs = db.list("pair", offset, 100)?;
        status.ram = db.list("ram", offset, 100)?;
        for session in &mut status.ram {
            let mut pending = ram_pending::snapshot(&db, &session.id)?;
            pending.full_check_required |= !session.ready
                || session.last_flushed.is_none()
                || status.ram_pending_error.is_some();
            session.pending = Some(pending);
            let pair: Option<SyncPair> = db.get("pair", &session.id)?;
            session.connected = pair.as_ref().is_some_and(|pair| {
                status.drives.iter().any(|drive| {
                    drive.id == pair.drive
                        && drive.evidence == pair.evidence
                        && !drive.ambiguous
                        && drive.mount.is_some()
                })
            });
        }
        status.operations = db.list("operation", offset, 100)?;
        status.catalogs = db.list("catalog", offset, 100)?;
        status.conflicts = db.list("conflict", offset, 100)?;
        status.rules = db.list("rule", offset, 100)?;
        status.backups = db.list("backup", offset, 100)?;
        status.recovery = db.list("recovery", offset, 100)?;
        if [
            status.pairs.len(),
            status.ram.len(),
            status.operations.len(),
            status.catalogs.len(),
            status.conflicts.len(),
            status.rules.len(),
            status.backups.len(),
            status.recovery.len(),
        ]
        .contains(&100)
        {
            status.next_offset = Some(offset + 100);
        }
        for session in &mut status.ram {
            if let Some(backing) = &mut session.backing {
                backing.mounted &=
                    crate::mounts::owned_mount(&session.path, "little-clip-ram", &session.id)
                        .is_ok_and(|mount| mount.is_some_and(|m| m.id == session.identity.mount));
            }
            let evidence = db
                .get::<SyncPair>("pair", &session.id)?
                .map(|pair| pair.evidence);
            session.connected = status.drives.iter().any(|d| {
                d.id == session.drive
                    && d.mount.is_some()
                    && !d.ambiguous
                    && evidence.as_ref() == Some(&d.evidence)
            });
        }
        transaction.commit()?;
        Ok(status)
    }
}
fn title(request: &DriveRequest) -> &'static str {
    match request {
        DriveRequest::MeasureStorage => "Measure local metadata",
        DriveRequest::CopyToRam { .. } => "Copy to RAM",
        DriveRequest::Backup {
            kind: BackupKind::Files,
            ..
        } => "Back up files",
        DriveRequest::ReadOnlyBackup { .. } => "Back up files read-only",
        DriveRequest::Backup { .. } => "Back up partition image",
        DriveRequest::Catalog { .. } | DriveRequest::CatalogWithLimits { .. } => {
            "Save contents catalog"
        }
        DriveRequest::Pair { .. } => "Enroll folder sync",
        DriveRequest::Sync { .. } | DriveRequest::FlushRam { .. } => "Synchronize",
        DriveRequest::SaveRam { .. } => "Save RAM copy",
        _ => "Drive settings",
    }
}
impl Owner {
    fn recover(&mut self) -> Result<()> {
        ram_pending::recover(&self.db)?;
        let mut offset = 0;
        loop {
            let operations: Vec<DriveOperation> = self.db.list("operation", offset, 128)?;
            if operations.is_empty() {
                break;
            }
            for mut op in operations {
                if op.finished.is_none() {
                    op.state = "interrupted".into();
                    op.message = op.partition.as_ref().map(PartitionPreparation::interruption_message)
                        .unwrap_or_else(|| "Daemon restarted; completed writes retained. Reconcile before retrying.".into());
                    op.message = preference::annotate(&self.db, &op.id, Ok(op.message))
                        .unwrap_or_else(|error| error.to_string());
                    op.finished = Some(now());
                    self.db.put("operation", &op.id, &op)?;
                }
            }
            offset += 128;
        }
        for mut session in policy::records::<RamSession>(&self.db, "ram")? {
            if let Some(backing) = &mut session.backing {
                backing.mounted = false;
                session.status = "RAM image retained; waiting for filesystem recovery".into();
                self.db.put("ram", &session.id, &session)?;
            }
        }
        Ok(())
    }
    fn run(&mut self, receiver: mpsc::Receiver<Envelope>) {
        let mut discovered = Instant::now();
        let mut synchronized = Instant::now() - Duration::from_secs(3600);

        loop {
            preflight::expire(&self.previews);
            if self.closing.load(Ordering::Acquire) {
                break;
            }
            // Retained admission adds a read phase before copying. Poll those
            // short phases promptly instead of adding an idle 200 ms per phase.
            let poll_ms = if self.preparation_pending() { 20 } else { 200 };
            match receiver.recv_timeout(Duration::from_millis(poll_ms)) {
                Ok(envelope) => self.operation(envelope),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
            self.poll_configuration();
            self.poll_preference();
            self.poll_scopes();
            self.poll_refresh();
            self.poll_policy();
            if discovered.elapsed() >= Duration::from_secs(5) {
                self.request_refresh(false);
                let _ = self.publish_policy(false);
                discovered = Instant::now();
            }
            self.poll_background();
            self.poll_catalog_controls();
            self.maintenance.poll(
                &self.backup_jobs,
                &self.db,
                &self.state,
                &self.closing,
                self.background.is_empty()
                    && !self.partition_backup_pending()
                    && !self.catalog_mounts.busy(),
            );
            self.poll_partition();
            self.poll_ram_setup();
            self.poll_ram_access();
            self.poll_ram_recovery();
            self.poll_sync();
            for envelope in std::mem::take(&mut self.deferred) {
                self.operation(envelope);
            }
            let periodic = synchronized.elapsed()
                >= Duration::from_secs(self.config.sync_interval_seconds.max(1));
            let changes = self.watch_changes(periodic);
            if changes.full {
                // Lost events invalidate the observation epoch, even on precise filesystems.
                self.sync_roots.clear();
                self.sync_epoch = self.sync_epoch.wrapping_add(1);
            }
            if periodic || changes.full {
                self.automatic_changed(&changes, true);
                synchronized = Instant::now();
            } else if !changes.paths.is_empty() {
                self.automatic_changed(&changes, false);
            }
            self.reap_ram();
        }
        self.closing.store(true, Ordering::Release);
        if let Ok(mut previews) = self.previews.lock() {
            previews.clear();
        }
        let mut stopping = std::mem::take(&mut self.deferred);
        stopping.extend(receiver.try_iter());
        for envelope in stopping {
            envelope.cancel.store(true, Ordering::Release);
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err("Daemon stopping; queued operation cancelled before execution".into()),
            );
        }
        self.drain_configuration();
        self.drain_scopes();
        self.drain_partition_preparation();
        self.drain_ram_preparation();
        self.drain_ram_recovery();
        self.drain_sync();
        drop(self.watch.take()); // Drain actual watch-worker pins before unmount/helper retirement.
        while let Some(job) = &self.partition {
            job.cancel.store(true, Ordering::Release);
            self.poll_partition();
            std::thread::sleep(Duration::from_millis(20));
        }
        // Retain accepted bulk ownership and the singleton lock until actual completion.
        while !self.background.is_empty() {
            for job in self.background.values() {
                job.cancel.store(true, Ordering::Release);
            }
            self.poll_background();
            std::thread::sleep(Duration::from_millis(20));
        }
        while self.maintenance.busy() {
            self.maintenance.poll(
                &self.backup_jobs,
                &self.db,
                &self.state,
                &self.closing,
                false,
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        self.ram_children.clear();
        self.mark_ram_unmounted();
        self.drain_catalog_controls();
        // Retain all status probes until actual exit, then observe the final
        // post-cleanup state before publishing exclusions and releasing ownership.
        self.request_refresh(true);
        while self.refresh.busy() {
            self.poll_refresh();
            std::thread::sleep(Duration::from_millis(20));
        }
        // Policy observations own temporary media pins until their actual exit.
        // Shutdown also installs the latest retained exclusions before releasing
        // the singleton, including startup followed immediately by shutdown.
        let _ = self.publish_policy(false);
        while self.publication.busy() {
            self.poll_policy();
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn preparation_pending(&self) -> bool {
        !self.sync_preparing.is_empty()
            || !self.scheduling.pending.is_empty()
            || self.ram_access.is_some()
            || self.ram_recovery.busy()
            || self.catalog_mounts.busy()
            || self.partition_preparing.is_some()
            || self.settings_busy()
            || self.watch.as_ref().is_some_and(Watch::registration_pending)
    }
    pub(super) fn settings_busy(&self) -> bool {
        self.configuration.busy() || self.preferences.writing()
    }
    fn operation(&mut self, envelope: Envelope) {
        if envelope.cancel.load(Ordering::Acquire) {
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err("Cancelled before operation setup".into()),
            );
            return;
        }
        if matches!(envelope.request, DriveRequest::Rule { .. }) {
            self.rule_operation(envelope);
            return;
        }
        if let Err(error) = self.validate_backup_request(&envelope) {
            self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
            return;
        }
        // State measurements and retirement do not need a source-drive pin.
        if matches!(
            envelope.request,
            DriveRequest::PruneBackups { .. }
                | DriveRequest::MeasureStorage
                | DriveRequest::MountImage { .. }
                | DriveRequest::UnmountImage { .. }
        ) {
            self.start_background(envelope, None);
            return;
        }
        if self.ram_setup_blocks(&envelope.request) {
            self.defer_operation(envelope);
            return;
        }
        self.start_scopes(envelope);
    }
    fn dispatch_scoped(
        &mut self,
        envelope: Envelope,
        drive: Option<String>,
        claims: Vec<crate::root::Scope>,
        source: Option<scheduling::Source>,
        selected: Option<Drive>,
    ) {
        // A probe may have started before another RAM setup acquired the slot.
        if self.ram_setup_blocks(&envelope.request) {
            self.defer_operation(envelope);
            return;
        }
        let busy = drive
            .as_ref()
            .is_some_and(|drive| self.check_busy(drive).is_err())
            || self.claims_busy(&claims)
            || self.sync_waiting.iter().any(|intent| {
                self.db
                    .get::<SyncPair>("pair", &intent.pair)
                    .ok()
                    .flatten()
                    .is_some_and(|pair| drive.as_ref() == Some(&pair.drive))
            });
        if busy {
            if matches!(envelope.request, DriveRequest::Pause { paused: true, .. }) {
                for intent in self
                    .sync_jobs
                    .values()
                    .filter(|job| drive.as_ref() == Some(&job.drive))
                    .map(|job| &job.intent)
                    .chain(
                        self.sync_preparing
                            .values()
                            .filter(|job| drive.as_ref() == Some(&job.drive))
                            .map(|job| &job.intent),
                    )
                    .filter(|intent| intent.operation.is_none())
                {
                    intent.cancel.store(true, Ordering::Release);
                }
            }
            self.defer_operation(envelope);
            return;
        }
        match &envelope.request {
            DriveRequest::Configure { .. } => self.start_configuration(envelope),
            DriveRequest::MountCatalog { .. } | DriveRequest::UnmountCatalog { .. } => {
                self.start_catalog_control(envelope)
            }
            DriveRequest::BackupDirectory { .. } => {
                self.start_preference(envelope, preference::Continuation::Explicit, None, claims)
            }
            DriveRequest::CopyToRam { .. } => self.start_ram_setup(envelope, source),
            DriveRequest::Pair { .. } => self.start_enrollment(envelope, source, claims),
            DriveRequest::Sync { .. }
            | DriveRequest::FlushRam { .. }
            | DriveRequest::SaveRam { .. } => self.start_reconciliation(envelope, claims),
            DriveRequest::Eject { .. }
            | DriveRequest::Backup {
                kind: BackupKind::Image,
                ..
            }
            | DriveRequest::ReadOnlyBackup { .. } => {
                self.start_partition(envelope, selected, claims)
            }
            DriveRequest::Backup { .. }
            | DriveRequest::Catalog { .. }
            | DriveRequest::CatalogWithLimits { .. } => self.start_background(envelope, source),
            _ => {
                let id = &envelope.id;
                let result = self.execute_recorded(id, envelope.request, &envelope.cancel);
                self.finish_operation(id, &envelope.cancel, result);
            }
        }
    }
    fn execute_recorded(
        &mut self,
        id: &str,
        request: DriveRequest,
        cancel: &AtomicBool,
    ) -> Result<String> {
        let mut op = self
            .db
            .get::<DriveOperation>("operation", id)?
            .ok_or("Accepted storage operation is missing")?;
        op.state = "running".into();
        self.db.put("operation", id, &op)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.execute(request, cancel)
        }));
        result.unwrap_or_else(|_| Err("Storage worker failed; completed writes retained".into()))
    }
    fn defer_operation(&mut self, envelope: Envelope) {
        if self.deferred.len() < 16 {
            self.deferred.push_back(envelope);
        } else {
            self.finish_operation(
                &envelope.id,
                &envelope.cancel,
                Err("Drive queue is full; retry admission".into()),
            );
        }
    }
    fn drive(&self, id: &str) -> Result<Drive> {
        discovery::select((self.inventory)()?, id)
    }

    fn execute(&mut self, request: DriveRequest, cancel: &AtomicBool) -> Result<String> {
        cancelled(cancel)?;
        match request {
            DriveRequest::Eject { .. } => {
                Err("Eject must use retained pre-unmount admission".into())
            }
            DriveRequest::MountImage { .. } | DriveRequest::UnmountImage { .. } => {
                Err("Image mounts require retained control admission".into())
            }
            DriveRequest::PruneBackups { .. } => {
                Err("Backup retirement requires retained worker admission".into())
            }
            DriveRequest::MeasureStorage => {
                Err("Metadata measurement requires retained worker admission".into())
            }
            DriveRequest::Configure { .. } => {
                Err("Configuration requires retained settings preparation".into())
            }
            DriveRequest::BackupDirectory { .. } => {
                Err("Backup preferences require retained write admission".into())
            }
            DriveRequest::CopyToRam { .. } => {
                Err("RAM setup must use retained reconciliation admission".into())
            }
            DriveRequest::Backup { .. }
            | DriveRequest::ReadOnlyBackup { .. }
            | DriveRequest::Catalog { .. }
            | DriveRequest::CatalogWithLimits { .. }
            | DriveRequest::PreviewBackup { .. }
            | DriveRequest::StartBackup { .. }
            | DriveRequest::StartReadOnlyBackup { .. }
            | DriveRequest::DiscardBackupReview { .. } => {
                Err("Backup and catalog work must use retained background admission".into())
            }
            DriveRequest::Pair { .. }
            | DriveRequest::Sync { .. }
            | DriveRequest::FlushRam { .. } => {
                Err("Reconciliation must use retained task admission".into())
            }
            DriveRequest::Pause { pair, paused } => {
                let mut pair: SyncPair = self.db.get("pair", &pair)?.ok_or("Unknown sync pair")?;
                pair.paused = paused;
                self.db.put("pair", &pair.id, &pair)?;
                Ok(if paused {
                    "Automatic sync paused"
                } else {
                    "Automatic sync resumed"
                }
                .into())
            }
            DriveRequest::Deletions { pair, enabled } => {
                let mut pair: SyncPair = self.db.get("pair", &pair)?.ok_or("Unknown sync pair")?;
                if enabled && pair.last_success.is_none() {
                    return Err("Complete the initial union sync before enabling deletions".into());
                }
                pair.deletions = enabled;
                self.db.put("pair", &pair.id, &pair)?;
                Ok("Deletion policy updated".into())
            }
            DriveRequest::MountCatalog { .. } | DriveRequest::UnmountCatalog { .. } => {
                Err("Catalog mounts require retained control admission".into())
            }
            DriveRequest::Rule {
                drive,
                relative,
                catalog,
                index,
            } => {
                rules::get(&self.db, &drive, &relative)?;
                let id = rule_id(&drive, &relative);
                let rule = DriveRule {
                    id: id.clone(),
                    drive,
                    relative,
                    catalog,
                    index,
                };
                catalog::redaction::set_rule(&self.db, &rule)?;
                self.publish_policy(true)?;
                Ok(if catalog {
                    "Catalog and indexing policy updated; saved catalogs retain their capture date"
                } else {
                    "Catalog exclusion saved; catalog status shows metadata cleanup progress and errors. Existing views retain their snapshot until unmounted"
                }.into())
            }
            DriveRequest::SaveRam { .. } => {
                Err("RAM saves require retained worker admission".into())
            }
            DriveRequest::DiscardRam { session, confirmed } => {
                if !confirmed {
                    return Err(
                        "Confirm discarding the RAM copy, including any unsynchronized edits"
                            .into(),
                    );
                }
                let session: RamSession =
                    self.db.get("ram", &session)?.ok_or("Unknown RAM session")?;
                self.discard_ram(&session, cancel)?;
                self.db.remove("pair", &session.id)?;
                self.db.remove("ram", &session.id)?;
                self.db
                    .0
                    .execute("DELETE FROM ram_pending WHERE pair=?1", [&session.id])?;
                self.unidentified_ram.remove(&session.id);
                Ok("RAM copy discarded".into())
            }
            DriveRequest::Resolve { conflict } => {
                let mut conflict: DriveConflict = self
                    .db
                    .get("conflict", &conflict)?
                    .ok_or("Unknown conflict")?;
                if !conflict.complete {
                    return Err(
                        "Finish preserving both conflict variants before acknowledging it".into(),
                    );
                }
                conflict.resolved = true;
                self.db.put("conflict", &conflict.id, &conflict)?;
                Ok("Conflict acknowledged; preserved variants remain available".into())
            }
            DriveRequest::Status { .. }
            | DriveRequest::Cancel { .. }
            | DriveRequest::GetRule { .. } => Err("Request must use coordinator admission".into()),
        }
    }
    fn automatic(&mut self) {
        let mut changes = crate::watch::Changes::default();
        changes.full = true;
        self.automatic_changed(&changes, true);
    }
    fn automatic_changed(&mut self, changes: &crate::watch::Changes, periodic: bool) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let Ok(full) = self.track_ram_changes(Some(&changes.paths), changes.full) else {
            return;
        };
        let paths = if periodic || full {
            None
        } else {
            Some(&changes.paths)
        };
        let Ok(pairs) = policy::records::<SyncPair>(&self.db, "pair") else {
            return;
        };
        let shared_paths = paths.cloned().map(Arc::new);
        for pair in pairs {
            let initial = self
                .db
                .get::<RamSession>("ram", &pair.id)
                .ok()
                .flatten()
                .is_some_and(|s| !s.ready);
            if pair.paused && !(initial && paths.is_some()) {
                continue;
            }
            if let (Some(paths), Some((a, b))) = (paths, self.sync_roots.get(&pair.id))
                && !paths
                    .iter()
                    .any(|p| p.starts_with(&a.path) || p.starts_with(&b.path))
            {
                continue;
            }
            let selected_paths = if pair.ram
                && ram_pending::snapshot(&self.db, &pair.id)
                    .is_ok_and(|pending| pending.full_check_required)
            {
                None
            } else {
                shared_paths.clone()
            };
            self.queue_sync(reconciliation::Intent {
                queued: Instant::now(),
                pair: pair.id,
                operation: None,
                cancel: Arc::new(AtomicBool::new(false)),
                paths: selected_paths,
                initialize: false,
                save: None,
                writeback: None,
            });
        }
    }
    fn publish_policy(&mut self, fresh: bool) -> Result<()> {
        let input = self
            .policy_input()
            .unwrap_or_else(|error| publication::Input {
                paths: vec![self.state.clone()],
                ram: Vec::new(),
                drives: Vec::new(),
                rules: Vec::new(),
                mount_signature: None,
                metadata_error: Some(error.to_string()),
            });
        self.publication.request(&self.state, input, fresh)?;
        Ok(())
    }
    fn policy_input(&self) -> Result<publication::Input> {
        let transaction = self.db.0.unchecked_transaction()?;
        image_controls::policy_ready(&self.db)?;
        let mut excluded = preflight::managed_backup_paths(&self.db)?;
        excluded.extend(
            self.partition
                .as_ref()
                .and_then(|job| job.destination.as_ref().map(|root| root.path.clone())),
        );
        excluded.extend(
            self.background
                .values()
                .filter_map(|job| job.destination.clone()),
        );
        self.db
            .visit::<ram_setup::Allocation>("ram-allocation", |allocation| {
                if let Some(backing) = allocation.backing {
                    excluded.push(backing.directory);
                }
                Ok(())
            })?;
        if let Some(path) = &self.config.backup_directory {
            excluded.push(path.clone());
        }
        let mut ram = Vec::new();
        for session in policy::records::<RamSession>(&self.db, "ram")? {
            ram.push(session.id);
            excluded.push(session.path);
            if let Some(backing) = session.backing {
                excluded.push(backing.directory);
            }
        }
        excluded.extend(policy::mounted_views(&self.db)?);
        excluded.push(self.state.clone());
        let drives = self
            .shared
            .lock()
            .map_err(|_| "Drive status unavailable")?
            .drives
            .clone();
        let mut rules = Vec::new();
        self.db.visit::<DriveRule>("rule", |rule| {
            rules.push(rule);
            Ok(())
        })?;
        transaction.commit()?;
        excluded.sort();
        excluded.dedup();
        Ok(publication::Input {
            paths: excluded,
            ram,
            drives,
            rules,
            mount_signature: None,
            metadata_error: None,
        })
    }
}
fn rule_id(drive: &str, relative: &Path) -> String {
    let relative: PathBuf = relative.components().collect();
    format!(
        "{drive}:{}",
        little_clip_core::path_address::display_path(&relative)
    )
}

fn write_task<T: Send + 'static>(
    pool: &little_clip_core::jobs::BackgroundJobs,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> little_clip_core::jobs::task::Task<Result<T>> {
    little_clip_core::jobs::task::Task::start_unbounded(pool, move |_| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
            .unwrap_or_else(|_| Err("Storage worker panicked; completed writes retained".into()))
    })
}
fn write_result<T>(
    result: std::result::Result<Result<T>, little_clip_core::JobError>,
) -> Result<T> {
    result.unwrap_or_else(|error| {
        Err(format!("Storage worker failed: {error}; completed writes retained").into())
    })
}
fn job_progress(state: PathBuf, id: String) -> impl FnMut(u64, u64) {
    let mut last = Instant::now() - Duration::from_secs(1);
    move |files, bytes| {
        if last.elapsed() < Duration::from_millis(250) {
            return;
        }
        last = Instant::now();
        if let Ok(db) = Store::existing(&state.join("drives.sqlite"), false)
            && let Ok(Some(mut operation)) = db.get::<DriveOperation>("operation", &id)
        {
            operation.files = files;
            operation.bytes = bytes;
            let _ = db.put("operation", &id, &operation);
        }
    }
}
fn available_memory() -> Result<u64> {
    let mem = fs::read_to_string("/proc/meminfo")?;
    let available = mem
        .lines()
        .find_map(|line| {
            line.strip_prefix("MemAvailable:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .ok_or("Cannot determine available RAM")?
        .saturating_mul(1024);
    Ok(available)
}
fn ram_reason(config: &DriveConfig) -> Result<()> {
    let available = available_memory()?;
    if available < config.ram_min_available {
        return Err("Insufficient available RAM before copying".into());
    }
    crate::ram::directory(config)?;
    crate::ram::helper()?;
    if !Path::new("/dev/fuse").exists() {
        return Err("RAM filesystem requires /dev/fuse".into());
    }
    for program in ["mke2fs", "debugfs", "e2fsck", "fusermount3"] {
        if little_clip_core::process::command_available(program).is_none() {
            return Err(format!("RAM filesystem requires the missing tool {program}").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    mod admission;
    mod backup_review;
    mod bulk_preparation;
    mod catalog_cleanup;
    mod catalog_controls;
    mod catalog_limits;
    mod catalog_workers;
    mod configuration;
    mod event_loss;
    mod guest_ram;
    mod image_controls;
    mod maintenance;
    mod managed_views;
    mod native_images;
    mod native_read_only;
    mod partition_preparation;
    mod policy_aliases;
    mod policy_coverage;
    mod preference;
    mod ram_access;
    mod ram_writeback;
    mod read_only;
    mod scope_admission;
    mod startup;
    mod status_refresh;
    mod storage;
    mod sync_preparation;
    mod watch_responsiveness;
    use super::*;
    #[test]
    fn backup_preference_is_lazy_preserves_settings_and_survives_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let state = temp.path().join("state");
        let inventory: Inventory = Arc::new(|| Ok(Vec::new()));
        let service = Service::start_with_settings(
            DriveConfig::default(),
            state.clone(),
            inventory.clone(),
            Some(path.clone()),
        )
        .unwrap();
        assert!(
            !path.exists(),
            "starting without using backups must not write preferences"
        );
        assert!(status(&service).backup_directory.is_none());
        std::fs::write(
            &path,
            "# keep preferences\n[drives]\nram_capacity_limit = 3000000000\n",
        )
        .unwrap();
        let destination = temp.path().join("backup");
        finish(
            &service,
            DriveRequest::BackupDirectory {
                path: destination.clone(),
            },
        );
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# keep preferences"));
        let config = little_clip_core::AppConfig::load_from_path(&path).unwrap();
        assert_eq!(config.drives.ram_capacity_limit, 3_000_000_000);
        assert_eq!(config.drives.backup_directory.as_ref(), Some(&destination));
        std::fs::write(&path, "invalid [").unwrap();
        let id = accept(
            &service,
            DriveRequest::BackupDirectory {
                path: temp.path().join("other-backup"),
            },
        );
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(
            operation
                .message
                .contains("Could not save backup directory preference")
        );
        assert_eq!(
            status(&service).backup_directory.as_ref(),
            Some(&destination)
        );
        std::fs::write(&path, saved).unwrap();
        service.shutdown().unwrap();
        let restarted =
            Service::start_with_settings(config.drives.clone(), state, inventory, Some(path))
                .unwrap();
        assert_eq!(status(&restarted).backup_directory, Some(destination));
        assert_eq!(status(&restarted).config.ram_capacity_limit, 3_000_000_000);
        let mut unset = status(&restarted).config;
        unset.backup_directory = None;
        finish(
            &restarted,
            DriveRequest::Configure {
                config: unset.clone(),
            },
        );
        assert!(status(&restarted).backup_directory.is_none());
        restarted.shutdown().unwrap();
        // A cleared file preference must not be resurrected by old SQLite settings.
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "# destination deliberately unset\n").unwrap();
        let db = Store::existing(&temp.path().join("state/drives.sqlite"), false).unwrap();
        db.put("settings", "config", &config.drives).unwrap();
        drop(db);
        let final_service = Service::start_with_settings(
            unset,
            temp.path().join("state"),
            Arc::new(|| Ok(Vec::new())),
            Some(path),
        )
        .unwrap();
        assert!(status(&final_service).backup_directory.is_none());
        final_service.shutdown().unwrap();
    }

    fn fixture_drive(path: &Path, id: &str) -> Drive {
        let device = path.with_extension("device");
        fs::write(&device, b"owned metadata-only node stand-in").unwrap();
        Drive {
            id: format!("part:{id}"),
            label: id.into(),
            device,
            mount: Some(path.into()),
            part_uuid: Some(id.into()),
            filesystem_uuid: Some(id.into()),
            filesystem: "vfat".into(),
            capacity: 1024 * 1024,
            drive_capacity: 1024 * 1024,
            disk_sequence: Some(1),
            partition_layout: None,
            removable: true,
            read_only: false,
            evidence: id.into(),
            ambiguous: false,
        }
    }
    #[test]
    fn a_panicking_write_keeps_its_effect_and_does_not_destroy_the_worker_pool() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("completed");
        let output = path.clone();
        let pool = little_clip_core::jobs::BackgroundJobs::new(1, 1);
        let task = write_task::<String>(&pool, move || {
            fs::write(output, b"completed before panic")?;
            panic!("owned fault injection");
        });
        let start = Instant::now();
        loop {
            if let Some(result) = task.poll() {
                assert!(result.unwrap().is_err());
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fs::read(&path).unwrap(), b"completed before panic");
        let next = write_task::<String>(&pool, || Ok("worker survived".into()));
        loop {
            if let Some(result) = next.poll() {
                assert_eq!(result.unwrap().unwrap(), "worker survived");
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn published_policy(service: &Service) -> IndexPolicy {
        serde_json::from_slice(&fs::read(service.state.join("index-policy.json")).unwrap()).unwrap()
    }
    fn status(service: &Service) -> DriveStatus {
        match service.request(DriveRequest::Status { offset: 0 }).unwrap() {
            DriveReply::Status(status) => *status,
            _ => panic!("status reply"),
        }
    }
    fn wait_operation(
        service: &Service,
        id: &str,
        predicate: impl Fn(&DriveOperation) -> bool,
    ) -> DriveOperation {
        let started = Instant::now();
        loop {
            if let Some(op) = status(service)
                .operations
                .into_iter()
                .find(|op| op.id == id && predicate(op))
            {
                return op;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "operation {id} did not reach its expected state"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait_contents(path: &Path, expected: &[u8]) {
        let started = Instant::now();
        while !fs::read(path).is_ok_and(|bytes| bytes == expected) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "independent writeback delayed for {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn wait_fuse_request(threads: impl Fn() -> Vec<libc::c_long>) -> (libc::c_long, String) {
        let started = Instant::now();
        loop {
            for tid in threads() {
                let wait =
                    fs::read_to_string(format!("/proc/self/task/{tid}/wchan")).unwrap_or_default();
                if wait.contains("request_wait_answer") || wait.contains("fuse") {
                    return (tid, wait);
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "Owned worker did not enter a kernel FUSE wait"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn worker_threads(prefix: &str) -> Vec<libc::c_long> {
        fs::read_dir("/proc/self/task")
            .unwrap()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                fs::read_to_string(entry.path().join("comm"))
                    .ok()?
                    .trim()
                    .starts_with(prefix)
                    .then(|| entry.file_name().to_string_lossy().parse().ok())
                    .flatten()
            })
            .collect()
    }
    fn finish_native_fixture(
        backing: RamBacking,
        id: String,
        native: crate::ram::Mounted,
        result: std::thread::Result<()>,
    ) {
        drop(native);
        crate::ram::discard(&backing, &id).unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }
    fn shutdown_while_held(
        service: &Service,
    ) -> (std::thread::JoinHandle<Result<()>>, mpsc::Receiver<()>) {
        let closing = service.clone();
        let (exited, completed) = mpsc::channel();
        let joining = std::thread::spawn(move || {
            let result = closing.shutdown();
            exited.send(()).unwrap();
            result
        });
        assert_shutdown_retained(service, &completed);
        (joining, completed)
    }
    fn finish_held_shutdown(
        (joining, completed): (std::thread::JoinHandle<Result<()>>, mpsc::Receiver<()>),
    ) {
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        joining.join().unwrap().unwrap();
    }
    fn assert_configuration_responsive(service: &Service) {
        let started = Instant::now();
        let result = completed(
            service,
            DriveRequest::Configure {
                config: status(service).config,
            },
        );
        assert_eq!(result.state, "complete", "{}", result.message);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    fn assert_shutdown_retained(service: &Service, finished: &mpsc::Receiver<()>) {
        let started = Instant::now();
        while !service.runtime.closing.load(Ordering::Acquire) {
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            finished.try_recv().is_err(),
            "Shutdown dropped live observer ownership"
        );
        let config = status(service).config;
        assert!(
            service
                .request(DriveRequest::Configure {
                    config: config.clone()
                })
                .is_err()
        );
        let competing = Service::start_with_inventory(
            config,
            service.state.clone(),
            Arc::new(|| Ok(Vec::new())),
        );
        assert!(
            matches!(competing, Err(error) if error.to_string().contains("Another drive coordinator"))
        );
    }
    fn assert_policy_paused(
        service: &Service,
        request: DriveRequest,
        previous: &IndexPolicy,
        reason: &str,
    ) {
        let failed = completed(service, request);
        assert_eq!(failed.state, "failed", "{}", failed.message);
        assert!(
            failed.message.contains("Background indexing paused"),
            "{}",
            failed.message
        );
        assert!(failed.message.contains(reason), "{}", failed.message);
        let paused = published_policy(service);
        assert!(paused.excluded.contains(&PathBuf::from("/")));
        assert_eq!(
            paused.context, previous.context,
            "Offline-owner observations remain behind the root exclusion"
        );
    }
    fn wait_ram(
        service: &Service,
        id: &str,
        predicate: impl Fn(&RamSession) -> bool,
    ) -> RamSession {
        let started = Instant::now();
        loop {
            let sessions = status(service).ram;
            if let Some(session) = sessions
                .iter()
                .find(|session| session.id == id && predicate(session))
            {
                return session.clone();
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "RAM session {id} did not reach its expected state: {sessions:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub(super) struct TestDrives {
        temp: tempfile::TempDir,
        drives: Arc<Mutex<Vec<Drive>>>,
    }
    impl TestDrives {
        fn saved_image(&self) -> DriveBackup {
            let state = Root::create(&self.temp.path().join("state")).unwrap();
            let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
            let destination = Root::create(&self.temp.path().join("saved-images")).unwrap();
            let input = File::open(self.temp.path().join("large/large-sparse")).unwrap();
            let mut drive = self.drives.lock().unwrap()[1].clone();
            drive.mount = None;
            drive.capacity = input.metadata().unwrap().len();
            backup::image_from(
                &db,
                &drive,
                &destination,
                &input,
                &AtomicBool::new(false),
                |_, _| Ok(()),
                || Ok(()),
            )
            .unwrap()
        }
        fn guest_source(&self) -> Option<tempfile::TempDir> {
            std::env::var_os("LITTLE_CLIP_DRIVE_TEST_SOURCE_DIR").map(|base| {
                crate::require_disposable_guest();
                let media = tempfile::tempdir_in(base).unwrap();
                let mut selected = self.drives.lock().unwrap();
                selected[1].mount = Some(media.path().into());
                selected[1].filesystem =
                    std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
                media
            })
        }
        pub(super) fn native_source(
            &self,
            relative: &str,
        ) -> (RamBacking, String, crate::ram::Mounted, PathBuf) {
            self.native_source_with(relative, |backing, id| {
                crate::ram::Mounted::start(backing, id, true, None).unwrap()
            })
        }
        fn native_source_with(
            &self,
            relative: &str,
            start: impl FnOnce(&RamBacking, &str) -> crate::ram::Mounted,
        ) -> (RamBacking, String, crate::ram::Mounted, PathBuf) {
            let base = crate::ram::directory(&DriveConfig::default()).unwrap();
            let backing = crate::ram::prepare(
                &base,
                &crate::ram::Budget {
                    bytes: 128 * 1024 * 1024,
                    inodes: 4096,
                },
                |_| Ok(()),
            )
            .unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let native = start(&backing, &id);
            let source = backing.directory.join("contents").join(relative);
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("edit"), b"original portable content").unwrap();
            self.drives.lock().unwrap()[1].mount = Some(source.clone());
            (backing, id, native, source)
        }
        pub(super) fn new(bytes: u64) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let small = temp.path().join("small");
            let large = temp.path().join("large");
            fs::create_dir(&small).unwrap();
            fs::create_dir(&large).unwrap();
            fs::write(small.join("edit"), b"initial").unwrap();
            File::create(large.join("large-sparse"))
                .unwrap()
                .set_len(bytes)
                .unwrap();
            let drives = Arc::new(Mutex::new(vec![
                fixture_drive(&small, "small"),
                fixture_drive(&large, "large"),
            ]));
            Self { temp, drives }
        }
        fn held_image(
            &self,
            restore_mounted: bool,
        ) -> (Service, String, mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (mut authority, entered, release) = self.held_unmount_authority();
            authority.open_image = Arc::new(|_, _| panic!("held image unexpectedly opened input"));
            let service = self.start_authority(None, authority);
            let id = accept(&service, image_request(restore_mounted));
            (service, id, entered, release)
        }
        fn image_authority(&self) -> partition::Authority {
            let image: backup::ImageAction =
                Arc::new(|_, _, _| Err("owned image mount authorization declined".into()));
            let action = |mounted: bool| -> partition::FilesystemAction {
                let drives = self.drives.clone();
                Arc::new(move |drive| {
                    let mut drives = drives.lock().unwrap();
                    let current = drives
                        .iter_mut()
                        .find(|candidate| candidate.device == drive.device)
                        .unwrap();
                    current.mount = if mounted { drive.mount.clone() } else { None };
                    current.read_only = mounted && drive.read_only;
                    Ok(())
                })
            };
            partition::Authority {
                unmount: action(false),
                mount: action(true),
                open_image: Arc::new(|_, _| Err("owned image authorization declined".into())),
                images: backup::ImageAuthority {
                    mount: image.clone(),
                    unmount: image,
                },
            }
        }
        fn held_unmount_authority(
            &self,
        ) -> (partition::Authority, mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (entered, release, gate) = worker_gate();
            let mut authority = self.image_authority();
            let unmount = authority.unmount.clone();
            authority.unmount = Arc::new(move |drive| {
                gate();
                unmount(drive)
            });
            (authority, entered, release)
        }
        fn pair(&self, service: &Service, drive: &str, relative: &str) -> PathBuf {
            let local = self.temp.path().join(format!("paired-{drive}"));
            finish(
                service,
                DriveRequest::Pair {
                    drive: format!("part:{drive}"),
                    relative: relative.into(),
                    local: local.clone(),
                },
            );
            local
        }
        fn start(
            &self,
            observed: Option<Arc<dyn Fn() + Send + Sync>>,
            unmounter: partition::FilesystemAction,
        ) -> Service {
            self.start_authority(
                observed,
                partition::Authority {
                    unmount: unmounter,
                    ..Default::default()
                },
            )
        }
        fn start_authority(
            &self,
            observed: Option<Arc<dyn Fn() + Send + Sync>>,
            authority: partition::Authority,
        ) -> Service {
            self.start_configured(
                DriveConfig {
                    ram_min_available: 0,
                    sync_interval_seconds: 3600,
                    backup_directory: Some(self.temp.path().join("backup")),
                    ..Default::default()
                },
                observed,
                authority,
            )
        }
        // Disable RAM allocation and the periodic fallback for read-stall probes.
        fn start_observed_reads(&self, observed: Arc<dyn Fn() + Send + Sync>) -> Service {
            self.start_configured(
                DriveConfig {
                    ram_min_available: u64::MAX,
                    sync_interval_seconds: 3600,
                    ..Default::default()
                },
                Some(observed),
                self.image_authority(),
            )
        }
        fn start_configured(
            &self,
            config: DriveConfig,
            observed: Option<Arc<dyn Fn() + Send + Sync>>,
            authority: partition::Authority,
        ) -> Service {
            self.start_with_publication(config, observed, authority, Default::default())
        }
        fn start_with_publication(
            &self,
            config: DriveConfig,
            observed: Option<Arc<dyn Fn() + Send + Sync>>,
            authority: partition::Authority,
            publisher: publication::Publication,
        ) -> Service {
            let drives = self.drives.clone();
            let service = Service::start_with_authority(
                config,
                self.temp.path().join("state"),
                Arc::new(move || {
                    if let Some(observe) = &observed {
                        observe();
                    }
                    Ok(drives.lock().unwrap().clone())
                }),
                None,
                Arc::new(authority),
                publisher,
            )
            .unwrap();
            let started = Instant::now();
            while status(&service).discovery_error.as_deref() == Some(refresh::INITIAL_DISCOVERY) {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            service
        }
    }
    fn two_drives(bytes: u64) -> (tempfile::TempDir, Service, PathBuf) {
        two_drives_observed(bytes, None)
    }
    fn two_drives_observed(
        bytes: u64,
        observed: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> (tempfile::TempDir, Service, PathBuf) {
        let fixture = TestDrives::new(bytes);
        let service = fixture.start(observed, Arc::new(partition::unmount));
        let small = fixture.temp.path().join("small");
        (fixture.temp, service, small)
    }
    #[test]
    fn subtree_enrollment_and_later_sync_refuse_links_outside_the_partition() {
        use std::os::unix::fs::symlink;
        let fixture = TestDrives::new(0);
        let source = fixture.temp.path().join("large");
        let outside = fixture.temp.path().join("host-data");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("edit"), b"host data must stay untouched").unwrap();
        symlink(&outside, source.join("escape")).unwrap();
        let service = fixture.start(None, Arc::new(partition::unmount));
        let refused = fixture.temp.path().join("must-not-create");
        failed(
            &service,
            DriveRequest::Pair {
                drive: "part:large".into(),
                relative: "escape".into(),
                local: refused.clone(),
            },
            "Symlink",
        );
        assert!(!refused.exists());
        fs::create_dir(source.join("selected")).unwrap();
        fs::write(source.join("selected/edit"), b"portable data").unwrap();
        let local = fixture.pair(&service, "large", "selected");
        let pair = status(&service).pairs.into_iter().next().unwrap();
        fs::rename(source.join("selected"), source.join("retained")).unwrap();
        symlink(&outside, source.join("selected")).unwrap();
        fs::write(local.join("edit"), b"local edit must not enter host data").unwrap();
        failed(&service, DriveRequest::Sync { pair: pair.id }, "Symlink");
        assert_eq!(
            fs::read(outside.join("edit")).unwrap(),
            b"host data must stay untouched"
        );
        assert_eq!(
            fs::read(source.join("retained/edit")).unwrap(),
            b"portable data"
        );
        assert_eq!(
            fs::read(local.join("edit")).unwrap(),
            b"local edit must not enter host data"
        );
        service.shutdown().unwrap();
    }

    #[test]
    fn sync_refuses_a_paired_folder_relocated_inside_its_source_tree() {
        use std::os::unix::fs::symlink;
        let fixture = TestDrives::new(0);
        let source = fixture.temp.path().join("large/selected");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("edit"), b"keep both trees").unwrap();
        let service = fixture.start(None, Arc::new(partition::unmount));
        let local = fixture.pair(&service, "large", "selected");
        let pair = status(&service).pairs.into_iter().next().unwrap();
        let moved = source.join("relocated");
        fs::rename(&local, &moved).unwrap();
        symlink(&moved, &local).unwrap();
        failed(&service, DriveRequest::Sync { pair: pair.id }, "overlap");
        assert!(!moved.join("relocated").exists());
        assert_eq!(fs::read(moved.join("edit")).unwrap(), b"keep both trees");
        service.shutdown().unwrap();
    }

    #[test]
    fn sync_rechecks_managed_storage_after_a_local_folder_moves() {
        use std::os::unix::fs::symlink;
        let fixture = TestDrives::new(0);
        let service = fixture.start(None, Arc::new(partition::unmount));
        let local = fixture.pair(&service, "large", "");
        let pair = status(&service).pairs.into_iter().next().unwrap();
        let backup = fixture.temp.path().join("backup");
        fs::create_dir(&backup).unwrap();
        let moved = backup.join("relocated-pair");
        fs::rename(&local, &moved).unwrap();
        symlink(&moved, &local).unwrap();
        fs::write(
            fixture.temp.path().join("large/new"),
            b"do not write into managed backups",
        )
        .unwrap();
        failed(&service, DriveRequest::Sync { pair: pair.id }, "managed");
        assert!(!moved.join("new").exists());
        service.shutdown().unwrap();
    }

    fn worker_gate() -> (
        mpsc::Receiver<()>,
        mpsc::Sender<()>,
        Arc<dyn Fn() + Send + Sync>,
    ) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let wait = Arc::new(move || {
            entered_tx.send(()).unwrap();
            release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(20))
                .unwrap();
        });
        (entered_rx, release_tx, wait)
    }
    fn worker_observer(
        prefix: &'static str,
        armed: Arc<AtomicBool>,
    ) -> (
        mpsc::Receiver<()>,
        mpsc::Sender<()>,
        Arc<dyn Fn() + Send + Sync>,
    ) {
        let (entered, release, gate) = worker_gate();
        let observe = Arc::new(move || {
            if std::thread::current()
                .name()
                .is_some_and(|name| name.starts_with(prefix))
                && armed.swap(false, Ordering::AcqRel)
            {
                gate();
            }
        });
        (entered, release, observe)
    }
    #[test]
    fn a_completed_owned_image_survives_remount_failure_with_a_terminal_warning() {
        let fixture = TestDrives::new(0);
        let original = fixture.drives.lock().unwrap()[1].clone();
        fixture.drives.lock().unwrap()[1].mount = None;
        let drive = fixture.drives.lock().unwrap()[1].clone();
        let state = Root::create(&fixture.temp.path().join("state")).unwrap();
        let db = Store::open(&state.path.join("drives.sqlite")).unwrap();
        let bytes = vec![29; drive.capacity as usize];
        let input_path = fixture.temp.path().join("owned-partition-bytes");
        fs::write(&input_path, &bytes).unwrap();
        let input = File::open(input_path).unwrap();
        let destination = Root::create(&fixture.temp.path().join("backup")).unwrap();
        let backup = backup::image_from(
            &db,
            &drive,
            &destination,
            &input,
            &AtomicBool::new(false),
            |_, _| Ok(()),
            || Ok(()),
        )
        .unwrap();
        let id = "owned-completed-image-job";
        let operation = DriveOperation {
            id: id.into(),
            title: "Owned image".into(),
            state: "running".into(),
            message: "published".into(),
            files: 1,
            bytes: drive.capacity,
            started: now(),
            finished: None,
            partition: Some(PartitionPreparation {
                file_mode: Default::default(),
                collection_mount: None,
                drive: original.clone(),
                destination: Some(destination.path.clone()),
                phase: PartitionPhase::ImagePublished,
                unmounted_by_job: true,
                restore_mounted: true,
                completed_backup: Some(backup.id.clone()),
                restore_error: None,
            }),
        };
        db.put("operation", id, &operation).unwrap();
        let context = partition::BackupContext {
            file_mode: Default::default(),
            read_only_mount: None,
            source: Arc::new(crate::device::Source::capture(&original).unwrap()),
            original,
            claims: Vec::new(),
            restore_mounted: true,
            unmounted_by_job: true,
        };
        let drives = fixture.drives.clone();
        let inventory: Inventory = Arc::new(move || Ok(drives.lock().unwrap().clone()));
        let mut authority = fixture.image_authority();
        authority.mount = Arc::new(|_| Err("owned remount refused".into()));
        let result = context.finish(
            Ok(&db),
            id,
            &inventory,
            &authority,
            Ok(format!("Backup completed: {}", backup.path.display())),
        );
        background::finish_record(&db, id, &AtomicBool::new(false), result).unwrap();
        let completed = db.get::<DriveOperation>("operation", id).unwrap().unwrap();
        assert_eq!(completed.state, "complete_with_warning");
        assert!(completed.finished.is_some());
        assert!(completed.message.contains("Backup completed"));
        assert!(completed.message.contains("owned remount refused"));
        let partition = completed.partition.unwrap();
        assert_eq!(partition.completed_backup, Some(backup.id.clone()));
        assert_eq!(partition.phase, PartitionPhase::MountRestoreFailed);
        assert!(partition.restore_error.is_some());
        assert_eq!(fs::read(&backup.path).unwrap(), bytes);
        assert!(
            db.get::<DriveBackup>("backup", &backup.id)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            db.get::<String>("backup-current", "part:large:Image")
                .unwrap(),
            Some(backup.id)
        );
    }
    #[test]
    fn legacy_image_requests_leave_restoration_off_and_file_backups_refuse_it() {
        let request: DriveRequest = serde_json::from_value(
            serde_json::json!({"Backup":{"drive":"part:large","kind":"Image"}}),
        )
        .unwrap();
        assert!(matches!(
            request,
            DriveRequest::Backup {
                restore_mounted: false,
                ..
            }
        ));
        let fixture = TestDrives::new(0);
        let service = fixture.start_authority(None, fixture.image_authority());
        let error = service
            .request(DriveRequest::Backup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                restore_mounted: true,
            })
            .unwrap_err();
        assert!(error.to_string().contains("requires a partition image"));
        assert!(status(&service).operations.is_empty());
        service.shutdown().unwrap();
    }
    #[test]
    fn image_restoration_is_retained_after_failure_and_keeps_other_sync_responsive() {
        let fixture = TestDrives::new(0);
        fixture.drives.lock().unwrap()[1].read_only = true;
        let (entered, release, gate) = worker_gate();
        let mut authority = fixture.image_authority();
        let mount = authority.mount.clone();
        authority.mount = Arc::new(move |drive| {
            assert!(drive.read_only);
            gate();
            mount(drive)
        });
        let service = fixture.start_authority(None, authority);
        let local = fixture.pair(&service, "small", "");
        let id = accept(&service, image_request(true));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let operation = wait_operation(&service, &id, |op| {
            op.partition
                .as_ref()
                .is_some_and(|p| p.phase == PartitionPhase::RestoringMount)
        });
        assert!(operation.partition.as_ref().unwrap().unmounted_by_job);
        fs::write(local.join("edit"), b"independent edit during remount").unwrap();
        wait_contents(
            &fixture.temp.path().join("small/edit"),
            b"independent edit during remount",
        );
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        let operation = finish_released(&service, &id, release);
        assert_eq!(operation.state, "cancelled");
        assert!(
            operation
                .message
                .contains("owned image authorization declined")
        );
        assert!(operation.message.contains("Partition mounted again"));
        let partition = operation.partition.unwrap();
        assert_eq!(partition.phase, PartitionPhase::MountRestored);
        assert!(partition.restore_error.is_none());
        assert!(partition.restore_mounted);
        assert!(status(&service).backups.is_empty());
        assert!(fixture.drives.lock().unwrap()[1].read_only);
        assert!(fixture.drives.lock().unwrap()[1].mount.is_some());
        service.shutdown().unwrap();
    }
    #[test]
    fn cancelled_image_restores_only_after_its_confirmed_unmount_and_never_opens_input() {
        let fixture = TestDrives::new(0);
        let (service, id, entered, release) = fixture.held_image(true);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        release.send(()).unwrap();
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "cancelled");
        assert!(operation.message.contains("cancelled before admission"));
        assert!(operation.message.contains("Partition mounted again"));
        assert_eq!(
            operation.partition.unwrap().phase,
            PartitionPhase::MountRestored
        );
        assert!(!fixture.temp.path().join("backup/large").exists());
        service.shutdown().unwrap();
    }
    #[test]
    fn image_failures_keep_separate_remount_failures_and_refuse_replaced_devices() {
        for replacement in [None, Some(false), Some(true)] {
            let fixture = TestDrives::new(0);
            let mut authority = fixture.image_authority();
            let drives = fixture.drives.clone();
            authority.open_image = Arc::new(move |drive, _| {
                if let Some(replace_node) = replacement {
                    if replace_node {
                        fs::rename(&drive.device, drive.device.with_extension("old")).unwrap();
                        fs::write(&drive.device, b"replacement").unwrap();
                    } else {
                        drives.lock().unwrap()[1].disk_sequence = Some(2);
                    }
                }
                Err("owned image open failed".into())
            });
            authority.mount = Arc::new(move |_| {
                assert!(replacement.is_none(), "replacement reached mount authority");
                Err("owned mount authorization declined".into())
            });
            let service = fixture.start_authority(None, authority);
            let operation = completed(&service, image_request(true));
            assert_eq!(operation.state, "failed");
            assert!(operation.message.contains("owned image open failed"));
            let partition = operation.partition.unwrap();
            assert_eq!(partition.phase, PartitionPhase::MountRestoreFailed);
            let error = partition.restore_error.unwrap();
            assert!(
                error.contains(match replacement {
                    None => "owned mount authorization declined",
                    Some(false) => "lifetime or identity changed",
                    Some(true) => "removed or replaced",
                }),
                "{error}"
            );
            assert!(status(&service).backups.is_empty());
            service.shutdown().unwrap();
        }
    }
    #[test]
    fn image_restoration_does_not_mount_originally_unmounted_or_already_restored_partitions() {
        for (initially_mounted, restore_mounted) in [(true, true), (false, true), (true, false)] {
            let fixture = TestDrives::new(0);
            let mut authority = fixture.image_authority();
            if !initially_mounted {
                fixture.drives.lock().unwrap()[1].mount = None;
                authority.unmount =
                    Arc::new(|_| panic!("already unmounted partition was unmounted"));
            }
            let drives = fixture.drives.clone();
            let mount = fixture.temp.path().join("large");
            authority.open_image = Arc::new(move |_, _| {
                if initially_mounted && restore_mounted {
                    drives.lock().unwrap()[1].mount = Some(mount.clone());
                }
                Err("owned image open failed".into())
            });
            authority.mount = Arc::new(|_| panic!("unnecessary mount request"));
            let service = fixture.start_authority(None, authority);
            let operation = completed(&service, image_request(restore_mounted));
            assert_eq!(operation.state, "failed");
            let partition = operation.partition.unwrap();
            assert_eq!(partition.unmounted_by_job, initially_mounted);
            assert_eq!(
                partition.phase,
                if initially_mounted && restore_mounted {
                    PartitionPhase::MountRestored
                } else {
                    PartitionPhase::OpeningImage
                }
            );
            assert!(partition.restore_error.is_none());
            service.shutdown().unwrap();
        }
    }
    #[test]
    fn restart_preserves_partition_checkpoints_without_replaying_mount_actions() {
        let fixture = TestDrives::new(0);
        let service = fixture.start(None, Arc::new(|_| panic!("unexpected authority request")));
        service.shutdown().unwrap();
        drop(service);
        let db = Store::existing(&fixture.temp.path().join("state/drives.sqlite"), false).unwrap();
        let mut expected = Vec::new();
        for (index, phase) in [
            PartitionPhase::Flushing,
            PartitionPhase::Unmounting,
            PartitionPhase::Unmounted,
            PartitionPhase::CopyingImage,
            PartitionPhase::VerifyingImage,
            PartitionPhase::PublishingImage,
            PartitionPhase::ImagePublished,
            PartitionPhase::RestoringMount,
        ]
        .into_iter()
        .enumerate()
        {
            let partition = PartitionPreparation {
                file_mode: Default::default(),
                collection_mount: None,
                drive: fixture.drives.lock().unwrap()[1].clone(),
                destination: Some(fixture.temp.path().join("backup")),
                phase,
                unmounted_by_job: index >= 2,
                restore_mounted: true,
                completed_backup: (phase == PartitionPhase::ImagePublished)
                    .then(|| "owned-completed-image".into()),
                restore_error: None,
            };
            let id = format!("owned-interruption-{index}");
            let operation = DriveOperation {
                id: id.clone(),
                title: "Owned partition checkpoint".into(),
                state: "running".into(),
                message: "before restart".into(),
                files: 0,
                bytes: 17,
                started: now(),
                finished: None,
                partition: Some(partition.clone()),
            };
            db.put("operation", &id, &operation).unwrap();
            expected.push((id, partition));
        }
        let mut legacy = serde_json::to_value(
            db.get::<DriveOperation>("operation", &expected[0].0)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        legacy.as_object_mut().unwrap().remove("partition");
        legacy["id"] = "owned-legacy-operation".into();
        db.put("operation", "owned-legacy-operation", &legacy)
            .unwrap();
        drop(db);
        let restarted = fixture.start(None, Arc::new(|_| panic!("restart replayed unmount")));
        let status = status(&restarted);
        for (id, partition) in expected {
            let operation = status.operations.iter().find(|op| op.id == id).unwrap();
            assert_eq!(operation.state, "interrupted");
            assert!(operation.finished.is_some());
            assert_eq!(operation.bytes, 17);
            assert_eq!(operation.message, partition.interruption_message());
            assert_eq!(operation.partition.as_ref().unwrap().phase, partition.phase);
            if partition.phase == PartitionPhase::Unmounting {
                assert!(operation.message.contains("may have taken effect"));
            }
        }
        let legacy = status
            .operations
            .iter()
            .find(|op| op.id == "owned-legacy-operation")
            .unwrap();
        assert!(legacy.partition.is_none());
        assert!(legacy.message.contains("completed writes retained"));
        restarted.shutdown().unwrap();
    }
    #[test]
    fn image_admission_requires_a_disk_lifetime_and_authorized_block_descriptor() {
        let fixture = TestDrives::new(0);
        let mut drive = fixture.drives.lock().unwrap()[1].clone();
        drive.disk_sequence = None;
        assert!(
            crate::device::Source::capture(&drive)
                .err()
                .unwrap()
                .to_string()
                .contains("disk sequence unavailable")
        );
        drive.disk_sequence = Some(1);
        let source = crate::device::Source::capture(&drive).unwrap();
        source.validate().unwrap();
        let ordinary = File::open(&drive.device).unwrap();
        assert!(
            source
                .validate_input(&ordinary)
                .unwrap_err()
                .to_string()
                .contains("Authorized descriptor")
        );
        fs::set_permissions(&drive.device, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(
            source
                .validate()
                .unwrap_err()
                .to_string()
                .contains("removed or replaced")
        );
    }
    #[test]
    fn image_refuses_device_node_replacement_while_unmount_is_pending() {
        let fixture = TestDrives::new(0);
        let device = fixture.drives.lock().unwrap()[1].device.clone();
        let (service, id, entered, release) = fixture.held_image(false);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        fs::rename(&device, device.with_extension("original")).unwrap();
        fs::write(&device, b"replacement device node").unwrap();
        release.send(()).unwrap();
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(
            operation.message.contains("removed or replaced"),
            "{}",
            operation.message
        );
        assert!(operation.message.contains("Unmount completed"));
        assert!(status(&service).backups.is_empty());
        assert!(!fixture.temp.path().join("backup/large").exists());
        service.shutdown().unwrap();
    }
    #[test]
    fn cancelled_image_waits_for_unmount_result_and_never_starts_copying() {
        let fixture = TestDrives::new(0);
        let (service, id, entered, release) = fixture.held_image(false);
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        assert_unfinished(&service, &id);
        finish(
            &service,
            DriveRequest::Rule {
                drive: "part:small".into(),
                relative: PathBuf::new(),
                catalog: false,
                index: false,
            },
        );
        release.send(()).unwrap();
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "cancelled");
        assert!(
            operation.message.contains("Partition is unmounted"),
            "{}",
            operation.message
        );
        assert!(status(&service).backups.is_empty());
        assert!(!fixture.temp.path().join("backup/large").exists());
        let refreshed = Instant::now();
        while status(&service)
            .drives
            .iter()
            .find(|drive| drive.id == "part:large")
            .unwrap()
            .mount
            .is_some()
        {
            assert!(
                refreshed.elapsed() < Duration::from_secs(5),
                "Unmount observation was not refreshed"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        service.shutdown().unwrap();
    }

    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; unmounts only an owned FUSE fixture"]
    fn partition_unmount_keeps_other_ram_responsive_and_retains_completed_cancellation() {
        let fixture = TestDrives::new(0);
        let (backing, source_id, native, source) = fixture.native_source("");
        let drives = fixture.drives.clone();
        let blocked = Arc::new(AtomicBool::new(false));
        let block = blocked.clone();
        let (entered, release, gate) = worker_gate();
        let unmount: partition::FilesystemAction = Arc::new(move |drive| {
            if block.swap(false, Ordering::AcqRel) {
                gate();
            }
            let result = CommandRequest::new("fusermount3")
                .arg("-u")
                .arg(drive.mount.as_ref().unwrap())
                .timeout(Duration::from_secs(5))
                .max_output_bytes(4096)
                .run()
                .map_err(|error| error.to_string())?;
            if !result.status.success() {
                return Err(format!("Owned FUSE fixture busy: {}", result.stderr_lossy()).into());
            }
            drives
                .lock()
                .unwrap()
                .iter_mut()
                .find(|candidate| candidate.device == drive.device)
                .unwrap()
                .mount = None;
            Ok(())
        });
        let service = fixture.start(None, unmount.clone());
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:small".into(),
            },
        );
        let small_ram = status(&service).ram.into_iter().next().unwrap();
        let local = fixture.pair(&service, "large", "");
        fs::write(local.join("edit"), b"local edit flushed before eject").unwrap();
        let external_reader = Root::open(&source).unwrap();
        let busy = accept(
            &service,
            DriveRequest::Eject {
                drive: "part:large".into(),
            },
        );
        let operation = wait_operation(&service, &busy, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(
            operation.message.contains("Owned FUSE fixture busy"),
            "{}",
            operation.message
        );
        assert_eq!(
            fs::read(source.join("edit")).unwrap(),
            b"local edit flushed before eject"
        );
        drop(external_reader);
        blocked.store(true, Ordering::Release);
        let id = accept(
            &service,
            DriveRequest::Eject {
                drive: "part:large".into(),
            },
        );
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        fs::write(
            small_ram.path.join("edit"),
            b"RAM edit during pending unmount",
        )
        .unwrap();
        wait_contents(
            &fixture.temp.path().join("small/edit"),
            b"RAM edit during pending unmount",
        );
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        let operation = finish_released(&service, &id, release);
        assert_eq!(operation.state, "complete", "{}", operation.message);
        assert!(operation.message.contains("Partition unmounted"));
        finish(
            &service,
            DriveRequest::DiscardRam {
                session: small_ram.id,
                confirmed: true,
            },
        );
        service.shutdown().unwrap();
        drop(native);
        let recovered = crate::ram::Mounted::start(&backing, &source_id, false, None).unwrap();
        assert_eq!(
            fs::read(source.join("edit")).unwrap(),
            b"local edit flushed before eject"
        );
        // Exercise real remount restoration after an owned image-open refusal.
        // The replacement server is retired before its creating worker exits.
        let servers = Arc::new(Mutex::new(Some(recovered)));
        fixture.drives.lock().unwrap()[1].mount = Some(source.clone());
        let mut authority = fixture.image_authority();
        let old_server = servers.clone();
        authority.unmount = Arc::new(move |drive| {
            unmount(drive)?;
            drop(old_server.lock().unwrap().take());
            Ok(())
        });
        let servers_for_mount = servers.clone();
        let mount_backing = backing.clone();
        let mount_id = source_id.clone();
        let mounted = authority.mount.clone();
        authority.mount = Arc::new(move |drive| {
            let server = crate::ram::Mounted::start(&mount_backing, &mount_id, false, None)?;
            *servers_for_mount.lock().unwrap() = Some(server);
            mounted(drive)
        });
        let restarted = fixture.start_authority(None, authority);
        let pair = status(&restarted)
            .pairs
            .into_iter()
            .find(|pair| pair.drive == "part:large")
            .unwrap();
        finish(
            &restarted,
            DriveRequest::Pause {
                pair: pair.id,
                paused: true,
            },
        );
        fs::create_dir_all(fixture.temp.path().join("backup")).unwrap();
        let DriveReply::BackupPreview(review) = restarted
            .request(DriveRequest::PreviewBackup {
                drive: "part:large".into(),
                kind: BackupKind::Image,
                destination: None,
            })
            .unwrap()
        else {
            panic!("expected image review");
        };
        // A second pending review must not keep our own source mount busy.
        restarted
            .request(DriveRequest::PreviewBackup {
                drive: "part:large".into(),
                kind: BackupKind::Image,
                destination: None,
            })
            .unwrap();
        let operation = completed(
            &restarted,
            DriveRequest::StartBackup {
                token: review.token,
                restore_mounted: true,
                remember_destination: false,
            },
        );
        assert_eq!(operation.state, "failed");
        assert!(
            operation
                .message
                .contains("owned image authorization declined")
        );
        assert_eq!(
            operation.partition.unwrap().phase,
            PartitionPhase::MountRestored
        );
        assert_eq!(
            fs::read(source.join("edit")).unwrap(),
            b"local edit flushed before eject"
        );
        drop(servers.lock().unwrap().take());
        fixture.drives.lock().unwrap()[1].mount = None;
        restarted.shutdown().unwrap();
        crate::ram::discard(&backing, &source_id).unwrap();
    }

    #[test]
    fn image_authority_refusal_keeps_completed_flushes_and_creates_no_backup() {
        let fixture = TestDrives::new(0);
        let source = fixture.temp.path().join("large");
        fs::write(source.join("edit"), b"before").unwrap();
        let checked = source.clone();
        let service = fixture.start(
            None,
            Arc::new(move |_| {
                assert_eq!(
                    fs::read(checked.join("edit")).unwrap(),
                    b"flushed before unmount"
                );
                Err("owned fixture authorization declined".into())
            }),
        );
        let local = fixture.pair(&service, "large", "");
        fs::write(local.join("edit"), b"flushed before unmount").unwrap();
        let id = accept(&service, image_request(false));
        let operation = wait_operation(&service, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(
            operation
                .message
                .contains("owned fixture authorization declined"),
            "{}",
            operation.message
        );
        assert!(operation.message.contains("still appears mounted"));
        assert!(status(&service).backups.is_empty());
        assert!(!fixture.temp.path().join("backup/large").exists());
        assert_eq!(
            fs::read(local.join("edit")).unwrap(),
            b"flushed before unmount"
        );
        let pair = status(&service).pairs.into_iter().next().unwrap();
        assert!(pair.last_success.is_some());
        assert!(pair.status.contains("files copied"));
        service.shutdown().unwrap();
    }

    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM helper; uses only owned fixture storage"]
    fn file_backup_rejects_private_ram_backing_configured_before_restart() {
        let fixture = TestDrives::new(0);
        let service = fixture.start(None, Arc::new(partition::unmount));
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:small".into(),
            },
        );
        let snapshot = status(&service);
        let session = snapshot.ram.into_iter().next().unwrap();
        let backing = session.backing.unwrap();
        let mut config = snapshot.config;
        config.backup_directory = Some(backing.directory.clone());
        service.shutdown().unwrap();
        let drives = fixture.drives.clone();
        let restarted = Service::start_with_authority(
            config,
            fixture.temp.path().join("state"),
            Arc::new(move || Ok(drives.lock().unwrap().clone())),
            None,
            Arc::new(partition::Authority::default()),
            publication::Publication::default(),
        )
        .unwrap();
        let id = accept(
            &restarted,
            DriveRequest::Backup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                restore_mounted: false,
            },
        );
        let operation = wait_operation(&restarted, &id, |op| op.finished.is_some());
        assert_eq!(operation.state, "failed");
        assert!(
            operation.message.contains("private RAM backing storage"),
            "{}",
            operation.message
        );
        assert!(!backing.directory.join("large").exists());
        assert!(backing.directory.join("volume.ext4").is_file());
        assert!(status(&restarted).backups.is_empty());
        finish(
            &restarted,
            DriveRequest::DiscardRam {
                session: session.id,
                confirmed: true,
            },
        );
        restarted.shutdown().unwrap();
    }

    fn assert_unfinished(service: &Service, id: &str) {
        let snapshot = status(service);
        let operation = snapshot
            .operations
            .iter()
            .find(|operation| operation.id == id)
            .unwrap();
        assert!(
            operation.finished.is_none(),
            "operation finished while its worker was held: {}",
            operation.message
        );
    }
    fn finish_released(service: &Service, id: &str, release: mpsc::Sender<()>) -> DriveOperation {
        assert_unfinished(service, id);
        release.send(()).unwrap();
        wait_operation(service, id, |operation| operation.finished.is_some())
    }
    fn cancel_and_finish_released(
        service: &Service,
        id: &str,
        release: mpsc::Sender<()>,
    ) -> DriveOperation {
        service
            .request(DriveRequest::Cancel {
                operation: id.into(),
            })
            .unwrap();
        finish_released(service, id, release)
    }
    fn image_request(restore_mounted: bool) -> DriveRequest {
        DriveRequest::Backup {
            drive: "part:large".into(),
            kind: BackupKind::Image,
            restore_mounted,
        }
    }
    fn accept(service: &Service, request: DriveRequest) -> String {
        match service.request(request).unwrap() {
            DriveReply::Accepted(id) => id,
            _ => panic!("expected an accepted operation"),
        }
    }
    fn completed(service: &Service, request: DriveRequest) -> DriveOperation {
        let id = accept(service, request);
        wait_operation(service, &id, |operation| operation.finished.is_some())
    }
    fn finish(service: &Service, request: DriveRequest) -> DriveOperation {
        let operation = completed(service, request);
        assert_eq!(operation.state, "complete", "{}", operation.message);
        operation
    }
    fn failed(service: &Service, request: DriveRequest, diagnostic: &str) -> DriveOperation {
        let operation = completed(service, request);
        assert_eq!(operation.state, "failed", "{}", operation.message);
        assert!(
            operation.message.contains(diagnostic),
            "{}",
            operation.message
        );
        operation
    }

    fn discard_ram(service: &Service, session: &RamSession) {
        finish(
            service,
            DriveRequest::DiscardRam {
                session: session.id.clone(),
                confirmed: true,
            },
        );
        assert!(!session.path.exists());
        let snapshot = status(service);
        assert!(!snapshot.ram.iter().any(|ram| ram.id == session.id));
        assert!(!snapshot.pairs.iter().any(|pair| pair.id == session.id));
        let db = Store::existing(&service.state.join("drives.sqlite"), true).unwrap();
        let pending = ram_pending::snapshot(&db, &session.id).unwrap();
        assert_eq!(pending.paths, 0);
        assert!(!pending.full_check_required);
    }

    #[test]
    fn shutdown_closes_admission_finishes_accepted_records_and_releases_singleton() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        File::create(source.join("large-sparse"))
            .unwrap()
            .set_len(100 * 1024 * 1024 * 1024)
            .unwrap();
        let drive = fixture_drive(&source, "shutdown");
        let inventory: Inventory = Arc::new(move || Ok(vec![drive.clone()]));
        let config = DriveConfig {
            backup_directory: Some(temp.path().join("backup")),
            ..Default::default()
        };
        let state = temp.path().join("state");
        let service =
            Service::start_with_inventory(config.clone(), state.clone(), inventory.clone())
                .unwrap();
        let mut ids = Vec::new();
        for request in [
            DriveRequest::Backup {
                drive: "part:shutdown".into(),
                kind: BackupKind::Files,
                restore_mounted: false,
            },
            DriveRequest::Rule {
                drive: "part:shutdown".into(),
                relative: "folder".into(),
                catalog: false,
                index: false,
            },
        ] {
            let id = accept(&service, request);
            ids.push(id);
        }
        service.shutdown().unwrap();
        service.shutdown().unwrap();
        let current = status(&service);
        for id in ids {
            let operation = current.operations.iter().find(|o| o.id == id).unwrap();
            assert!(operation.finished.is_some());
            assert!(matches!(operation.state.as_str(), "complete" | "cancelled"));
        }
        assert!(current.backups.is_empty());
        assert!(
            service
                .request(DriveRequest::CopyToRam {
                    drive: "part:shutdown".into()
                })
                .unwrap_err()
                .to_string()
                .contains("admission is closed")
        );
        // No sleep or dropping the old client is needed to release storage ownership.
        let restarted = Service::start_with_inventory(config, state, inventory).unwrap();
        restarted.shutdown().unwrap();
    }
    #[test]
    fn slow_initial_reconciliation_does_not_block_another_pairs_events_or_controls() {
        let (temp, service, small) = two_drives(100 * 1024 * 1024 * 1024);
        let local = temp.path().join("small-local");
        finish(
            &service,
            DriveRequest::Pair {
                drive: "part:small".into(),
                relative: PathBuf::new(),
                local: local.clone(),
            },
        );
        let id = accept(
            &service,
            DriveRequest::Pair {
                drive: "part:large".into(),
                relative: PathBuf::new(),
                local: temp.path().join("large-local"),
            },
        );
        wait_operation(&service, &id, |op| op.state == "running");
        fs::write(local.join("edit"), b"independent event during full scan").unwrap();
        wait_contents(&small.join("edit"), b"independent event during full scan");
        assert!(
            status(&service)
                .operations
                .iter()
                .any(|o| o.id == id && o.finished.is_none())
        );
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        service.shutdown().unwrap();
        assert!(
            status(&service)
                .operations
                .iter()
                .find(|o| o.id == id)
                .unwrap()
                .finished
                .is_some()
        );
    }
    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
    fn ram_preflight_keeps_other_writeback_responsive_and_cancellation_retains_admission() {
        let held = Arc::new(AtomicBool::new(false));
        let hold = held.clone();
        let (entered_rx, release_tx, gate) = worker_gate();
        let (temp, service, small) = two_drives_observed(
            1024,
            Some(Arc::new(move || {
                if std::thread::current()
                    .name()
                    .is_some_and(|name| name.starts_with("drive-ram-setup-"))
                    && hold.swap(false, Ordering::AcqRel)
                {
                    gate();
                }
            })),
        );
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:small".into(),
            },
        );
        let ram = status(&service).ram.into_iter().next().unwrap();
        held.store(true, Ordering::Release);
        let id = accept(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:large".into(),
            },
        );
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        fs::write(ram.path.join("edit"), b"writeback while preflight is held").unwrap();
        wait_contents(&small.join("edit"), b"writeback while preflight is held");
        finish(
            &service,
            DriveRequest::Pause {
                pair: ram.id.clone(),
                paused: true,
            },
        );
        let mut config = status(&service).config;
        config.ram_session_max_bytes = 128 * 1024 * 1024;
        config.ram_total_max_bytes = config.ram_session_max_bytes;
        let reload = accept(&service, DriveRequest::Configure { config });
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        let during = status(&service);
        assert!(
            during
                .operations
                .iter()
                .find(|op| op.id == id)
                .unwrap()
                .finished
                .is_none()
        );
        assert_eq!(
            during.config.ram_total_max_bytes,
            DriveConfig::default().ram_total_max_bytes
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            wait_operation(&service, &id, |op| op.finished.is_some()).state,
            "cancelled"
        );
        assert_eq!(
            wait_operation(&service, &reload, |op| op.finished.is_some()).state,
            "complete"
        );
        assert_eq!(status(&service).ram.len(), 1);
        let db = Store::existing(&temp.path().join("state/drives.sqlite"), true).unwrap();
        assert_eq!(
            db.0.query_row(
                "SELECT count(*) FROM entities WHERE kind='ram-allocation'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        finish(
            &service,
            DriveRequest::DiscardRam {
                session: ram.id,
                confirmed: true,
            },
        );
        service.shutdown().unwrap();
    }

    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
    fn large_ram_population_keeps_an_existing_ram_session_promptly_writable() {
        let (_temp, service, small) = two_drives(1024 * 1024 * 1024);
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:small".into(),
            },
        );
        let ram = status(&service)
            .ram
            .into_iter()
            .find(|s| s.drive == "part:small")
            .unwrap();
        let id = accept(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:large".into(),
            },
        );
        wait_operation(&service, &id, |op| op.state == "running");
        fs::write(
            ram.path.join("edit"),
            b"RAM first while another card populates",
        )
        .unwrap();
        wait_contents(
            &small.join("edit"),
            b"RAM first while another card populates",
        );
        assert!(
            status(&service)
                .operations
                .iter()
                .any(|o| o.id == id && o.finished.is_none())
        );
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        let start = Instant::now();
        while status(&service)
            .operations
            .iter()
            .find(|o| o.id == id)
            .unwrap()
            .finished
            .is_none()
        {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        for session in status(&service).ram {
            finish(
                &service,
                DriveRequest::DiscardRam {
                    session: session.id,
                    confirmed: true,
                },
            );
        }
        service.shutdown().unwrap();
    }
    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
    fn daemon_sync_and_ram_survive_client_closure_and_media_removal() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("portable");
        let local = temp.path().join("local");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("x"), b"initial").unwrap();
        let drive = fixture_drive(&source, "fixture");
        let inventory = Arc::new(Mutex::new(vec![drive.clone()]));
        let observed = inventory.clone();
        let callback: Inventory = Arc::new(move || Ok(observed.lock().unwrap().clone()));
        let state = temp.path().join("state");
        let service = Service::start_with_inventory(
            DriveConfig {
                sync_interval_seconds: 1,
                ram_min_available: 0,
                ..Default::default()
            },
            state.clone(),
            callback.clone(),
        )
        .unwrap();
        finish(
            &service,
            DriveRequest::Pair {
                drive: drive.id.clone(),
                relative: PathBuf::new(),
                local: local.clone(),
            },
        );
        assert_eq!(fs::read(local.join("x")).unwrap(), b"initial");
        // No file-manager instance or request remains while automatic sync operates.
        fs::write(local.join("y"), b"background").unwrap();
        let start = Instant::now();
        while !source.join("y").exists() {
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(50));
        }
        // Population's catch-up pass remains read-only-source compatible.
        inventory.lock().unwrap()[0].read_only = true;
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: drive.id.clone(),
            },
        );
        let ram = status(&service).ram.into_iter().next().unwrap();
        assert!(ram.ready);
        assert!(ram.last_flushed.is_some());
        inventory.lock().unwrap().clear();
        fs::rename(&source, temp.path().join("removed")).unwrap();
        fs::write(ram.path.join("x"), b"RAM offline").unwrap();
        assert_eq!(fs::read(ram.path.join("x")).unwrap(), b"RAM offline");
        let offline = wait_ram(&service, &ram.id, |session| {
            !session.connected
                && session
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.paths >= 1)
        });
        let baseline = status(&service)
            .pairs
            .into_iter()
            .find(|p| p.id == ram.id)
            .unwrap()
            .last_success;
        let saved = temp.path().join("saved");
        finish(
            &service,
            DriveRequest::SaveRam {
                session: ram.id.clone(),
                destination: saved.clone(),
            },
        );
        let saved_copy = fs::read_dir(&saved)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(saved_copy.join("x")).unwrap(), b"RAM offline");
        let after_save = status(&service);
        assert_eq!(
            after_save
                .pairs
                .iter()
                .find(|p| p.id == ram.id)
                .unwrap()
                .last_success,
            baseline
        );
        let saved_session = after_save.ram.iter().find(|s| s.id == ram.id).unwrap();
        assert_eq!(saved_session.last_flushed, offline.last_flushed);
        assert!(saved_session.pending.as_ref().unwrap().paths >= 1);
        let export = saved_session.last_saved.as_ref().unwrap();
        assert_eq!(export.path, saved_copy);
        assert!(export.populated);
        assert!(
            saved_session
                .save_status
                .as_ref()
                .unwrap()
                .contains("RAM copy saved")
        );
        service.shutdown().unwrap();
        drop(service);
        let service = Service::start_with_inventory(
            DriveConfig {
                sync_interval_seconds: 1,
                ram_min_available: 0,
                ..Default::default()
            },
            state.clone(),
            callback.clone(),
        )
        .unwrap();
        let recovered = wait_ram(&service, &ram.id, |session| session.browsable());
        assert!(!recovered.connected);
        assert_eq!(recovered.last_flushed, offline.last_flushed);
        assert!(recovered.pending.as_ref().unwrap().full_check_required);
        assert!(recovered.pending.as_ref().unwrap().paths >= 1);
        fs::write(ram.path.join("after-offline-restart"), b"still in RAM").unwrap();
        wait_ram(&service, &ram.id, |session| {
            session
                .pending
                .as_ref()
                .is_some_and(|pending| pending.paths >= 2)
        });
        fs::rename(temp.path().join("removed"), &source).unwrap();
        fs::write(source.join("x"), b"portable offline").unwrap();
        inventory.lock().unwrap().push(drive.clone());
        finish(
            &service,
            DriveRequest::FlushRam {
                session: ram.id.clone(),
            },
        );
        assert!(!status(&service).conflicts.is_empty());
        let flushed = wait_ram(&service, &ram.id, |session| {
            session.connected
                && session
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.paths == 0 && !pending.full_check_required)
        });
        assert!(flushed.last_flushed >= offline.last_flushed);
        // Routine scans must not repeatedly advertise lost event coverage.
        for _ in 0..30 {
            assert!(
                !status(&service).ram[0]
                    .pending
                    .as_ref()
                    .unwrap()
                    .full_check_required
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            fs::read(source.join("after-offline-restart")).unwrap(),
            b"still in RAM"
        );
        use std::io::Write;
        let mut held = File::create(ram.path.join("daemon-held-open")).unwrap();
        held.write_all(b"accepted before graceful shutdown")
            .unwrap();
        service.shutdown().unwrap();
        drop(service);
        let service = Service::start_with_inventory(
            DriveConfig {
                sync_interval_seconds: 1,
                ram_min_available: 0,
                ..Default::default()
            },
            state,
            callback,
        )
        .unwrap();
        assert!(status(&service).ram.iter().any(|s| s.path == ram.path));
        let start = Instant::now();
        while !status(&service).ram.iter().any(|s| s.browsable()) {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            fs::read(ram.path.join("daemon-held-open")).unwrap(),
            b"accepted before graceful shutdown"
        );
        drop(held);
        discard_ram(&service, &ram);
        service.shutdown().unwrap();
        drop(service);
    }
    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
    fn cancelled_initial_ram_copy_retains_live_backing_and_resumes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("a"), b"accepted before cancellation").unwrap();
        File::create(source.join("b"))
            .unwrap()
            .set_len(128 * 1024 * 1024)
            .unwrap();
        let drive = fixture_drive(&source, "cancelled");
        let service = Service::start_with_inventory(
            DriveConfig {
                ram_min_available: 0,
                ..Default::default()
            },
            temp.path().join("state"),
            Arc::new(move || Ok(vec![drive.clone()])),
        )
        .unwrap();
        let id = accept(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:cancelled".into(),
            },
        );
        let start = Instant::now();
        let session = loop {
            if let Some(session) = status(&service).ram.into_iter().next()
                && session.path.join("a").is_file()
            {
                break session;
            }
            assert!(start.elapsed() < Duration::from_secs(15));
            std::thread::sleep(Duration::from_millis(2));
        };
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        let start = Instant::now();
        loop {
            if let Some(op) = status(&service)
                .operations
                .into_iter()
                .find(|o| o.id == id && o.finished.is_some())
            {
                assert_eq!(op.state, "cancelled");
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        let retained = status(&service)
            .ram
            .into_iter()
            .find(|s| s.id == session.id)
            .unwrap();
        assert!(!retained.ready);
        assert!(
            retained.backing.as_ref().unwrap().mounted,
            "cancelled copy lost its live mount record"
        );
        assert!(retained.backing.as_ref().unwrap().initialized);
        assert_eq!(
            fs::read(retained.path.join("a")).unwrap(),
            b"accepted before cancellation"
        );
        failed(
            &service,
            DriveRequest::Eject {
                drive: "part:cancelled".into(),
            },
            "RAM population is incomplete",
        );
        finish(
            &service,
            DriveRequest::FlushRam {
                session: session.id.clone(),
            },
        );
        assert!(status(&service).ram[0].browsable());
        assert_eq!(
            fs::metadata(retained.path.join("b")).unwrap().len(),
            128 * 1024 * 1024
        );
        finish(
            &service,
            DriveRequest::DiscardRam {
                session: session.id,
                confirmed: true,
            },
        );
        service.shutdown().unwrap();
        drop(service);
    }
    #[test]
    #[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
    fn bulk_backup_leaves_another_drives_ram_writeback_and_status_responsive() {
        let (_temp, service, small) = two_drives(16 * 1024 * 1024 * 1024);
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: "part:small".into(),
            },
        );
        let ram = status(&service).ram[0].clone();
        let id = accept(
            &service,
            DriveRequest::Backup {
                drive: "part:large".into(),
                kind: BackupKind::Files,
                restore_mounted: false,
            },
        );
        wait_operation(&service, &id, |op| op.state == "running");
        fs::write(ram.path.join("edit"), b"RAM first during backup").unwrap();
        wait_contents(&small.join("edit"), b"RAM first during backup");
        assert!(
            status(&service)
                .operations
                .iter()
                .any(|o| o.id == id && o.finished.is_none())
        );
        service
            .request(DriveRequest::Cancel {
                operation: id.clone(),
            })
            .unwrap();
        wait_operation(&service, &id, |op| op.finished.is_some());
        finish(
            &service,
            DriveRequest::DiscardRam {
                session: ram.id,
                confirmed: true,
            },
        );
        service.shutdown().unwrap();
        drop(service);
    }
    #[test]
    fn unset_backup_directory_and_duplicate_identity_fail_without_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let mut drive = fixture_drive(&source, "duplicate");
        drive.ambiguous = true;
        drive.filesystem_uuid = None;
        let service = Service::start_with_inventory(
            DriveConfig::default(),
            temp.path().join("state"),
            Arc::new(move || Ok(vec![drive.clone()])),
        )
        .unwrap();
        let id = accept(
            &service,
            DriveRequest::Backup {
                drive: "part:duplicate".into(),
                kind: BackupKind::Image,
                restore_mounted: false,
            },
        );
        let start = Instant::now();
        loop {
            if let Some(op) = status(&service)
                .operations
                .into_iter()
                .find(|o| o.id == id && o.finished.is_some())
            {
                assert_eq!(op.state, "failed");
                assert!(op.message.contains("DriveBackupDir"));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(status(&service).backup_directory.is_none());
        service.shutdown().unwrap();
        drop(service);
    }
}
