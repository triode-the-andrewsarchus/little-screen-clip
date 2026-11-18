//! One retained preparation worker serializes quota admission and empty-image setup.
use super::*;
use little_clip_core::jobs::task::Task;
use serde::{Deserialize, Serialize};

pub(super) struct Pending {
    pub drive: Drive,
    pub source: Arc<Root>,
    operation: String,
    session: String,
    cancel: Arc<AtomicBool>,
    task: Task<Result<crate::ram::Mounted>>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Allocation {
    session: String,
    operation: String,
    pub backing: Option<RamBacking>,
}

impl Owner {
    pub(super) fn ram_setup_blocks(&self, request: &DriveRequest) -> bool {
        ((self.ram_access.is_some() || self.ram_recovery.busy() || self.settings_busy())
            && matches!(request, DriveRequest::CopyToRam { .. }))
            || self.ram_setup.is_some()
                && matches!(
                    request,
                    DriveRequest::CopyToRam { .. } | DriveRequest::Configure { .. }
                )
    }
    pub(super) fn start_ram_setup(
        &mut self,
        envelope: Envelope,
        source: Option<scheduling::Source>,
    ) {
        let prepared = (|| {
            let DriveRequest::CopyToRam { .. } = &envelope.request else {
                return Err("Invalid RAM setup request".into());
            };
            let source = source.ok_or("Mount the drive before copying it to RAM")?;
            let drive = source.drive;
            self.check_busy(&drive.id)?;
            eligible(&self.db, &self.config, &drive)?;
            Ok((drive, source.root))
        })();
        let (drive, source) = match prepared {
            Ok(value) => value,
            Err(error) => {
                self.finish_operation(&envelope.id, &envelope.cancel, Err(error));
                return;
            }
        };
        let session = uuid::Uuid::new_v4().to_string();
        if let Ok(Some(mut operation)) = self.db.get::<DriveOperation>("operation", &envelope.id) {
            operation.state = "preparing".into();
            operation.message = "Inventorying source for bounded RAM allocation".into();
            let _ = self.db.put("operation", &envelope.id, &operation);
        }
        let state = self.state.clone();
        let config = self.config.clone();
        let inventory = self.inventory.clone();
        let worker_source = source.clone();
        let worker_drive = drive.clone();
        let worker_session = session.clone();
        let operation = envelope.id.clone();
        let cancel = envelope.cancel.clone();
        let task = write_task(&self.ram_setup_jobs, move || {
            let db = Store::existing(&state.join("drives.sqlite"), false)?;
            let mut progress = job_progress(state.clone(), operation.clone());
            let result = Preparation {
                state: &state,
                config: &config,
                inventory: &inventory,
                drive: &worker_drive,
                source: &worker_source,
                id: &worker_session,
                operation: &operation,
            }
            .run(&db, &cancel, &mut progress);
            if let Err(error) = &result {
                cleanup_allocation(&db, &worker_session)
                    .map_err(|cleanup| format!("{error}; {cleanup}"))?;
            }
            result
        });
        self.ram_setup = Some(Pending {
            drive,
            source,
            operation: envelope.id,
            session,
            cancel: envelope.cancel,
            task,
        });
    }

    pub(super) fn poll_ram_setup(&mut self) {
        let Some(result) = self.ram_setup.as_ref().and_then(|job| job.task.poll()) else {
            return;
        };
        let job = self.ram_setup.take().expect("polled retained RAM setup");
        let result = result
            .unwrap_or_else(|error| Err(format!("RAM preparation worker failed: {error}").into()));
        match result {
            Ok(helper) => {
                self.ram_children.insert(job.session.clone(), helper);
                if job.drive.part_uuid.is_none() && job.drive.filesystem_uuid.is_none() {
                    self.unidentified_ram
                        .insert(job.session.clone(), job.source);
                }
                let intent = reconciliation::Intent {
                    queued: Instant::now(),
                    pair: job.session,
                    operation: Some(job.operation),
                    cancel: job.cancel,
                    paths: None,
                    initialize: true,
                    save: None,
                    writeback: None,
                };
                if self.closing.load(Ordering::Acquire) || intent.cancel.load(Ordering::Acquire) {
                    self.sync_result(
                        &intent,
                        Err("RAM setup finished; initial copy cancelled before population".into()),
                        None,
                    );
                } else if let Err(error) = self.publish_policy(false) {
                    self.sync_result(
                        &intent,
                        Err(format!(
                            "RAM image retained; index exclusion could not be published: {error}"
                        )
                        .into()),
                        None,
                    );
                } else {
                    self.queue_sync(intent);
                }
            }
            Err(error) => {
                let result = if self
                    .db
                    .get::<Allocation>("ram-allocation", &job.session)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    Err(format!("{error}; setup journal retained for restart recovery").into())
                } else {
                    Err(error)
                };
                self.finish_operation(&job.operation, &job.cancel, result);
            }
        }
    }
}

