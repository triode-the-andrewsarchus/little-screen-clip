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
}
#[derive(Clone)]
pub struct Service {
    sender: mpsc::SyncSender<Envelope>,
    shared: Arc<Mutex<DriveStatus>>,
    tokens: Tokens,
    state: PathBuf,
}
struct Owner {
    inventory: Inventory,
    db: Store,
    state: PathBuf,
    config: DriveConfig,
    shared: Arc<Mutex<DriveStatus>>,
    tokens: Tokens,
    watch: Option<Watch>,
    _lock: File,
    children: HashMap<String, std::process::Child>,
}

impl Service {
    /// Starts the bounded storage coordinator and recovers unfinished operation records.
    ///
    /// # Errors
    /// Returns directory, database, singleton-lock or thread startup failures.
    pub fn start(config: DriveConfig, state: PathBuf) -> Result<Self> {
        Self::start_with_inventory(config, state, Arc::new(discovery::discover))
    }
    fn start_with_inventory(
        config: DriveConfig,
        state: PathBuf,
        inventory: Inventory,
    ) -> Result<Self> {
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
        if config.backup_directory.is_none() {
            config.backup_directory = db
                .get::<DriveConfig>("settings", "config")?
                .and_then(|c| c.backup_directory);
        }
        let shared = Arc::new(Mutex::new(DriveStatus {
            config: config.clone(),
            backup_directory: config.backup_directory.clone(),
            ..Default::default()
        }));
        let tokens = Arc::new(Mutex::new(HashMap::new()));
        let (sender, receiver) = mpsc::sync_channel(16);
        let mut owner = Owner {
            inventory,
            db,
            state: root.path.clone(),
            config,
            shared: shared.clone(),
            tokens: tokens.clone(),
            watch: Watch::new().ok(),
            _lock: lock,
            children: HashMap::new(),
        };
        owner.recover()?;
        owner.discover();
        owner.publish_policy()?;
        std::thread::Builder::new()
            .name("portable-drive-owner".into())
            .spawn(move || owner.run(receiver))?;
        Ok(Self {
            sender,
            shared,
            tokens,
            state: root.path,
        })
    }
    /// Admits an operation or returns a bounded status page without waiting for writes.
    ///
    /// # Errors
    /// Returns queue, database or validation failures. Accepted writes retain ownership.
    pub fn request(&self, request: DriveRequest) -> Result<DriveReply> {
        if let DriveRequest::Status { offset } = request {
            return self.status(offset).map(|s| DriveReply::Status(Box::new(s)));
        }
        if let DriveRequest::Cancel { operation } = request {
            let tokens = self
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
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let db = Store::open(&self.state.join("drives.sqlite"))?;
        let operation = DriveOperation {
            id: id.clone(),
            title: title(&request).into(),
            state: "queued".into(),
            message: String::new(),
            files: 0,
            bytes: 0,
            started: now(),
            finished: None,
        };
        db.put("operation", &id, &operation)?;
        self.tokens
            .lock()
            .map_err(|_| "Drive cancellation state unavailable")?
            .insert(id.clone(), cancel.clone());
        if self
            .sender
            .try_send(Envelope {
                id: id.clone(),
                request,
                cancel,
            })
            .is_err()
        {
            self.tokens.lock().ok().map(|mut t| t.remove(&id));
            db.remove("operation", &id)?;
            return Err("Drive operation queue is full or unavailable; try again".into());
        }
        Ok(DriveReply::Accepted(id))
    }
    fn status(&self, offset: usize) -> Result<DriveStatus> {
        let mut status = self
            .shared
            .lock()
            .map_err(|_| "Drive status unavailable")?
            .clone();
        let db = Store::open(&self.state.join("drives.sqlite"))?;
        status.pairs = db.list("pair", offset, 100)?;
        status.ram = db.list("ram", offset, 100)?;
        status.operations = db.list("operation", offset, 100)?;
        status.catalogs = db.list("catalog", offset, 100)?;
        status.conflicts = db.list("conflict", offset, 100)?;
        status.rules = db.list("rule", offset, 100)?;
        status.backups = db.list("backup", offset, 100)?;
        if [
            status.pairs.len(),
            status.ram.len(),
            status.operations.len(),
            status.catalogs.len(),
            status.conflicts.len(),
            status.rules.len(),
            status.backups.len(),
        ]
        .contains(&100)
        {
            status.next_offset = Some(offset + 100);
        }
        for session in &mut status.ram {
            session.connected = status
                .drives
                .iter()
                .any(|d| d.id == session.drive && d.mount.is_some() && !d.ambiguous);
        }
        Ok(status)
    }
}
fn title(request: &DriveRequest) -> &'static str {
    match request {
        DriveRequest::CopyToRam { .. } => "Copy to RAM",
        DriveRequest::Backup {
            kind: BackupKind::Files,
            ..
        } => "Back up files",
        DriveRequest::Backup { .. } => "Back up partition image",
        DriveRequest::Catalog { .. } => "Save contents catalog",
        DriveRequest::Pair { .. } => "Enroll folder sync",
        DriveRequest::Sync { .. } | DriveRequest::FlushRam { .. } => "Synchronize",
        DriveRequest::SaveRam { .. } => "Save RAM copy",
        _ => "Drive settings",
    }
}
impl Owner {
    fn recover(&mut self) -> Result<()> {
        let mut offset = 0;
        loop {
            let operations: Vec<DriveOperation> = self.db.list("operation", offset, 128)?;
            if operations.is_empty() {
                break;
            }
            for mut op in operations {
                if op.finished.is_none() {
                    op.state = "interrupted".into();
                    op.message =
                        "Daemon restarted; completed writes retained. Reconcile before retrying."
                            .into();
                    op.finished = Some(now());
                    self.db.put("operation", &op.id, &op)?;
                }
            }
            offset += 128;
        }
        for mut catalog in self.db.list::<Catalog>("catalog", 0, 1000)? {
            catalog.mount = None;
            self.db.put("catalog", &catalog.id, &catalog)?;
        }
        Ok(())
    }
    fn discover(&self) {
        let mut status = match self.shared.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        match (self.inventory)() {
            Ok(drives) => {
                status.drives = drives;
                status.discovery_error = None;
            }
            Err(e) => {
                status.drives.clear();
                status.discovery_error = Some(e.to_string());
            }
        }
        status.ram_unavailable = ram_reason(&self.config).err().map(|e| e.to_string());
        status.backup_directory = self.config.backup_directory.clone();
        status.config = self.config.clone();
    }
    fn run(&mut self, receiver: mpsc::Receiver<Envelope>) {
        let mut discovered = Instant::now();
        let mut synchronized = Instant::now() - Duration::from_secs(3600);
        let mut wake = Instant::now();
        loop {
            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(envelope) => self.operation(envelope),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
            if discovered.elapsed() >= Duration::from_secs(5) {
                self.discover();
                let _ = self.publish_policy();
                discovered = Instant::now();
            }
            let changed = self.watch.as_mut().is_some_and(Watch::changed);
            if synchronized.elapsed()
                >= Duration::from_secs(self.config.sync_interval_seconds.max(1))
                || (changed && wake.elapsed() >= Duration::from_millis(200))
            {
                self.automatic();
                synchronized = Instant::now();
                wake = Instant::now();
            }
            self.children
                .retain(|_, child| matches!(child.try_wait(), Ok(None)));
        }
        // Persistent helpers are owned by this daemon, never detached.
        for child in self.children.values_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn operation(&mut self, envelope: Envelope) {
        let id = &envelope.id;
        let mut op = match self.db.get::<DriveOperation>("operation", id) {
            Ok(Some(op)) => op,
            _ => return,
        };
        op.state = "running".into();
        let _ = self.db.put("operation", id, &op);
        let progress_db = self.state.join("drives.sqlite");
        let op_id = id.clone();
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut progress = |files, bytes| {
            if last.elapsed() < Duration::from_millis(250) {
                return;
            }
            last = Instant::now();
            if let Ok(db) = Store::open(&progress_db) {
                if let Ok(Some(mut op)) = db.get::<DriveOperation>("operation", &op_id) {
                    op.files = files;
                    op.bytes = bytes;
                    let _ = db.put("operation", &op_id, &op);
                }
            }
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.execute(envelope.request, &envelope.cancel, &mut progress)
        }));
        op = self.db.get("operation", id).ok().flatten().unwrap_or(op);
        op.finished = Some(now());
        match result {
            Ok(Ok(message)) => {
                op.state = "complete".into();
                op.message = message;
            }
            Ok(Err(e)) => {
                op.state = if envelope.cancel.load(Ordering::Acquire) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                op.message = e.to_string();
            }
            Err(_) => {
                op.state = "failed".into();
                op.message = "Storage worker failed; completed writes retained".into();
            }
        }
        let _ = self.db.put("operation", id, &op);
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.remove(id);
        }
        self.discover();
        let _ = self.publish_policy();
    }
    fn drive(&self, id: &str) -> Result<Drive> {
        let mut matches = (self.inventory)()?.into_iter().filter(|d| d.id == id);
        let drive = matches.next().ok_or("Drive is not available")?;
        if drive.ambiguous || matches.next().is_some() {
            return Err(
                "Multiple drives have the same identity; automatic writes are suspended".into(),
            );
        }
        Ok(drive)
    }
    fn execute(
        &mut self,
        request: DriveRequest,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        cancelled(cancel)?;
        match request {
            DriveRequest::Eject { drive } => {
                let drive = self.drive(&drive)?;
                for pair in self.db.list::<SyncPair>("pair", 0, 1000)? {
                    if pair.drive == drive.id && !pair.paused {
                        self.synchronize(&pair.id, cancel, progress)?;
                    }
                }
                let output = CommandRequest::new("udisksctl")
                    .args(["unmount", "--block-device"])
                    .arg(&drive.device)
                    .timeout(Duration::from_secs(60))
                    .own_process_group()
                    .run()
                    .map_err(|e| e.to_string())?;
                if !output.status.success() {
                    return Err(format!(
                        "Unmount failed; RAM copies remain available: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )
                    .into());
                }
                Ok("Partition unmounted; RAM copy retained".into())
            }
            DriveRequest::MountImage { backup } => backup::mount_image(&self.db, &backup),
            DriveRequest::UnmountImage { backup } => backup::unmount_image(&self.db, &backup),
            DriveRequest::PruneBackups { drive } => {
                let keep = self
                    .config
                    .backup_keep_versions
                    .ok_or("Set drives.backup_keep_versions before explicitly pruning backups")?;
                backup::prune(&self.db, &drive, keep, cancel)
            }
            DriveRequest::Configure { mut config } => {
                if config.backup_directory.is_none() {
                    config.backup_directory = self.config.backup_directory.clone();
                }
                self.config = config;
                Ok("Drive configuration reloaded".into())
            }
            DriveRequest::BackupDirectory { path } => {
                let root = Root::create(&path)?;
                self.config.backup_directory = Some(root.path);
                self.db.put("settings", "config", &self.config)?;
                Ok("Backup directory configured".into())
            }
            DriveRequest::CopyToRam { drive } => {
                let drive = self.drive(&drive)?;
                self.ram(&drive, cancel, progress)
            }
            DriveRequest::Backup { drive, kind } => {
                let destination = self
                    .config
                    .backup_directory
                    .clone()
                    .ok_or("Choose a DriveBackupDir before starting a backup")?;
                let mut drive = self.drive(&drive)?;
                if kind == BackupKind::Image && drive.mount.is_some() {
                    for pair in self.db.list::<SyncPair>("pair", 0, 1000)? {
                        if pair.drive == drive.id {
                            self.synchronize(&pair.id, cancel, progress)?;
                        }
                    }
                    let output = CommandRequest::new("udisksctl")
                        .args(["unmount", "--block-device"])
                        .arg(&drive.device)
                        .timeout(Duration::from_secs(60))
                        .own_process_group()
                        .run()
                        .map_err(|e| e.to_string())?;
                    if !output.status.success() {
                        return Err(format!(
                            "Cannot unmount partition for imaging: {}",
                            String::from_utf8_lossy(&output.stderr)
                        )
                        .into());
                    }
                    drive = self.drive(&drive.id)?;
                }
                let version = match kind {
                    BackupKind::Files => {
                        backup::files(&self.db, &drive, &destination, cancel, progress)?
                    }
                    BackupKind::Image => {
                        backup::image(&self.db, &drive, &destination, cancel, progress)?
                    }
                };
                Ok(format!(
                    "Backup completed: {} ({} bytes written)",
                    version.path.display(),
                    version.bytes_written
                ))
            }
            DriveRequest::Pair {
                drive,
                relative,
                local,
            } => {
                if !valid_relative(&relative) {
                    return Err("Invalid sync subtree".into());
                }
                let drive = self.drive(&drive)?;
                if drive.part_uuid.is_none() && drive.filesystem_uuid.is_none() {
                    return Err("Drive has no persistent partition or filesystem identity".into());
                }
                let mount = drive
                    .mount
                    .as_ref()
                    .ok_or("Mount the drive before pairing a folder")?;
                let source = Root::open(&mount.join(&relative))?;
                let local = Root::create(&local)?;
                if source.overlap(&local) {
                    return Err("Sync roots overlap".into());
                }
                let id = uuid::Uuid::new_v4().to_string();
                let pair = SyncPair {
                    local_identity: Some(local.storage_identity()?),
                    id: id.clone(),
                    drive: drive.id,
                    evidence: drive.evidence,
                    relative,
                    local: local.path,
                    paused: false,
                    deletions: false,
                    ram: false,
                    last_success: None,
                    status: "Waiting for initial union sync".into(),
                };
                self.db.put("pair", &id, &pair)?;
                self.synchronize(&id, cancel, progress)
            }
            DriveRequest::Sync { pair } | DriveRequest::FlushRam { session: pair } => {
                self.synchronize(&pair, cancel, progress)
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
            DriveRequest::Catalog { drive } => {
                let drive = self.drive(&drive)?;
                let catalog = catalog::capture(
                    &self.db,
                    &self.state,
                    &drive,
                    &self.config,
                    cancel,
                    progress,
                )?;
                Ok(format!(
                    "Catalog saved: {} · {}",
                    catalog.id, catalog.coverage
                ))
            }
            DriveRequest::MountCatalog { catalog } => self.mount(&catalog),
            DriveRequest::UnmountCatalog { catalog } => self.unmount(&catalog),
            DriveRequest::Rule {
                drive,
                relative,
                catalog,
                index,
            } => {
                if !valid_relative(&relative) {
                    return Err("Invalid rule subtree".into());
                }
                let id = format!(
                    "{}:{}",
                    drive,
                    little_clip_core::path_address::display_path(&relative)
                );
                let rule = DriveRule {
                    id: id.clone(),
                    drive,
                    relative,
                    catalog,
                    index,
                };
                self.db.put("rule", &id, &rule)?;
                Ok("Catalog and indexing policy updated; existing catalog versions remain dated snapshots".into())
            }
            DriveRequest::SaveRam {
                session,
                destination,
            } => self.save_ram(&session, &destination, cancel, progress),
            DriveRequest::DiscardRam { session, confirmed } => {
                if !confirmed {
                    return Err(
                        "Confirm discarding the RAM copy, including any unsynchronized edits"
                            .into(),
                    );
                }
                let session: RamSession =
                    self.db.get("ram", &session)?.ok_or("Unknown RAM session")?;
                let root = Root::open(&session.path)?;
                if little_clip_core::kernel_interface::MountIdentity::directory(&root.path)?
                    != session.identity
                {
                    return Err("RAM directory identity changed; refusing discard".into());
                }
                let base = Root::open(&self.config.ram_directory)?;
                if root.path.parent() != Some(base.path.as_path())
                    || !root
                        .path
                        .file_name()
                        .is_some_and(|n| n.as_encoded_bytes().starts_with(b"little-clip-ram-"))
                {
                    return Err("RAM session path is no longer owned".into());
                }
                let coverage = scan::scan(
                    &self.db,
                    &root,
                    "discard-check",
                    &scan::Limits {
                        hashes: false,
                        ..Default::default()
                    },
                    cancel,
                    |_, _| {},
                )?;
                if !coverage.complete {
                    return Err("Cannot validate RAM tree for discard".into());
                }
                root.validate()?;
                fs::remove_dir_all(&root.path)?;
                self.db.remove("pair", &session.id)?;
                self.db.remove("ram", &session.id)?;
                Ok("RAM copy discarded".into())
            }
            DriveRequest::Resolve { conflict } => {
                let mut conflict: DriveConflict = self
                    .db
                    .get("conflict", &conflict)?
                    .ok_or("Unknown conflict")?;
                conflict.resolved = true;
                self.db.put("conflict", &conflict.id, &conflict)?;
                Ok("Conflict acknowledged; preserved variants remain available".into())
            }
            DriveRequest::Status { .. } | DriveRequest::Cancel { .. } => {
                Err("Request must use coordinator admission".into())
            }
        }
    }
    fn synchronize(
        &mut self,
        id: &str,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        let mut pair: SyncPair = self.db.get("pair", id)?.ok_or("Unknown sync pair")?;
        let drive = self.drive(&pair.drive)?;
        if drive.evidence != pair.evidence {
            return Err(
                "Drive identity differs from the enrolled partition; sync suspended".into(),
            );
        }
        if drive.read_only && !(pair.ram && pair.last_success.is_none()) {
            return Err("Drive is read-only; edits remain local".into());
        }
        let mount = drive
            .mount
            .as_ref()
            .ok_or("Drive is offline; local contents retained")?;
        let portable = Root::open(&mount.join(&pair.relative))?;
        let local = Root::open(&pair.local)?;
        if pair.local_identity != Some(local.storage_identity()?) {
            return Err("Paired local directory was replaced; re-enroll it explicitly".into());
        }
        let result = sync::synchronize(&self.db, &pair, &portable, &local, cancel, progress)?;
        pair.last_success = Some(now());
        pair.status = format!(
            "{} files copied · {} conflicts",
            result.files, result.conflicts
        );
        self.db.put("pair", id, &pair)?;
        if let Some(watch) = &mut self.watch {
            watch.add(&portable.path);
            watch.add(&local.path);
        }
        if let Some(mut session) = self.db.get::<RamSession>("ram", id)? {
            session.status = pair.status.clone();
            session.ready = true;
            session.connected = true;
            self.db.put("ram", id, &session)?;
        }
        Ok(pair.status)
    }
    fn automatic(&mut self) {
        let pairs: Vec<SyncPair> = match self.db.list("pair", 0, 1000) {
            Ok(pairs) => pairs,
            Err(_) => return,
        };
        for mut pair in pairs {
            if pair.paused {
                continue;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            if let Err(error) = self.synchronize(&pair.id, &cancel, &mut |_, _| {}) {
                pair.status = error.to_string();
                let _ = self.db.put("pair", &pair.id, &pair);
                if let Ok(Some(mut session)) = self.db.get::<RamSession>("ram", &pair.id) {
                    session.status = pair.status.clone();
                    let _ = self.db.put("ram", &pair.id, &session);
                }
            }
        }
    }
    fn ram(
        &mut self,
        drive: &Drive,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        ram_reason(&self.config)?;
        if drive.drive_capacity == 0 || drive.drive_capacity >= self.config.ram_capacity_limit {
            return Err("Drive capacity exceeds the Copy to RAM limit or is unknown".into());
        }
        if self
            .db
            .list::<RamSession>("ram", 0, 1000)?
            .iter()
            .any(|s| s.drive == drive.id)
        {
            return Err("A RAM copy already exists for this partition".into());
        }
        let mount = drive
            .mount
            .as_ref()
            .ok_or("Mount the drive before copying it to RAM")?;
        let source = Root::open(mount)?;
        let base = Root::open(&self.config.ram_directory)?;
        let pin = kernel_interface::pin_content_directory(&base.path)?;
        let stats = rustix::fs::fstatvfs(&pin)?;
        if stats.f_bavail.saturating_mul(stats.f_frsize)
            < drive.capacity.saturating_add(64 * 1024 * 1024)
            || stats.f_favail < 1024
        {
            return Err("Insufficient tmpfs byte or inode headroom".into());
        }
        let temporary = tempfile::Builder::new()
            .prefix("little-clip-ram-")
            .tempdir_in(&base.path)?;
        if source.overlap(&Root::open(temporary.path())?) {
            return Err("Source overlaps RAM copy destination".into());
        }
        let path = temporary.keep();
        let id = uuid::Uuid::new_v4().to_string();
        let pair = SyncPair {
            local_identity: Some(Root::open(&path)?.storage_identity()?),
            id: id.clone(),
            drive: drive.id.clone(),
            evidence: drive.evidence.clone(),
            relative: PathBuf::new(),
            local: path.clone(),
            paused: true,
            deletions: false,
            ram: true,
            last_success: None,
            status: "Initial RAM copy in progress".into(),
        };
        let session = RamSession {
            identity: little_clip_core::kernel_interface::MountIdentity::directory(&path)?,
            id: id.clone(),
            drive: drive.id.clone(),
            path,
            ready: false,
            connected: true,
            status: pair.status.clone(),
        };
        self.db.put("ram", &id, &session)?;
        self.db.put("pair", &id, &pair)?;
        let result = self.synchronize(&id, cancel, progress)?;
        let mut pair: SyncPair = self.db.get("pair", &id)?.ok_or("RAM pair disappeared")?;
        pair.paused = false;
        self.db.put("pair", &id, &pair)?;
        Ok(format!(
            "RAM copy ready: {} · {result}",
            session.path.display()
        ))
    }
    fn save_ram(
        &self,
        id: &str,
        destination: &Path,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<String> {
        let session: RamSession = self.db.get("ram", id)?.ok_or("Unknown RAM session")?;
        let source = Root::open(&session.path)?;
        let base = Root::create(destination)?;
        let name = format!("ram-copy-{}", uuid::Uuid::new_v4());
        base.ensure_directory(Path::new(&name))?;
        let target = Root::open(&base.path.join(&name))?;
        if source.overlap(&target) {
            return Err("Save destination overlaps RAM source".into());
        }
        let scan_id = format!("save:{id}");
        let coverage = scan::scan(
            &self.db,
            &source,
            &scan_id,
            &scan::Limits::default(),
            cancel,
            &mut *progress,
        )?;
        if !coverage.complete {
            return Err("RAM copy could not be fully inventoried".into());
        }
        let mut after = Vec::new();
        let mut count = 0;
        let mut bytes = 0;
        loop {
            let entries = scan::entries(&self.db, &scan_id, &after, 128)?;
            if entries.is_empty() {
                break;
            }
            for entry in entries {
                bytes += sync::install(&source, &target, &entry, &entry.path, None, cancel, false)?;
                count += 1;
                after = entry.path.as_os_str().as_encoded_bytes().to_vec();
                progress(count, bytes);
            }
        }
        Ok(format!("RAM copy saved to {}", target.path.display()))
    }
    fn mount(&mut self, id: &str) -> Result<String> {
        if !Path::new("/dev/fuse").exists() {
            return Err("FUSE is unavailable: /dev/fuse is missing".into());
        }
        let mut catalog: Catalog = self.db.get("catalog", id)?.ok_or("Unknown catalog")?;
        if self.children.contains_key(id) {
            return Err("Catalog is already mounted".into());
        }
        let mounts = Root::create(&self.state.join("mounts"))?;
        mounts.ensure_directory(Path::new(id))?;
        let path = mounts.path.join(id);
        let helper = std::env::current_exe()?
            .parent()
            .ok_or("Missing executable directory")?
            .join("clip-drive-mount");
        let mut child = CommandRequest::new(helper)
            .arg(&self.state)
            .arg(id)
            .arg(&path)
            .own_process_group()
            .die_with_parent()
            .build_command()
            .spawn()?;
        std::thread::sleep(Duration::from_millis(200));
        if let Some(status) = child.try_wait()? {
            return Err(format!("Catalog mount helper exited: {status}").into());
        }
        catalog.mount = Some(path.clone());
        self.db.put("catalog", id, &catalog)?;
        self.children.insert(id.into(), child);
        Ok(format!("Catalog mounted at {}", path.display()))
    }
    fn unmount(&mut self, id: &str) -> Result<String> {
        let mut catalog: Catalog = self.db.get("catalog", id)?.ok_or("Unknown catalog")?;
        let path = catalog.mount.as_ref().ok_or("Catalog is not mounted")?;
        let output = CommandRequest::new("fusermount3")
            .arg("-u")
            .arg(path)
            .timeout(Duration::from_secs(10))
            .own_process_group()
            .run()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Could not unmount catalog: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        if let Some(mut child) = self.children.remove(id) {
            let _ = child.wait();
        }
        catalog.mount = None;
        self.db.put("catalog", id, &catalog)?;
        Ok("Catalog unmounted".into())
    }
    fn publish_policy(&self) -> Result<()> {
        let mut excluded = Vec::<PathBuf>::new();
        if let Some(path) = &self.config.backup_directory {
            excluded.push(path.clone());
        }
        for session in self.db.list::<RamSession>("ram", 0, 1000)? {
            excluded.push(session.path);
        }
        excluded.push(self.state.clone());
        let drives = self
            .shared
            .lock()
            .map_err(|_| "Drive status unavailable")?
            .drives
            .clone();
        for rule in self.db.list::<DriveRule>("rule", 0, 1000)? {
            if !rule.index {
                for drive in &drives {
                    if drive.id == rule.drive {
                        if let Some(mount) = &drive.mount {
                            excluded.push(mount.join(&rule.relative));
                        }
                    }
                }
            }
        }
        let json = serde_json::to_vec(&IndexPolicy { excluded })?;
        let root = kernel_interface::pin_content_directory(&self.state)?;
        let path = self.state.join("index-policy.json");
        if path.exists() {
            little_clip_core::file_source::FileSource::capture(&path)?
                .write_in(&path, &json, &root)?;
        } else {
            little_clip_core::file_source::create_file_in(&path, &json, &root)?;
        }
        Ok(())
    }
}
fn ram_reason(config: &DriveConfig) -> Result<()> {
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
    if available < config.ram_min_available {
        return Err("Insufficient available RAM before copying".into());
    }
    let root = kernel_interface::pin_content_directory(&config.ram_directory)?;
    if rustix::fs::fstatfs(&root)?.f_type as u32 != 0x01021994 {
        return Err("RAM destination must be a verified tmpfs filesystem".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status(service: &Service) -> DriveStatus {
        match service.request(DriveRequest::Status { offset: 0 }).unwrap() {
            DriveReply::Status(status) => *status,
            _ => panic!("status reply"),
        }
    }
    fn finish(service: &Service, request: DriveRequest) -> DriveOperation {
        let DriveReply::Accepted(id) = service.request(request).unwrap() else {
            panic!("accepted operation");
        };
        let started = Instant::now();
        loop {
            if let Some(op) = status(service)
                .operations
                .into_iter()
                .find(|o| o.id == id && o.finished.is_some())
            {
                assert_eq!(op.state, "complete", "{}", op.message);
                return op;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "operation did not finish"
            );
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    #[test]
    fn daemon_sync_and_ram_survive_client_closure_and_media_removal() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("portable");
        let local = temp.path().join("local");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("x"), b"initial").unwrap();
        let drive = Drive {
            id: "part:fixture".into(),
            label: "Fixture".into(),
            device: "/dev/not-real".into(),
            mount: Some(source.clone()),
            part_uuid: Some("fixture".into()),
            filesystem_uuid: Some("fs".into()),
            filesystem: "vfat".into(),
            capacity: 1024 * 1024,
            drive_capacity: 1024 * 1024,
            removable: true,
            read_only: false,
            evidence: "owned-fixture".into(),
            ambiguous: false,
        };
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
        finish(
            &service,
            DriveRequest::CopyToRam {
                drive: drive.id.clone(),
            },
        );
        let ram = status(&service).ram.into_iter().next().unwrap();
        assert!(ram.ready);
        inventory.lock().unwrap().clear();
        fs::rename(&source, temp.path().join("removed")).unwrap();
        fs::write(ram.path.join("x"), b"RAM offline").unwrap();
        assert_eq!(fs::read(ram.path.join("x")).unwrap(), b"RAM offline");
        std::thread::sleep(Duration::from_millis(1200));
        assert!(ram.path.exists());
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
        drop(service);
        std::thread::sleep(Duration::from_millis(300));
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
        finish(
            &service,
            DriveRequest::DiscardRam {
                session: ram.id,
                confirmed: true,
            },
        );
        assert!(!ram.path.exists());
        drop(service);
        std::thread::sleep(Duration::from_millis(300));
    }
    #[test]
    fn unset_backup_directory_and_duplicate_identity_fail_without_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        let drive = Drive {
            id: "part:duplicate".into(),
            label: "Duplicate".into(),
            device: "/dev/not-real".into(),
            mount: Some(source),
            part_uuid: Some("duplicate".into()),
            filesystem_uuid: None,
            filesystem: "vfat".into(),
            capacity: 1024,
            drive_capacity: 1024,
            removable: true,
            read_only: false,
            evidence: "fixture".into(),
            ambiguous: true,
        };
        let service = Service::start_with_inventory(
            DriveConfig::default(),
            temp.path().join("state"),
            Arc::new(move || Ok(vec![drive.clone()])),
        )
        .unwrap();
        let DriveReply::Accepted(id) = service
            .request(DriveRequest::Backup {
                drive: "part:duplicate".into(),
                kind: BackupKind::Image,
            })
            .unwrap()
        else {
            panic!()
        };
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
        drop(service);
        std::thread::sleep(Duration::from_millis(250));
    }
}
