//! Short-lived reviewed selections retain roots without mutating backup preferences.
use super::*;
use backup::Destination;

pub(super) type Previews = Arc<Mutex<HashMap<String, Review>>>;
pub(super) const LIFETIME: Duration = Duration::from_secs(300);
const MAX_PREVIEWS: usize = 16;
const HEADROOM: u64 = 64 * 1024 * 1024;

#[derive(serde::Deserialize)]
struct RetainedDestination {
    backup: Destination,
}

pub(super) fn managed_backup_paths(db: &Store) -> Result<Vec<PathBuf>> {
    let snapshot = if db.0.is_autocommit() {
        Some(db.0.unchecked_transaction()?)
    } else {
        None
    };
    let mut paths = Vec::new();
    db.visit::<Destination>("backup-root", |root| {
        paths.push(root.path);
        Ok(())
    })?;
    // Keep legacy destinations protected across the atomic completed-to-retiring
    // transition, including roots absent from the current preference.
    let mut collect = |path: PathBuf| {
        if let Some(base) = path.parent().and_then(Path::parent) {
            paths.push(base.to_path_buf());
        }
    };
    db.visit::<Destination>("backup", |version| {
        collect(version.path);
        Ok(())
    })?;
    for kind in ["backup-publication", "backup-prune"] {
        db.visit::<RetainedDestination>(kind, |retained| {
            collect(retained.backup.path);
            Ok(())
        })?;
    }
    if let Some(snapshot) = snapshot {
        snapshot.commit()?;
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[derive(Clone)]
pub(super) struct Review {
    pub info: BackupPreview,
    pub source: Option<Arc<Root>>,
    pub destination: Arc<Root>,
    pub(super) created: Instant,
    pub(super) remember: bool,
}
impl Review {
    fn capture(drive: Drive, kind: BackupKind, path: &Path, state: &Path) -> Result<Self> {
        // A cancelled review creates neither directories nor preference writes.
        let destination = Arc::new(Root::open(path)?);
        let source = drive
            .mount
            .as_ref()
            .map(|path| Root::open(path).map(Arc::new))
            .transpose()?;
        if kind == BackupKind::Files && source.is_none() {
            return Err("Mount the partition before reviewing a file backup".into());
        }
        let db = Store::existing(&state.join("drives.sqlite"), true)?;
        policy::validate_backup_storage(&db, state, &destination)?;
        if source
            .as_ref()
            .is_some_and(|source| source.overlap(&destination))
        {
            return Err("Backup destination overlaps the source partition".into());
        }
        let info = BackupPreview {
            token: uuid::Uuid::new_v4().to_string(),
            space: space(&destination, &drive, kind)?,
            drive,
            kind,
            destination: destination.path.clone(),
        };
        Ok(Self {
            info,
            source,
            destination,
            created: Instant::now(),
            remember: false,
        })
    }
    pub fn validate_lifetime(&self) -> Result<()> {
        if self.created.elapsed() >= LIFETIME {
            return Err("Backup review expired; review the partition and destination again".into());
        }
        Ok(())
    }
    pub fn validate(&self, drive: &Drive) -> Result<()> {
        self.validate_lifetime()?;
        if drive != &self.info.drive {
            return Err(
                "Selected partition changed since review; review it again before backing up".into(),
            );
        }
        self.destination.validate()?;
        if let Some(source) = &self.source {
            source.validate()?;
        }
        Ok(())
    }
}

pub(super) fn expire(previews: &Previews) {
    if let Ok(mut previews) = previews.lock() {
        previews.retain(|_, review| review.created.elapsed() < LIFETIME);
    }
}

pub(super) fn space(root: &Root, drive: &Drive, kind: BackupKind) -> Result<BackupSpace> {
    if kind == BackupKind::Image {
        root.check_file_size(drive.capacity)?;
    }
    let (pin, stats) = root.writable_directory().map_err(|error| {
        format!("Backup destination is not writable/searchable by this user: {error}")
    })?;
    let available_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
    let btrfs = rustix::fs::fstatfs(&pin)?.f_type as u32 == 0x9123683e;
    let full_image_bytes = if kind == BackupKind::Image {
        if drive.capacity == 0 {
            return Err("Partition capacity is unknown; cannot estimate a complete image".into());
        }
        Some(drive.capacity.saturating_add(HEADROOM))
    } else {
        None
    };
    let required = if btrfs {
        HEADROOM
    } else {
        full_image_bytes.unwrap_or(HEADROOM)
    };
    if available_bytes < required {
        return Err(format!("Backup destination has {available_bytes} available bytes; at least {required} bytes are required for this backup's space admission").into());
    }
    Ok(BackupSpace {
        available_bytes,
        full_image_bytes,
        btrfs,
    })
}

pub(super) fn destination(
    db: &Store,
    state: &Path,
    configured: Option<&Path>,
    review: Option<&Review>,
    source: Option<&Root>,
) -> Result<Arc<Root>> {
    let destination = match review {
        Some(review) => {
            review.destination.validate()?;
            review.destination.clone()
        }
        None => Arc::new(Root::create(
            configured.ok_or("Choose a DriveBackupDir before starting a backup")?,
        )?),
    };
    policy::validate_backup_storage(db, state, &destination)?;
    if source.is_some_and(|source| source.overlap(&destination)) {
        return Err("Backup destination overlaps the source partition".into());
    }
    Ok(destination)
}

impl Service {
    pub(super) fn preview_backup(
        &self,
        selector: &str,
        kind: BackupKind,
        destination: Option<PathBuf>,
    ) -> Result<DriveReply> {
        if self.runtime.closing.load(Ordering::Acquire) {
            return Err("Drive services are shutting down; admission is closed".into());
        }
        expire(&self.previews);
        let drive = discovery::select((self.inventory)()?, selector)?;
        let path = destination
            .or_else(|| {
                self.shared
                    .lock()
                    .ok()
                    .and_then(|s| s.backup_directory.clone())
            })
            .ok_or("Choose a DriveBackupDir before reviewing a backup")?;
        let review = Review::capture(drive, kind, &path, &self.state)?;
        let info = review.info.clone();
        review.validate(&discovery::select((self.inventory)()?, selector)?)?;
        let _admission = self
            .runtime
            .tokens
            .lock()
            .map_err(|_| "Drive admission unavailable")?;
        if self.runtime.closing.load(Ordering::Acquire) {
            return Err("Drive services are shutting down; admission is closed".into());
        }
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| "Backup review storage unavailable")?;
        if previews.len() >= MAX_PREVIEWS {
            return Err(
                "Too many pending backup reviews; wait for an older review to expire".into(),
            );
        }
        previews.insert(info.token.clone(), review);
        Ok(DriveReply::BackupPreview(Box::new(info)))
    }

    pub(super) fn take_backup_review(
        &self,
        request: DriveRequest,
    ) -> Result<(DriveRequest, Option<Review>)> {
        let (token, restore_mounted, remember_destination, read_only) = match request {
            DriveRequest::StartBackup {
                token,
                restore_mounted,
                remember_destination,
            } => (token, restore_mounted, remember_destination, false),
            DriveRequest::StartReadOnlyBackup {
                token,
                remember_destination,
            } => (token, false, remember_destination, true),
            _ => return Ok((request, None)),
        };
        let mut previews = self
            .previews
            .lock()
            .map_err(|_| "Backup review storage unavailable")?;
        let review = previews
            .get(&token)
            .ok_or("Backup review is unavailable or already used; review again")?;
        if review.created.elapsed() >= LIFETIME {
            previews.remove(&token);
            return Err("Backup review expired; review the partition and destination again".into());
        }
        if read_only && review.info.kind != BackupKind::Files {
            return Err("Read-only file collection requires a file backup review; partition imaging already uses exclusive unmounted access".into());
        }
        let mut review = previews
            .remove(&token)
            .expect("validated review under lock");
        review.remember = remember_destination;
        let request = if read_only {
            DriveRequest::ReadOnlyBackup {
                drive: review.info.drive.id.clone(),
            }
        } else {
            DriveRequest::Backup {
                drive: review.info.drive.id.clone(),
                kind: review.info.kind,
                restore_mounted,
            }
        };
        Ok((request, Some(review)))
    }
}

impl Owner {
    pub(super) fn admitted_source(
        &self,
        envelope: &Envelope,
        drive: &Drive,
    ) -> Result<Option<Arc<Root>>> {
        match &envelope.review {
            Some(review) => {
                review.validate(drive)?;
                Ok(review.source.clone())
            }
            None => drive
                .mount
                .as_ref()
                .map(|path| Root::open(path).map(Arc::new))
                .transpose(),
        }
    }
    pub(super) fn validate_backup_request(&self, envelope: &Envelope) -> Result<()> {
        if !matches!(
            envelope.request,
            DriveRequest::Backup { .. } | DriveRequest::ReadOnlyBackup { .. }
        ) {
            return Ok(());
        }
        if let Some(review) = &envelope.review {
            review.validate_lifetime()
        } else if self.config.backup_directory.is_none() {
            Err("Choose a DriveBackupDir before starting a backup".into())
        } else {
            Ok(())
        }
    }

    pub(super) fn admit_backup_destination(
        &mut self,
        envelope: &Envelope,
        destination: Option<&Root>,
    ) -> Result<()> {
        if envelope
            .review
            .as_ref()
            .is_some_and(|review| review.remember)
        {
            return Err("Remembered destinations require retained preference admission".into());
        }
        if let Some(destination) = destination {
            backup::remember_destination(&self.db, &destination.path)?;
            self.publish_policy(false)?;
        }
        Ok(())
    }
}