pub(super) fn recover_allocation(db: &Store, id: &str) -> Result<()> {
    let operation = db
        .get::<Allocation>("ram-allocation", id)?
        .map(|record| record.operation);
    let result = cleanup_allocation(db, id);
    if let Err(error) = &result
        && let Some(operation) = operation
        && let Some(mut record) = db.get::<DriveOperation>("operation", &operation)?
    {
        record.message = error.to_string();
        db.put("operation", &operation, &record)?;
    }
    result
}

fn eligible(db: &Store, config: &DriveConfig, drive: &Drive) -> Result<()> {
    if drive.ambiguous {
        return Err("RAM writeback requires an unambiguous drive identity".into());
    }
    if drive.drive_capacity == 0 || drive.drive_capacity >= config.ram_capacity_limit {
        return Err("Drive capacity exceeds the Copy to RAM limit or is unknown".into());
    }
    if db
        .list::<RamSession>("ram", 0, 1000)?
        .iter()
        .any(|session| session.drive == drive.id)
    {
        return Err("A RAM copy already exists for this partition".into());
    }
    if !db.list::<Allocation>("ram-allocation", 0, 1)?.is_empty() {
        return Err("An abandoned RAM allocation still needs identity-checked cleanup before reserving more RAM".into());
    }
    Ok(())
}

