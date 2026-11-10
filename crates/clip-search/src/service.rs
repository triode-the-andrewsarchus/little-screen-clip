//! One background owner for index transactions, inference, and reconciliation work.
mod readers;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
use super::{
    answer::{self, Answer, State},
    query::{Operator, Query, Scope},
    reader::{self, Prepared, Reply, Request, SourceRoot},
    runtime::{Model, Paths, Role},
    store::{Checkpoint, RootState, Segment, Store},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use little_clip_core::config::FileSearchConfig;
use little_clip_core::file_inventory::Entry;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub enum Command {
    Backup(PathBuf),
    Restore(PathBuf),
    Clear,
    Recover,
    Preferences(FileSearchConfig),
    Enroll(PathBuf),
    Rebuild(PathBuf),
    Remove(PathBuf),
    Setup,
    SetupAnswer,
    Answer(Box<AnswerJob>),
    Search {
        id: u64,
        path: PathBuf,
        query: Query,
        hidden: bool,
        cancel: Arc<AtomicBool>,
    },
}
pub struct AnswerJob {
    pub id: u64,
    pub root: SourceRoot,
    pub entry: Entry,
    pub question: String,
    pub kind: super::query::AnswerType,
    pub cancel: Arc<AtomicBool>,
    pub plans: answer::extraction::SharedPlans,
}
pub struct AnswerResult {
    pub id: u64,
    pub answer: Answer,
}
pub struct SearchResult {
    pub id: u64,
    pub complete: bool,
    pub result: Result<super::Results, String>,
}
pub struct SearchProgress {
    pub id: u64,
    pub checked: Arc<AtomicU64>,
}
pub enum ResultReset {
    Semantic,
    Scope,
}
#[derive(Default)]
/// Latest coordinator observations and request-scoped progress/completion.
pub struct Snapshot {
    pub roots: Vec<RootState>,
    pub settings: FileSearchConfig,
    pub message: String,
    pub error: Option<String>,
    pub backend: Option<String>,
    pub capacity: Option<String>,
    pub has_index: bool,
    pub maintenance: bool,
    pub maintenance_outcome: Option<Result<String, String>>,
    pub clear_results: Option<ResultReset>,
    pub result: Option<SearchResult>,
    pub progress: Option<SearchProgress>,
    pub answers: VecDeque<AnswerResult>,
    pub answer_pipeline: Option<String>,
    pub reused_vectors: u64,
    pub generated_vectors: u64,
    notifier: Option<crate::ChangeNotifier>,
}
impl Snapshot {
    pub fn checked_segments(&self, request: u64) -> Option<u64> {
        self.progress
            .as_ref()
            .filter(|progress| progress.id == request)
            .map(|progress| progress.checked.load(Ordering::Relaxed))
    }
}

fn publish_search_result(snapshot: &Mutex<Snapshot>, result: SearchResult) {
    let notifier = {
        let mut snapshot = snapshot.lock().unwrap();
        if result.complete {
            snapshot.progress = None;
        }
        snapshot.result = Some(result);
        snapshot.notifier.clone()
    };
    // Consumer callbacks can inspect the snapshot; never wake while holding its lock.
    if let Some(notify) = notifier {
        notify();
    }
}
// Command admission and starting a background model step share this lock.
// A queued command cannot be lost between clearing cancellation and inference.
#[derive(Default)]
struct Dispatch {
    queued: usize,
    stopped: bool,
    interactive: Option<Arc<AtomicBool>>,
    cancel: Arc<AtomicBool>,
}
impl Dispatch {
    fn close_admission(&mut self) {
        self.stopped = true;
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(cancel) = self.interactive.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    fn begin_background(&mut self) -> bool {
        if self.queued != 0 || self.stopped {
            return false;
        }
        self.cancel.store(false, Ordering::Relaxed);
        true
    }
}

/// Owns one background search coordinator and its accepted command lifecycle.
///
/// The host polls request-scoped snapshots and retains this owner until
/// [`Self::allow_close`] succeeds. Dropping it is not a substitute for settling
/// accepted maintenance writes. Reader helpers remain explicitly dispatched by
/// the host executable before normal application startup.
pub struct Service {
    commands: mpsc::SyncSender<Command>,
    dispatch: Arc<Mutex<Dispatch>>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Service {
    /// Starts the existing coordinator with private paths and guarded preferences.
    ///
    /// # Errors
    /// Returns worker-thread startup failure. Invalid exclusions are published
    /// as a snapshot error with conservative exclusion, not silently accepted.
    pub fn start(home: &Path, config: FileSearchConfig) -> Result<Self, String> {
        let paths = Paths::for_user(home);
        let (exclusions, policy_error) = match super::policy::exclusions(&config.excluded_paths) {
            Ok(paths) => (paths, None),
            Err(error) => (vec![PathBuf::from("/")], Some(error)),
        };
        let (commands, incoming) = mpsc::sync_channel(8);
        let snapshot = Arc::new(Mutex::new(Snapshot {
            settings: config.clone(),
            error: policy_error,
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let dispatch = Arc::new(Mutex::new(Dispatch::default()));
        let mut worker = Worker {
            paths,
            config,
            exclusions,
            store: None,
            model: None,
            dispatch: dispatch.clone(),
            snapshot: snapshot.clone(),
            stop: stop.clone(),
            work: VecDeque::new(),
            pipeline: String::new(),
            last_snapshot: Instant::now(),
            roots_dirty: false,
            finishing: Default::default(),
            pruning: None,
            pruning_dirty: false,
            answers_until: None,
            monitor: super::monitor::Monitor::default(),
            capacity_blocked: false,
            readers: Default::default(),
            staged_files: Vec::new(),
            next_probe: 0,
        };
        let thread = std::thread::Builder::new()
            .name("file-search".into())
            .spawn(move || worker.run(incoming))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            dispatch,
            snapshot,
            stop,
            worker: Some(thread),
        })
    }
    /// Returns whether accepted index maintenance still owns completion.
    pub fn maintenance_busy(&self) -> bool {
        self.snapshot.lock().unwrap().maintenance
    }
    /// Closes admission and polls accepted work and child retirement without waiting.
    ///
    /// Returns `false` until queued controls, maintenance and the coordinator have
    /// settled. The host must keep polling rather than discard this service.
    pub fn allow_close(&mut self) -> bool {
        let queued = {
            let mut dispatch = self.dispatch.lock().unwrap();
            dispatch.close_admission();
            dispatch.queued != 0
        };
        // Let already admitted controls and accepted maintenance settle. Their
        // write lifetime is independent of read cancellation/admission closure.
        if queued || self.maintenance_busy() {
            return false;
        }
        self.stop.store(true, Ordering::Relaxed);
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            return false;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join(); // is_finished: never wait on the UI thread.
        }
        true
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        let cancel = match &command {
            Command::Search { cancel, .. } => Some(cancel.clone()),
            Command::Answer(job) => Some(job.cancel.clone()),
            _ => None,
        };
        let maintenance = matches!(
            &command,
            Command::Backup(_) | Command::Restore(_) | Command::Clear | Command::Recover
        );
        let mut snapshot = self.snapshot.lock().unwrap();
        if maintenance && snapshot.maintenance {
            return Err("Index maintenance is already in progress".into());
        }
        if maintenance {
            snapshot.maintenance = true;
        }
        let mut dispatch = self.dispatch.lock().unwrap();
        if dispatch.stopped {
            if maintenance {
                snapshot.maintenance = false;
            }
            return Err("Search is shutting down".into());
        }
        if self.commands.try_send(command).is_err() {
            if maintenance {
                snapshot.maintenance = false;
            }
            return Err("Search is busy; retry shortly".into());
        }
        dispatch.queued += 1;
        dispatch.cancel.store(true, Ordering::Relaxed);
        if let Some(old) = std::mem::replace(&mut dispatch.interactive, cancel.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        drop(dispatch);
        if maintenance {
            snapshot.maintenance_outcome = None;
        }
        if cancel.is_some() {
            snapshot.progress = None;
        }
        drop(snapshot);
        Ok(())
    }

    pub fn set_notifier(&self, notify: crate::ChangeNotifier) {
        let (available, previous) = {
            let mut snapshot = self.snapshot.lock().unwrap();
            let previous = snapshot.notifier.replace(notify.clone());
            (snapshot.result.is_some(), previous)
        };
        // Observer teardown may also access state; keep it outside the lock.
        drop(previous);
        // Cover a completion that raced ahead of the first consumer registration.
        if available {
            notify();
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.dispatch.lock().unwrap().close_admission();
    }
}

const MAX_FILE_SEGMENTS: usize = 2048;
// Two 64-MiB retained file reservations leave half of the 256-MiB staging
// allowance for the two bounded reader transports and one inference request.
// This excludes the separately bounded metadata inventory and model weights.
const MAX_STAGED_FILES: usize = 2;

#[derive(Clone, Copy)]
enum SourceCheck {
    Pending,
    Extract(Option<[u8; 32]>),
    Reuse([u8; 32]),
}
struct FileWork {
    root: SourceRoot,
    root_id: i64,
    generation: i64,
    publishing: bool,
    entry: Entry,
    id: i64,
    cursor: usize,
    prepared: Option<Prepared>,
    text_cursor: usize,
    segments: Vec<Segment>,
    coverage: String,
    source_check: SourceCheck,
    published: usize,
    resume_hash: Option<[u8; 32]>,
    resume_pending: bool,
    failed: bool,
    staging: Option<Arc<i64>>,
}
impl FileWork {
    fn new(root: SourceRoot, root_id: i64, generation: i64, entry: Entry, id: i64) -> Self {
        // A name-only record starts at final validation. It has no content
        // checkpoint, digest or decoder stage to resume.
        let (publishing, coverage, source_check) = if reader::supports_content(&entry) {
            (false, "complete", SourceCheck::Pending)
        } else {
            (
                true,
                reader::NAME_ONLY_UNSUPPORTED,
                SourceCheck::Extract(None),
            )
        };
        Self {
            root,
            root_id,
            generation,
            publishing,
            entry,
            id,
            cursor: 0,
            prepared: None,
            text_cursor: 0,
            segments: Vec::new(),
            coverage: coverage.into(),
            source_check,
            published: 0,
            resume_hash: None,
            resume_pending: false,
            failed: false,
            staging: None,
        }
    }
}
struct InventoryWork {
    root: SourceRoot,
    id: i64,
    generation: i64,
    path: PathBuf,
    recursive: bool,
    pipeline: String,
    invalidated: bool,
    remaining_bytes: u64,
    _permit: Option<Arc<()>>,
    entries: VecDeque<Entry>,
}

enum Work {
    Probe {
        id: i64,
        path: PathBuf,
        persist: bool,
    },
    Directory {
        root: SourceRoot,
        id: i64,
        generation: i64,
        path: PathBuf,
        recursive: bool,
        // Some resumes retirement after this directory's complete inventory.
        remaining: Option<VecDeque<PathBuf>>,
    },
    Inventory(Box<InventoryWork>),
    File(Box<FileWork>),
}
impl Work {
    fn path(&self) -> &Path {
        match self {
            Self::Probe { path, .. } | Self::Directory { path, .. } => path,
            Self::Inventory(inventory) => &inventory.path,
            Self::File(file) => &file.entry.path,
        }
    }
    fn root_id(&self) -> i64 {
        match self {
            Self::Probe { id, .. } | Self::Directory { id, .. } => *id,
            Self::Inventory(inventory) => inventory.id,
            Self::File(file) => file.root_id,
        }
    }
}
struct Worker {
    paths: Paths,
    config: FileSearchConfig,
    exclusions: Vec<PathBuf>,
    store: Option<Store>,
    model: Option<Model>,
    dispatch: Arc<Mutex<Dispatch>>,
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    work: VecDeque<Work>,
    pipeline: String,
    last_snapshot: Instant,
    roots_dirty: bool,
    finishing: VecDeque<i64>,
    pruning: Option<super::store::PruneCursor>,
    pruning_dirty: bool,
    answers_until: Option<Instant>,
    monitor: super::monitor::Monitor,
    capacity_blocked: bool,
    readers: readers::Readers,
    staged_files: Vec<Weak<i64>>,
    // Fresh probes have temporary negative IDs; stored roots/imports use positive IDs.
    next_probe: i64,
}
impl Worker {
    fn run(&mut self, incoming: mpsc::Receiver<Command>) {
        self.restore_enrollment();
        while !self.stop.load(Ordering::Relaxed) {
            match incoming.recv_timeout(if self.background_ready() {
                Duration::ZERO
            } else if !self.readers.is_empty() {
                Duration::from_millis(10)
            } else {
                Duration::from_millis(100)
            }) {
                Ok(command) => {
                    self.dispatch.lock().unwrap().queued -= 1;
                    let maintenance = matches!(
                        &command,
                        Command::Backup(_)
                            | Command::Restore(_)
                            | Command::Clear
                            | Command::Recover
                    );
                    let result = self.command(command);
                    let failure = result.as_ref().err().cloned();
                    self.report(result);
                    self.check_storage_fault();
                    if maintenance {
                        let mut snapshot = self.snapshot.lock().unwrap();
                        snapshot.maintenance = false;
                        snapshot.maintenance_outcome =
                            Some(failure.map_or(Ok(snapshot.message.clone()), Err));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            self.background_tick();
        }
        self.cancel_reads(None);
        self.model = None;
        self.work.clear();
        while !self.readers.is_empty() {
            self.readers.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn background_tick(&mut self) {
        if self.dispatch.lock().unwrap().stopped {
            return;
        }
        self.readers.poll();
        self.monitor_changes();
        self.check_storage_fault();
        if let Some(store) = &self.store {
            store.release_idle_vectors(Duration::from_secs(300));
        }
        if let Some(work) = self.background_ready().then(|| self.next_work()).flatten() {
            self.advance(work);
        }
        self.finish_root();
        self.prune_background();
        if self.roots_dirty {
            self.update_roots(false);
        }
        if !self.config.keep_model_loaded
            && self
                .model
                .as_ref()
                .is_some_and(|model| model.last_used.elapsed() > Duration::from_secs(300))
        {
            self.model = None;
            self.snapshot.lock().unwrap().backend = None;
        }
        self.snapshot.lock().unwrap().backend = self.model.as_ref().and_then(Model::backend_status);
    }
    fn restore_enrollment(&mut self) {
        self.snapshot.lock().unwrap().has_index =
            std::fs::symlink_metadata(self.paths.data.join("index.sqlite"))
                .is_ok_and(|metadata| metadata.is_file());
        // Empty defaults do not create a DB, load a model, or inspect any roots.
        if !self.config.roots.is_empty() {
            let result = self.database().map(|_| ());
            self.report(result);
            self.update_roots(true);
            if let Some(store) = &self.store
                && let Ok(roots) = store.monitor_roots()
            {
                for (id, root) in roots {
                    if super::policy::excluded(&root.path).is_none()
                        && self.config.roots.iter().any(|address| {
                            little_clip_core::path_address::decode_path(address)
                                .ok()
                                .as_ref()
                                == Some(&root.path)
                        })
                    {
                        self.monitor.register(id, root);
                    }
                }
            }
        }
        for address in self.config.roots.clone() {
            if let Ok(path) = little_clip_core::path_address::decode_path(&address) {
                let result = self.enroll(&path, false);
                self.report(result);
            }
        }
    }
    fn monitor_changes(&mut self) {
        let invalidations = self.monitor.poll();
        self.invalidate_reads(invalidations);
        if self.config.paused
            || self.capacity_blocked
            || self
                .answers_until
                .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let busy = self
            .work
            .iter()
            .map(Work::root_id)
            .chain(self.finishing.iter().copied())
            .collect();
        for refresh in self.monitor.due(&busy, Instant::now()) {
            let result = match refresh.directories {
                None => self.enroll(&refresh.root.path, false),
                Some(directories) => self.queue_delta(refresh.id, refresh.root, directories),
            };
            if result.is_err() {
                self.monitor.failed(refresh.id);
            }
            self.report(result);
        }
    }
    fn cancel_reads(&mut self, root: Option<i64>) {
        if let Some(root) = root {
            self.finishing.retain(|id| *id != root);
        } else {
            self.finishing.clear();
        }
        if root.is_none() {
            self.pruning = None;
            self.pruning_dirty = false;
        }
        let watches = match root {
            Some(root) => self.readers.cancel_root(root).into_iter().collect(),
            None => self.readers.clear(),
        };
        for instance in watches {
            self.monitor.abandon_watch(&instance);
        }
    }
    fn invalidate_reads(&mut self, mut invalidations: Vec<(i64, Option<PathBuf>)>) {
        for (id, _) in &invalidations {
            self.cancel_reads(Some(*id));
        }
        // Abandoning a registration can retire its shared instance. Drain that
        // gap before any replacement job is admitted, without polling I/O again.
        let gaps = self.monitor.take_invalidations();
        for (id, _) in &gaps {
            self.cancel_reads(Some(*id));
        }
        invalidations.extend(gaps);
        if invalidations.is_empty() {
            return;
        }
        // The coordinator owns buffered applications between slices. Mark only
        // affected parents; unrelated hot folders must not restart this scan.
        let affected: HashSet<_> = invalidations
            .iter()
            .map(|(id, path)| (*id, path.as_deref()))
            .collect();
        for work in &mut self.work {
            if let Work::Inventory(inventory) = work
                && (affected.contains(&(inventory.id, None))
                    || affected.contains(&(inventory.id, Some(inventory.path.as_path()))))
            {
                inventory.invalidated = true;
            }
        }
        if let Some(store) = &self.store {
            for (id, path) in invalidations {
                self.report(store.invalidate_directory(id, path.as_deref()));
            }
        }
        self.update_roots(true);
    }
    fn queue_delta(
        &mut self,
        id: i64,
        root: SourceRoot,
        directories: Vec<PathBuf>,
    ) -> Result<(), String> {
        if self.work.len().saturating_add(directories.len()) > 100_000 {
            return Err("Index work queue is full; reconciliation will retry".into());
        }
        let generation = self.database()?.begin_delta(id)?;
        for path in directories {
            self.work.push_back(Work::Directory {
                root: root.clone(),
                id,
                generation,
                path,
                recursive: false,
                remaining: None,
            });
        }
        self.update_roots(true);
        Ok(())
    }
    fn ready_work(&self) -> Option<usize> {
        let can_stage = self
            .staged_files
            .iter()
            .filter(|slot| slot.strong_count() != 0)
            .count()
            < MAX_STAGED_FILES;
        let indexing = !self.config.paused
            && !self.capacity_blocked
            && self
                .answers_until
                .is_none_or(|until| Instant::now() >= until);
        self.work.iter().position(|work| {
            (matches!(work, Work::Probe { .. }) || indexing)
                && !matches!(work, Work::File(file) if file.staging.is_none() && self.staged_files.iter().filter_map(Weak::upgrade).any(|owner| *owner == file.id))
                && (can_stage
                    || !matches!(work, Work::File(file) if !file.publishing && file.staging.is_none()))
                && (!matches!(
                    work,
                    Work::Directory {
                        remaining: None,
                        ..
                    }
                ) || self.readers.inventory_available(work.root_id()))
                && self.readers.can_run(work.root_id(), work.path())
        })
    }
    fn next_work(&mut self) -> Option<Work> {
        // Admission and cancellation reset must be atomic with Service::send.
        if !self.dispatch.lock().unwrap().begin_background() {
            return None;
        }
        let mut work = self.work.remove(self.ready_work()?)?;
        if let Work::File(file) = &mut work
            && !file.publishing
            && !self.reserve_file_staging(file)
        {
            self.work.push_back(work);
            return None;
        }
        Some(work)
    }
    fn background_ready(&self) -> bool {
        self.ready_work().is_some() || !self.finishing.is_empty() || self.pruning.is_some()
    }
    fn pause_capacity(&mut self, reason: String) {
        self.capacity_blocked = true;
        self.snapshot.lock().unwrap().capacity = Some(format!("Indexing paused: {reason}"));
        self.update_roots(true);
    }
    fn retry_capacity(&mut self) {
        self.capacity_blocked = false;
        self.snapshot.lock().unwrap().capacity = None;
        if let Some(store) = &self.store {
            self.report(store.reset_storage_fault());
        }
    }
    fn check_storage_fault(&mut self) {
        if !self.capacity_blocked && self.store.as_ref().is_some_and(Store::storage_failed) {
            self.pause_capacity("Index storage is not writable · check disk space/permissions and retry in Search settings".into());
        }
    }
    fn admit(
        &mut self,
        file: Option<&FileWork>,
        extra: u64,
        inventory_bytes: u64,
    ) -> Result<bool, String> {
        let limits = self.config.limits.clone();
        let reason = match file {
            Some(file) => self.database()?.capacity(
                Some(file.id),
                &file.segments,
                extra,
                file.prepared
                    .as_ref()
                    .and_then(|prepared| prepared.data.as_ref())
                    .map_or(0, |data| data.len() as u64),
                &limits,
            )?,
            None => self
                .database()?
                .capacity(None, &[], extra, inventory_bytes, &limits)?,
        };
        if let Some(reason) = reason {
            self.pause_capacity(reason);
            return Ok(false);
        }
        Ok(true)
    }

    fn advance(&mut self, work: Work) {
        // Enrollment does not admit source contents or finish a catalog scan.
        // Its result must pass the normal source/policy checks before either.
        if matches!(work, Work::Probe { .. }) {
            let result = self.step(work);
            self.report(result);
            self.update_roots(true);
            return;
        }
        let root_id = work.root_id();
        self.pruning.get_or_insert_with(Default::default);
        self.pruning_dirty = true;
        let admitted = self.admit(
            match &work {
                Work::File(file) => Some(file),
                _ => None,
            },
            0,
            match &work {
                Work::Inventory(inventory) => inventory.remaining_bytes,
                _ => 0,
            },
        );
        if !matches!(admitted, Ok(true)) {
            if let Err(error) = admitted {
                self.pause_capacity(format!("Cannot verify index storage: {error}"));
            }
            self.work.push_front(work);
            return;
        }
        if let Err(error) = self.step(work) {
            if self.shutdown_cancelled(&error) {
                return;
            }
            if self.store.as_ref().is_some_and(Store::storage_failed) {
                self.pause_capacity(format!("Index storage write failed: {error} · check disk space and retry in Search settings"));
            }
            if let Some(store) = &self.store {
                let _ = store.finish_scan(root_id, Some(&error));
            }
            self.cancel_reads(Some(root_id));
            self.work.retain(|work| work.root_id() != root_id);
            self.monitor.failed(root_id);
            self.model = None;
            self.snapshot.lock().unwrap().backend = None;
            self.report(Err(error));
        } else if !self.work.iter().any(|work| work.root_id() == root_id)
            && !self.finishing.contains(&root_id)
        {
            self.finishing.push_back(root_id);
        }
        if self.work.is_empty() {
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.message = format!(
                "Embedding work: {} generated · {} reused",
                snapshot.generated_vectors, snapshot.reused_vectors
            );
        }
        self.update_roots(self.work.is_empty());
    }
    fn report(&self, result: Result<(), String>) {
        self.snapshot.lock().unwrap().backend = self.model.as_ref().and_then(Model::backend_status);
        if let Err(error) = result {
            self.snapshot.lock().unwrap().error = Some(error);
        }
    }
    fn finish_root(&mut self) {
        let Some(id) = self.finishing.front().copied() else {
            return;
        };
        let Some(store) = &self.store else {
            self.finishing.clear();
            return;
        };
        let Some(cancel) = self.read_admission() else {
            return;
        };
        self.finishing.pop_front();
        let state = {
            let _guard = super::retrieval::SqlCancellation::new(&store.connection, &cancel);
            store.scan_state(id)
        };
        if cancel.load(Ordering::Relaxed) {
            self.finishing.push_back(id);
            return;
        }
        // The read handler must be gone before this small accepted write.
        let result = state.and_then(|state| store.finish_scan_state(id, &state, None));
        if result.is_ok() {
            self.monitor.succeeded(id);
        } else {
            self.monitor.failed(id);
        }
        self.report(result);
        self.update_roots(true);
    }
    fn read_admission(&self) -> Option<Arc<AtomicBool>> {
        let mut dispatch = self.dispatch.lock().unwrap();
        dispatch.begin_background().then(|| dispatch.cancel.clone())
    }
    fn prune_background(&mut self) {
        let Some(cursor) = &self.pruning else {
            return;
        };
        let Some(store) = &self.store else {
            self.pruning = None;
            self.pruning_dirty = false;
            return;
        };
        let Some(cancel) = self.read_admission() else {
            return;
        };
        let batch = {
            let _guard = super::retrieval::SqlCancellation::new(&store.connection, &cancel);
            store.prune_candidates(cursor)
        };
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // Candidate selection is cancellable; each admitted write is atomic
        // and bounded to 64 paths or 64 segments of one unreferenced file.
        let work = &self.work;
        // Preserve source-before-destination move reuse during ordinary scans.
        // Pause/pressure may reclaim unowned inputs, but queued IDs stay pinned.
        let awaiting_metadata = !self.config.paused
            && !self.capacity_blocked
            && work.iter().any(|work| {
                matches!(
                    work,
                    Work::Probe { .. }
                        | Work::Inventory(_)
                        | Work::Directory {
                            remaining: None,
                            ..
                        }
                )
            });
        match batch.and_then(|batch| {
            self.store.as_mut().unwrap().apply_prune_batch(batch, |id| {
                awaiting_metadata
                    || work
                        .iter()
                        .any(|work| matches!(work, Work::File(file) if file.id == id))
            })
        }) {
            Ok((cursor, removed)) => {
                if removed > 0 && self.capacity_blocked {
                    self.retry_capacity();
                }
                if cursor.done {
                    self.pruning = self.pruning_dirty.then(Default::default);
                    self.pruning_dirty = false;
                } else {
                    self.pruning = Some(cursor);
                }
            }
            Err(error) => {
                self.pruning = None;
                self.pruning_dirty = false;
                self.report(Err(error));
            }
        }
    }
    fn update_roots(&mut self, force: bool) {
        self.roots_dirty = true;
        if !force && self.last_snapshot.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_snapshot = Instant::now();
        let mut roots = match self.root_summary() {
            Ok(Some(roots)) => roots,
            Ok(None) => return,
            Err(error) => {
                self.report(Err(error));
                return;
            }
        };
        for root in &mut roots {
            root.mount = self
                .monitor
                .expected(&root.path)
                .and_then(|source| source.mount);
        }
        for work in &self.work {
            if let Work::Probe { id, path, .. } = work {
                if let Some(root) = roots.iter_mut().find(|root| root.id == *id) {
                    root.state = "checking".into();
                } else {
                    roots.push(RootState {
                        id: *id,
                        path: path.clone(),
                        state: "checking".into(),
                        error: None,
                        files: 0,
                        vectors: 0,
                        mount: None,
                    });
                }
            }
        }
        self.snapshot.lock().unwrap().roots = roots;
        self.roots_dirty = false;
    }
    fn root_summary(&self) -> Result<Option<Vec<RootState>>, String> {
        let Some(store) = &self.store else {
            return Ok(Some(Vec::new()));
        };
        let Some(cancel) = self.read_admission() else {
            return Ok(None);
        };
        // Only this read is interruptible. Remove its handler before any later
        // accepted mutation; keep the previous snapshot until a retry succeeds.
        let _guard = super::retrieval::SqlCancellation::new(&store.connection, &cancel);
        let result = store.roots();
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        result.map(Some)
    }
    fn database(&mut self) -> Result<&mut Store, String> {
        if self.store.is_none() {
            let mut store = Store::open(&self.paths.data)?;
            let configured = self
                .config
                .roots
                .iter()
                .filter_map(|address| little_clip_core::path_address::decode_path(address).ok())
                .filter(|path| super::policy::excluded(path).is_none())
                .collect::<Vec<_>>();
            store.disable_unconfigured(&configured)?;
            self.store = Some(store);
            self.pruning = Some(Default::default());
            self.pruning_dirty = false;
            self.snapshot.lock().unwrap().has_index = true;
        }
        Ok(self.store.as_mut().unwrap())
    }
    fn model(&mut self, role: Role, cancel: &AtomicBool) -> Result<&mut Model, String> {
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.role != role || !model.usable())
        {
            self.model = None;
            self.snapshot.lock().unwrap().backend = None;
        }
        if self.model.is_none() {
            self.paths.resolve_helpers()?;
            if !self.paths.configured(role) {
                return Err(match role {
                    Role::Embedding => "Embedding model is not installed. Open Index and choose Install embedding model.",
                    Role::Answer => "Answer model is not installed. Open Index and choose Install answer model.",
                }.into());
            }
            let model = Model::start_preferred(&self.paths, role, cancel)?;
            if role == Role::Embedding {
                let pipeline = format!("search-v2:{}", model.info["pipeline"]);
                let changed = !self.pipeline.is_empty() && self.pipeline != pipeline;
                self.pipeline = pipeline;
                if changed {
                    // The caller may itself own a partial file outside the queue.
                    // Retire queued work and abort this call before new vectors
                    // can be appended to an old-model prefix or inventory.
                    self.finish_maintenance(
                        "Embedding pipeline changed; enrolled folders will be rechecked.".into(),
                    );
                    return Err(
                        "Embedding pipeline changed; retry after folder revalidation".into(),
                    );
                }
            } else {
                self.snapshot.lock().unwrap().answer_pipeline =
                    Some(model.info["pipeline"].to_string());
            }
            self.snapshot.lock().unwrap().backend = model.backend_status();
            self.model = Some(model);
        }
        Ok(self.model.as_mut().unwrap())
    }
    fn command(&mut self, command: Command) -> Result<(), String> {
        self.snapshot.lock().unwrap().error = None;
        match command {
            Command::Backup(path) => self.backup(&path),
            Command::Restore(path) => self.restore_index(&path),
            Command::Clear => self.clear_index(),
            Command::Recover => self.recover_index(),
            Command::Preferences(config) => self.preferences(config),
            Command::Enroll(path) => self.enroll(&path, true),
            Command::Rebuild(path) => {
                self.retry_capacity();
                self.enroll(&path, false)?;
                if let Some(id) = self.enrollment_id(&path)?
                    && id > 0
                {
                    self.database()?.reset_retries(id)?;
                }
                Ok(())
            }
            Command::Remove(path) => {
                self.retry_capacity();
                let Some(id) = self.enrollment_id(&path)? else {
                    return Ok(());
                };
                self.cancel_reads(Some(id));
                self.work.retain(|work| work.root_id() != id);
                if id < 0 {
                    self.update_roots(true);
                    return Ok(());
                }
                self.database()?.remove_root(id)?;
                self.monitor.remove(id, Some(&path));
                {
                    let mut snapshot = self.snapshot.lock().unwrap();
                    snapshot.clear_results = Some(ResultReset::Scope);
                    snapshot.result = None;
                }
                self.persist_roots()?;
                self.update_roots(true);
                Ok(())
            }
            Command::Setup => self.setup(false),
            Command::SetupAnswer => self.setup(true),
            Command::Answer(job) => {
                let answer = self
                    .answer_file(&job)
                    .unwrap_or_else(|error| Answer::status(State::Failed, error));
                if matches!(answer.state, State::Failed) {
                    self.model = None;
                    self.snapshot.lock().unwrap().backend = None;
                }
                let mut snapshot = self.snapshot.lock().unwrap();
                while snapshot.answers.len() >= 8 {
                    snapshot.answers.pop_front();
                }
                snapshot
                    .answers
                    .push_back(AnswerResult { id: job.id, answer });
                self.answers_until = Some(Instant::now() + Duration::from_secs(2));
                Ok(())
            }
            Command::Search {
                id,
                path,
                query,
                hidden,
                cancel,
            } => {
                let result = self.search(id, &path, &query, hidden, &cancel);
                if result.is_err() {
                    self.model = None;
                    self.snapshot.lock().unwrap().backend = None;
                }
                publish_search_result(
                    &self.snapshot,
                    SearchResult {
                        id,
                        complete: true,
                        result,
                    },
                );
                Ok(())
            }
        }
    }
    fn backup(&mut self, path: &Path) -> Result<(), String> {
        self.snapshot.lock().unwrap().message = "Backing up the index…".into();
        let cancel = self.stop.clone();
        self.database()?.export(path, &cancel)?;
        self.snapshot.lock().unwrap().message = format!(
            "Index backup saved: {}",
            little_clip_core::path_address::display_path(path)
        );
        Ok(())
    }
    fn restore_index(&mut self, path: &Path) -> Result<(), String> {
        self.snapshot.lock().unwrap().message = "Validating index backup…".into();
        let cancel = self.stop.clone();
        let limits = self.config.limits.clone();
        let prepared =
            self.database()?
                .prepare_restore(path, &limits, &cancel, reader::prepare_restore)?;
        let mut config = self.config.clone();
        config.paused = true;
        little_clip_core::config::save_file_search_preferences(&config)
            .map_err(|e| e.to_string())?;
        self.config = config;
        self.snapshot.lock().unwrap().settings = self.config.clone();
        let configured = self
            .config
            .roots
            .iter()
            .map(|address| little_clip_core::path_address::decode_path(address))
            .collect::<Result<Vec<_>, _>>()?;
        self.database()?
            .apply_restore(prepared, &configured, &cancel)?;
        self.finish_maintenance("Index restored; indexing paused. Configured folders were kept; restored files require fresh verification.".into());
        Ok(())
    }

    fn recover_index(&mut self) -> Result<(), String> {
        let mut config = self.config.clone();
        config.paused = true;
        little_clip_core::config::save_file_search_preferences(&config)
            .map_err(|e| e.to_string())?;
        self.config = config;
        self.cancel_reads(None);
        self.work.clear();
        self.monitor.reset_watches();
        self.model = None;
        {
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.settings = self.config.clone();
            snapshot.clear_results = Some(ResultReset::Semantic);
            snapshot.result = None;
            snapshot.backend = None;
        }
        let (store, archive) = Store::recover_empty(&self.paths.data, self.store.take())?;
        self.store = Some(store);
        self.finish_maintenance(format!("Empty index created; indexing paused. Previous cache preserved at {}. You can now restore a backup or resume indexing.", little_clip_core::path_address::display_path(&archive)));
        Ok(())
    }

    fn clear_index(&mut self) -> Result<(), String> {
        let mut config = self.config.clone();
        config.paused = true;
        little_clip_core::config::save_file_search_preferences(&config)
            .map_err(|error| error.to_string())?;
        self.config = config;
        self.snapshot.lock().unwrap().settings = self.config.clone();
        let warning = self.database()?.clear_generated()?;
        self.finish_maintenance(warning.unwrap_or_else(|| {
            "Index cleared; indexing paused. Enrolled folders and model files were kept.".into()
        }));
        Ok(())
    }

    fn finish_maintenance(&mut self, message: String) {
        self.cancel_reads(None);
        self.work.clear();
        self.monitor.reset_watches();
        self.model = None;
        self.restore_enrollment();
        self.retry_capacity();
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.clear_results = Some(ResultReset::Semantic);
        snapshot.result = None;
        snapshot.backend = None;
        snapshot.message = message;
    }

    fn preferences(&mut self, mut config: FileSearchConfig) -> Result<(), String> {
        super::policy::validate_preferences(&config)?;
        config.roots = self.config.roots.clone();
        little_clip_core::config::save_file_search_preferences(&config)
            .map_err(|error| error.to_string())?;
        let recheck = config.include_hidden != self.config.include_hidden
            || config.excluded_paths != self.config.excluded_paths;
        let scope_changed = recheck
            || config.always_search != self.config.always_search
            || config.show_offline_results != self.config.show_offline_results;
        self.exclusions = super::policy::exclusions(&config.excluded_paths)?;
        self.config = config;
        self.retry_capacity();
        {
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.settings = self.config.clone();
            snapshot.message = "Search preferences saved".into();
            if scope_changed {
                snapshot.clear_results = Some(ResultReset::Scope);
                snapshot.result = None;
            }
        }
        if self.store.is_some() {
            let result = self.admit(None, 0, 0).map(|_| ());
            self.report(result);
        }
        if recheck {
            self.cancel_reads(None);
            self.work.clear();
            self.monitor.reset_watches();
            self.retire_exclusions()?;
            for address in self.config.roots.clone() {
                let path = little_clip_core::path_address::decode_path(&address)?;
                let result = self.enroll(&path, false);
                self.report(result);
            }
        }
        Ok(())
    }

    fn retire_exclusions(&mut self) -> Result<(), String> {
        let Some(store) = &mut self.store else {
            return Ok(());
        };
        for root in store.roots()? {
            for excluded in &self.exclusions {
                if root.path.starts_with(excluded) {
                    store.retire_path(root.id, &root.path)?;
                } else if excluded.starts_with(&root.path) {
                    store.retire_path(root.id, excluded)?;
                }
            }
        }
        store.prune_unreferenced()
    }

    fn setup(&mut self, answer: bool) -> Result<(), String> {
        self.model = None;
        self.snapshot.lock().unwrap().backend = None;
        self.paths.resolve_helpers()?;
        self.snapshot.lock().unwrap().message =
            "Installing pinned model and inference runtime…".into();
        let mut request = little_clip_core::process::CommandRequest::new("python3")
            .arg("-I")
            .arg(self.paths.helpers.join("setup.py"))
            .arg("--parent")
            .arg(std::process::id().to_string())
            .arg("--runtime")
            .arg(self.paths.runtime())
            .arg("--models")
            .arg(self.paths.models());
        if answer {
            request = request
                .arg("--answers")
                .arg(super::runtime::ANSWER_PROVIDER);
        }
        let output = request
            .own_process_group()
            .timeout(Duration::from_secs(3600))
            .max_output_bytes(64 * 1024)
            .run_cancellable(&self.stop)
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!(
                "Model installation failed: {}",
                output.stderr_lossy()
            ));
        }
        self.snapshot.lock().unwrap().message =
            "Model installed. Rebuild an enrolled folder to continue indexing.".into();
        Ok(())
    }
    fn answer_file(&mut self, job: &AnswerJob) -> Result<Answer, String> {
        if super::policy::excluded_by_user(&job.entry.path, &self.exclusions) {
            return Ok(Answer::status(
                State::Excluded,
                "Excluded by search preferences.",
            ));
        }
        if !answer::supports(&job.entry) {
            return Ok(Answer::status(
                State::Unsupported,
                "This answer path supports readable documents and still images.",
            ));
        }
        let started = Instant::now();
        let structured = self.answer_structured(job);
        let (answer, used_plan) = match structured {
            Ok(answer::extraction::Attempt::Answer(answer)) => (answer, true),
            fallback => {
                let feedback = match &fallback {
                    Ok(answer::extraction::Attempt::Escalate(reason)) => Some(reason.as_str()),
                    _ => None,
                };
                let (mut answer, used_plan) = self.answer_document(job, feedback)?;
                match fallback {
                    Ok(answer::extraction::Attempt::Escalate(reason)) => {
                        answer.reason.push_str(&format!(
                            " Metadata candidate review requested full-document answering: {reason}"
                        ));
                    }
                    Err(_) if used_plan => answer.reason.push_str(
                        " Front-matter extraction was unavailable; used a reviewed document field.",
                    ),
                    Err(_) => answer.reason.push_str(
                        " Structured extraction/review was unavailable; used document answering.",
                    ),
                    _ => {}
                }
                (answer, used_plan)
            }
        };
        match reader::call(
            &Request::Verify {
                root: job.root.clone(),
                entry: job.entry.clone(),
            },
            &job.cancel,
        ) {
            Ok(Reply::Verified) => {
                tracing::debug!(target: "little_clip_search_timing",
                    elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
                    structured = used_plan, "column answer ready");
                Ok(answer)
            }
            _ => Ok(Answer::status(
                State::Unavailable,
                "Source changed or could not be verified after answering.",
            )),
        }
    }
    fn answer_structured(
        &mut self,
        job: &AnswerJob,
    ) -> Result<answer::extraction::Attempt, String> {
        let Reply::Header(source) = reader::call(
            &Request::Header {
                root: job.root.clone(),
                entry: job.entry.clone(),
            },
            &job.cancel,
        )?
        else {
            return Err("Invalid header source reply".into());
        };
        self.answer_extracted(answer::extraction::Input::Header(&source), job)
    }
    fn answer_extracted(
        &mut self,
        input: answer::extraction::Input<'_>,
        job: &AnswerJob,
    ) -> Result<answer::extraction::Attempt, String> {
        answer::extraction::try_answer(
            input,
            &job.question,
            job.kind,
            &job.plans,
            &job.cancel,
            |request| {
                self.model(Role::Answer, &job.cancel)?.answer(
                    request,
                    Duration::from_secs(30),
                    &job.cancel,
                )
            },
        )
    }
    fn answer_document(
        &mut self,
        job: &AnswerJob,
        feedback: Option<&str>,
    ) -> Result<(Answer, bool), String> {
        let document = match answer::read_document(&job.root, &job.entry, &job.cancel) {
            Ok(document) => document,
            Err(error) => return Ok((Answer::status(State::Unavailable, error), false)),
        };
        let mut document_feedback = None;
        if feedback.is_none()
            && let Some(source) = document.complete_single_text()
        {
            match self.answer_extracted(answer::extraction::Input::Document(source), job) {
                Ok(answer::extraction::Attempt::Answer(answer)) => {
                    tracing::debug!(target: "little_clip_search_timing", "column labelled field accepted");
                    return Ok((answer, true));
                }
                Ok(answer::extraction::Attempt::Escalate(reason)) => {
                    tracing::debug!(target: "little_clip_search_timing", "column labelled field escalated");
                    document_feedback = Some(reason)
                }
                _ => {}
            }
        }
        if document.needs_vision()
            && !self
                .paths
                .models()
                .join("mmproj-Qwen_Qwen3.5-9B-f16.gguf")
                .is_file()
        {
            return Ok((
                Answer::status(
                    State::Unsupported,
                    "Vision projection is not installed. Open Index and choose Install answer model.",
                ),
                false,
            ));
        }
        let mut result = answer::answer_prepared(
            document,
            &job.question,
            job.kind,
            &job.cancel,
            |mut request, timeout| {
                if let Some(feedback) = document_feedback.as_deref().or(feedback) {
                    request["review_feedback"] = serde_json::json!(feedback);
                }
                self.model(Role::Answer, &job.cancel)?
                    .answer(request, timeout, &job.cancel)
            },
        )?;
        if let Some(reason) = document_feedback {
            result.reason.push_str(&format!(
                " Labelled-field review requested full-document answering: {reason}"
            ));
        }
        Ok((result, false))
    }
}

mod indexing;
#[cfg(test)]
use indexing::*;

#[cfg(test)]
mod tests;