struct Preparation<'a> {
    state: &'a Path,
    config: &'a DriveConfig,
    inventory: &'a Inventory,
    drive: &'a Drive,
    source: &'a Root,
    id: &'a str,
    operation: &'a str,
}
impl Preparation<'_> {
    fn run(
        &self,
        db: &Store,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<crate::ram::Mounted> {
        cancelled(cancel)?;
        self.validate_source()?;
        eligible(db, self.config, self.drive)?;
        ram_reason(self.config)?;
        policy::Storage::capture(db, self.state, self.config.backup_directory.as_deref())?
            .validate_source(self.source)?;
        db.put(
            "ram-allocation",
            self.id,
            &Allocation {
                session: self.id.into(),
                operation: self.operation.into(),
                backing: None,
            },
        )?;
        let (files, bytes) = size_check(db, self.config, self.source, self.id, cancel, progress)?;
        self.validate_source()?;
        let backing = self.allocate(db, files, bytes, cancel)?;
        cancelled(cancel)?;
        self.validate_source()?;
        self.publish(db, backing)
    }
    fn validate_source(&self) -> Result<()> {
        if discovery::select((self.inventory)()?, &self.drive.id)? != *self.drive {
            return Err("Drive changed during RAM preparation".into());
        }
        self.source.validate()
    }
    fn allocate(
        &self,
        db: &Store,
        files: u64,
        bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<RamBacking> {
        let base = crate::ram::directory(self.config)?;
        let stats = rustix::fs::fstatvfs(base.directory(Path::new(""), false)?)?;
        let budget = crate::ram::budget(self.config, self.drive.capacity, files, bytes)?;
        crate::ram::admit(
            self.config,
            &budget,
            &db.list("ram", 0, 1000)?,
            &stats,
            available_memory()?,
        )?;
        cancelled(cancel)?;
        if let Some(mut operation) = db.get::<DriveOperation>("operation", self.operation)? {
            operation.message = format!(
                "Formatting RAM filesystem with a {} byte / {} inode reservation",
                budget.bytes, budget.inodes
            );
            db.put("operation", self.operation, &operation)?;
        }
        crate::ram::prepare(&base, &budget, |backing| {
            db.put(
                "ram-allocation",
                self.id,
                &Allocation {
                    session: self.id.into(),
                    operation: self.operation.into(),
                    backing: Some(backing.clone()),
                },
            )
        })
    }
    fn publish(&self, db: &Store, backing: RamBacking) -> Result<crate::ram::Mounted> {
        let id = self.id;
        let drive = self.drive;
        let path = backing.directory.join("contents");
        let pair = SyncPair {
            local_identity: None,
            id: id.into(),
            drive: drive.id.clone(),
            evidence: drive.evidence.clone(),
            relative: PathBuf::new(),
            local: path.clone(),
            paused: true,
            deletions: false,
            ram: true,
            last_success: None,
            status: "Initial RAM copy incomplete; waiting for population".into(),
        };
        let mut session = RamSession {
            identity: kernel_interface::MountIdentity::directory(&path)?,
            id: id.into(),
            drive: drive.id.clone(),
            path,
            ready: false,
            connected: true,
            status: pair.status.clone(),
            backing: Some(backing),
            last_saved: None,
            save_status: None,
            last_flushed: None,
            pending: None,
        };
        {
            let transaction = db.0.unchecked_transaction()?;
            db.put("ram", id, &session)?;
            db.put("pair", id, &pair)?;
            db.remove("ram-allocation", id)?;
            transaction.commit()?;
        }
        super::ram_sessions::start_view(db, &mut session)?
            .ok_or_else(|| "Missing prepared RAM view".into())
    }
}

fn size_check(
    db: &Store,
    config: &DriveConfig,
    source: &Root,
    id: &str,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(u64, u64),
) -> Result<(u64, u64)> {
    let id = format!("ram-preflight:{id}");
    let coverage = scan::scan(
        db,
        source,
        &id,
        &scan::Limits {
            hashes: false,
            entries: config.ram_session_max_entries.saturating_sub(128),
            ..Default::default()
        },
        cancel,
        progress,
    )?;
    if !coverage.complete {
        return Err(
            "RAM preflight could not inventory every entry within available inode headroom".into(),
        );
    }
    let bytes = db.0.query_row("SELECT coalesce(sum(json_extract(json,'$.size')),0) FROM entries WHERE scan=?1 AND json_extract(json,'$.kind')='file'", [&id], |row| row.get(0))?;
    db.clear_scan(&id)?;
    Ok((coverage.count, bytes))
}

fn cleanup_allocation(db: &Store, id: &str) -> Result<()> {
    if let Some(allocation) = db.get::<Allocation>("ram-allocation", id)? {
        if allocation.session != id {
            return Err(
                "RAM allocation address differs from its registry key; retaining data".into(),
            );
        }
        let published = policy::records::<RamSession>(db, "ram")?
            .into_iter()
            .any(|session| {
                session.id == id
                    || allocation
                        .backing
                        .as_ref()
                        .zip(session.backing.as_ref())
                        .is_some_and(|(a, b)| {
                            a.directory == b.directory
                                || [
                                    (&a.directory_identity, &b.directory_identity),
                                    (&a.image_identity, &b.image_identity),
                                ]
                                .into_iter()
                                .any(|(a, b)| (a.filesystem, a.inode) == (b.filesystem, b.inode))
                        })
            });
        if published {
            return Err(
                "Published RAM data has a stale allocation record; retaining both for inspection"
                    .into(),
            );
        }
        if let Some(backing) = &allocation.backing {
            match fs::symlink_metadata(&backing.directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                _ => crate::ram::discard(backing, id).map_err(|error| {
                    format!(
                        "Empty RAM allocation retained at {}: {error}",
                        backing.directory.display()
                    )
                })?,
            }
        }
    }
    db.clear_scan(&format!("ram-preflight:{id}"))?;
    db.remove("ram-allocation", id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restart_coordinator(db: Store, state: &Path) -> Service {
        drop(db);
        Service::start_with_inventory(
            DriveConfig::default(),
            state.into(),
            Arc::new(|| Ok(Vec::new())),
        )
        .unwrap()
    }

    #[test]
    fn interrupted_ram_preflight_retires_only_its_saved_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let source = Root::create(&temp.path().join("source")).unwrap();
        fs::write(source.path.join("keep"), b"original content").unwrap();
        let db = Store::open(&state.join("drives.sqlite")).unwrap();
        db.put(
            "ram-allocation",
            "fixture",
            &Allocation {
                session: "fixture".into(),
                operation: "interrupted".into(),
                backing: None,
            },
        )
        .unwrap();
        scan::scan(
            &db,
            &source,
            "ram-preflight:fixture",
            &scan::Limits {
                hashes: false,
                ..Default::default()
            },
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        let service = restart_coordinator(db, &state);
        let db = Store::existing(&state.join("drives.sqlite"), true).unwrap();
        let started = Instant::now();
        while db
            .get::<Allocation>("ram-allocation", "fixture")
            .unwrap()
            .is_some()
        {
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            db.get::<Allocation>("ram-allocation", "fixture")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            db.0.query_row(
                "SELECT count(*) FROM entries WHERE scan='ram-preflight:fixture'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            fs::read(source.path.join("keep")).unwrap(),
            b"original content"
        );
        service.shutdown().unwrap();
    }

    #[test]
    #[ignore = "requires verified tmpfs and mke2fs/debugfs; creates only owned empty RAM images"]
    fn ram_allocation_recovery_removes_only_unpublished_identity_checked_images() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let base = crate::ram::directory(&DriveConfig::default()).unwrap();
        let db = Store::open(&state.join("drives.sqlite")).unwrap();
        let allocate = |db: &Store| {
            crate::ram::prepare(
                &base,
                &crate::ram::Budget {
                    bytes: 128 * 1024 * 1024,
                    inodes: 4096,
                },
                |backing| {
                    db.put(
                        "ram-allocation",
                        "fixture",
                        &Allocation {
                            session: "fixture".into(),
                            operation: "owned-fixture".into(),
                            backing: Some(backing.clone()),
                        },
                    )
                },
            )
            .unwrap()
        };
        let abandoned = allocate(&db);
        let service = restart_coordinator(db, &state);
        let started = Instant::now();
        while abandoned.directory.exists() {
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !abandoned.directory.exists(),
            "startup should retire an empty unpublished image"
        );
        service.shutdown().unwrap();
        let db = Store::existing(&state.join("drives.sqlite"), false).unwrap();
        assert!(
            db.get::<Allocation>("ram-allocation", "fixture")
                .unwrap()
                .is_none()
        );

        let replaced = allocate(&db);
        let retained = replaced.directory.with_extension("retained");
        fs::rename(&replaced.directory, &retained).unwrap();
        fs::create_dir(&replaced.directory).unwrap();
        fs::write(replaced.directory.join("foreign"), b"preserve replacement").unwrap();
        assert!(cleanup_allocation(&db, "fixture").is_err());
        assert_eq!(
            fs::read(replaced.directory.join("foreign")).unwrap(),
            b"preserve replacement"
        );
        assert!(retained.join("volume.ext4").is_file());
        fs::remove_file(replaced.directory.join("foreign")).unwrap();
        fs::remove_dir(&replaced.directory).unwrap();
        fs::rename(retained, &replaced.directory).unwrap();
        let path = replaced.directory.join("contents");
        fs::write(path.join("edit"), b"published user edit").unwrap();
        let mut published = RamSession {
            identity: kernel_interface::MountIdentity::directory(&path).unwrap(),
            id: "published-under-another-key".into(),
            drive: "owned-fixture".into(),
            path: path.clone(),
            ready: false,
            connected: false,
            status: "retained".into(),
            backing: Some(replaced),
            last_saved: None,
            save_status: None,
            last_flushed: None,
            pending: None,
        };
        db.put("ram", &published.id, &published).unwrap();
        let error = cleanup_allocation(&db, "fixture").unwrap_err();
        assert!(error.to_string().contains("Published RAM data"), "{error}");
        assert_eq!(fs::read(path.join("edit")).unwrap(), b"published user edit");
        db.remove("ram", &published.id).unwrap();
        published.id = "fixture".into();
        db.put("ram", &published.id, &published).unwrap();
        assert!(cleanup_allocation(&db, "fixture").is_err());
        assert_eq!(fs::read(path.join("edit")).unwrap(), b"published user edit");
        // Explicit fixture teardown after proving published data is never auto-cleaned.
        fs::remove_file(path.join("edit")).unwrap();
        db.remove("ram", "fixture").unwrap();
        cleanup_allocation(&db, "fixture").unwrap();
    }
}
