//! One background owner for index transactions, inference, and reconciliation work.
mod readers;
use super::{
    answer::{self, Answer, State},
    query::{Operator, Query, Scope},
    reader::{self, Prepared, Reply, Request, SourceRoot},
    runtime::{Model, Paths, Role},
    store::{Checkpoint, RootState, Segment, Store},
};
use crate::file_manager::filesystem::{self, Entry};
use base64::{Engine, engine::general_purpose::STANDARD};
use little_clip_core::config::FileSearchConfig;
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

pub(super) enum Command {
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
pub(super) struct AnswerJob {
    pub id: u64,
    pub root: SourceRoot,
    pub entry: Entry,
    pub question: String,
    pub kind: super::query::AnswerType,
    pub cancel: Arc<AtomicBool>,
    pub plans: answer::extraction::SharedPlans,
}
pub(super) struct AnswerResult {
    pub id: u64,
    pub answer: Answer,
}
pub(super) struct SearchResult {
    pub id: u64,
    pub complete: bool,
    pub result: Result<super::Results, String>,
}
pub(super) struct SearchProgress {
    pub id: u64,
    pub checked: Arc<AtomicU64>,
}
pub(super) enum ResultReset {
    Semantic,
    Scope,
}
#[derive(Default)]
pub(super) struct Snapshot {
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
    repaint: Option<egui::Context>,
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
    let repaint = {
        let mut snapshot = snapshot.lock().unwrap();
        if result.complete {
            snapshot.progress = None;
        }
        snapshot.result = Some(result);
        snapshot.repaint.clone()
    };
    // UI callbacks can inspect the snapshot; never wake while holding its lock.
    if let Some(ctx) = repaint {
        ctx.request_repaint_of(egui::ViewportId::ROOT);
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

pub(in crate::file_manager) struct Service {
    commands: mpsc::SyncSender<Command>,
    dispatch: Arc<Mutex<Dispatch>>,
    pub(super) snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Service {
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
    pub(in crate::file_manager) fn maintenance_busy(&self) -> bool {
        self.snapshot.lock().unwrap().maintenance
    }
    pub(in crate::file_manager) fn allow_close(&mut self) -> bool {
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
    pub(super) fn send(&self, command: Command) -> Result<(), String> {
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

    pub(super) fn set_repaint_context(&self, ctx: &egui::Context) {
        let available = {
            let mut snapshot = self.snapshot.lock().unwrap();
            snapshot.repaint = Some(ctx.clone());
            snapshot.result.is_some()
        };
        // Cover a completion that raced ahead of the first UI registration.
        if available {
            ctx.request_repaint();
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
    failed: bool,
    staging: Option<Arc<()>>,
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
    answers_until: Option<Instant>,
    monitor: super::monitor::Monitor,
    capacity_blocked: bool,
    readers: readers::Readers,
    staged_files: Vec<Weak<()>>,
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
        self.prune_idle();
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
                            filesystem::decode_path(address).ok().as_ref() == Some(&root.path)
                        })
                    {
                        self.monitor.register(id, root);
                    }
                }
            }
        }
        for address in self.config.roots.clone() {
            if let Ok(path) = filesystem::decode_path(&address) {
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
        self.pruning = None;
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
        self.ready_work().is_some()
            || !self.finishing.is_empty()
            || (self.pruning.is_some() && self.work.is_empty())
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
        self.pruning = None;
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
            self.pruning = Some(Default::default());
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
    fn prune_idle(&mut self) {
        if !self.work.is_empty() || !self.finishing.is_empty() {
            return;
        }
        let Some(cursor) = &self.pruning else {
            return;
        };
        let Some(store) = &self.store else {
            self.pruning = None;
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
        match batch.and_then(|batch| self.store.as_mut().unwrap().apply_prune_batch(batch)) {
            Ok(cursor) => self.pruning = (!cursor.done).then_some(cursor),
            Err(error) => {
                self.pruning = None;
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
                .filter_map(|address| filesystem::decode_path(address).ok())
                .filter(|path| super::policy::excluded(path).is_none())
                .collect::<Vec<_>>();
            store.disable_unconfigured(&configured)?;
            self.store = Some(store);
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
                self.pipeline = format!("search-v2:{}", model.info["pipeline"]);
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
        self.snapshot.lock().unwrap().message =
            format!("Index backup saved: {}", filesystem::display_path(path));
        Ok(())
    }
    fn restore_index(&mut self, path: &Path) -> Result<(), String> {
        self.snapshot.lock().unwrap().message = "Validating index backup…".into();
        let cancel = self.stop.clone();
        let limits = self.config.limits.clone();
        let prepared = self.database()?.prepare_restore(path, &limits, &cancel)?;
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
            .map(|address| filesystem::decode_path(address))
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
        self.finish_maintenance(format!("Empty index created; indexing paused. Previous cache preserved at {}. You can now restore a backup or resume indexing.", filesystem::display_path(&archive)));
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
        super::settings::validate(&config)?;
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
                let path = filesystem::decode_path(&address)?;
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
    fn persist_roots(&mut self) -> Result<(), String> {
        let roots = self
            .database()?
            .roots()?
            .iter()
            .map(|root| filesystem::display_path(&root.path))
            .collect::<Vec<_>>();
        little_clip_core::config::save_file_search_roots(&roots).map_err(|e| e.to_string())?;
        self.config.roots = roots;
        self.snapshot.lock().unwrap().settings = self.config.clone();
        Ok(())
    }
    fn enrollment_id(&self, path: &Path) -> Result<Option<i64>, String> {
        let stored = self
            .store
            .as_ref()
            .map(|store| store.root_id(path))
            .transpose()?
            .flatten();
        Ok(stored.or_else(|| {
            self.work.iter().find_map(|work| match work {
                Work::Probe {
                    id, path: pending, ..
                } if pending == path => Some(*id),
                _ => None,
            })
        }))
    }
    fn enroll(&mut self, path: &Path, persist: bool) -> Result<(), String> {
        if let Some(reason) = super::policy::excluded(path) {
            return Err(reason.into());
        }
        if super::policy::excluded_by_user(path, &self.exclusions) {
            return Err("Folder is excluded by search preferences".into());
        }
        if let Some(Work::Probe {
            persist: pending, ..
        }) = self
            .work
            .iter_mut()
            .find(|work| matches!(work, Work::Probe { path: pending, .. } if pending == path))
        {
            *pending |= persist;
            return Ok(());
        }
        if self.work.len() >= 100_000 {
            return Err("Index work queue is full; reconciliation will retry".into());
        }
        let stored = self.enrollment_id(path)?;
        let id = match stored {
            Some(id) if id > 0 => {
                // A pending source recheck must not authorize cached results.
                self.store
                    .as_ref()
                    .unwrap()
                    .invalidate_directory(id, None)?;
                id
            }
            Some(_) => return Err("Invalid stored root identifier".into()),
            None => {
                self.next_probe = self
                    .next_probe
                    .checked_sub(1)
                    .ok_or("Enrollment request IDs exhausted")?;
                self.next_probe
            }
        };
        self.cancel_reads(Some(id));
        self.work.retain(|work| work.root_id() != id);
        self.work.push_back(Work::Probe {
            id,
            path: path.into(),
            persist,
        });
        self.update_roots(true);
        Ok(())
    }
    fn probe(&mut self, id: i64, path: PathBuf, persist: bool) -> Result<(), String> {
        let result = self
            .readers
            .request(id, 0, &path, Request::Probe(path.clone()));
        let result = match result {
            Ok(None) => {
                self.work.push_back(Work::Probe { id, path, persist });
                return Ok(());
            }
            Ok(Some(Reply::Root(root))) => self.start_scan(root, persist),
            Ok(Some(_)) => Err("Invalid enrollment reply".into()),
            Err(error) => Err(error),
        };
        if let Err(error) = &result
            && let Some(store) = &self.store
        {
            match store.unavailable(&path, error) {
                Ok(Some(id)) => {
                    self.cancel_reads(Some(id));
                    self.work.retain(|work| work.root_id() != id);
                    self.monitor.failed(id);
                }
                Ok(None) => {}
                Err(error) => self.report(Err(error)),
            }
        }
        result
    }
    fn start_scan(&mut self, root: SourceRoot, persist: bool) -> Result<(), String> {
        self.monitor.check_source(&root)?;
        if root.path.starts_with(&self.paths.data) {
            return Err("The search cache cannot index itself".into());
        }
        if super::policy::excluded_by_user(&root.path, &self.exclusions) {
            return Err("Folder is excluded by search preferences".into());
        }
        let id = if persist {
            self.database()?.enroll_new_source(&root)?
        } else {
            self.database()?.enroll_source(&root)?
        };
        if persist {
            self.persist_roots()?;
        }
        self.cancel_reads(Some(id));
        self.work.retain(|work| work.root_id() != id);
        let generation = self.database()?.begin_scan(id)?;
        self.monitor.started(id, root.clone());
        self.work.push_back(Work::Directory {
            path: root.path.clone(),
            root,
            id,
            generation,
            recursive: true,
            remaining: None,
        });
        self.update_roots(true);
        Ok(())
    }
    fn step(&mut self, work: Work) -> Result<(), String> {
        match work {
            Work::Probe { id, path, persist } => self.probe(id, path, persist),
            Work::Directory {
                root,
                id,
                generation,
                path,
                recursive,
                remaining,
            } => {
                // Both inventory and retirement obey the current policy.
                if self.index_excludes(&path, &root.path) {
                    self.database()?.retire_path(id, &path)
                } else if let Some(remaining) = remaining {
                    self.finish_directory_inventory(root, id, generation, path, remaining)
                } else {
                    self.scan(root, id, generation, path, recursive)
                }
            }
            Work::Inventory(inventory) => self.apply_inventory(*inventory),
            Work::File(file) => self.index_file(*file),
        }
    }
    fn index_excludes(&self, path: &Path, root: &Path) -> bool {
        path.starts_with(&self.paths.data)
            || super::policy::excluded_by_user(path, &self.exclusions)
            || super::policy::excluded_from_root(path, root, self.config.include_hidden)
    }

    fn scan(
        &mut self,
        root: SourceRoot,
        id: i64,
        generation: i64,
        path: PathBuf,
        recursive: bool,
    ) -> Result<(), String> {
        let Some(inventory) = self.read_inventory(&root, id, generation, &path)? else {
            self.work.push_back(Work::Directory {
                root,
                id,
                generation,
                path,
                recursive,
                remaining: None,
            });
            return Ok(());
        };
        // Identify the pipeline before admitting current segment reuse.
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        if let Err(error) = self.model(Role::Embedding, &stop) {
            if self.background_interrupted(&error) {
                self.work.push_front(Work::Directory {
                    root,
                    id,
                    generation,
                    path,
                    recursive,
                    remaining: None,
                });
                return Ok(());
            }
            return Err(error);
        }
        self.work
            .push_front(Work::Inventory(Box::new(InventoryWork {
                invalidated: false,
                remaining_bytes: inventory.storage_bytes,
                _permit: inventory.permit,
                entries: inventory.directory.entries.into(),
                pipeline: self.pipeline.clone(),
                root,
                id,
                generation,
                path,
                recursive,
            })));
        Ok(())
    }
    fn apply_inventory(&mut self, mut inventory: InventoryWork) -> Result<(), String> {
        let InventoryWork { id, generation, .. } = inventory;
        if self.index_excludes(&inventory.path, &inventory.root.path) {
            return self.database()?.retire_path(id, &inventory.path);
        }
        if inventory.invalidated {
            // A partial application must not restore membership from an old
            // snapshot after an event. A new generation also revisits removals
            // among entries already observed in this abandoned application.
            let generation = self.database()?.begin_delta(id)?;
            // Pending files from this abandoned directory will be reconciled by
            // the fresh inventory. Their committed checkpoints remain reusable.
            self.work.retain(|work| {
                !matches!(work, Work::File(file)
                if file.root_id == id && file.entry.path.parent() == Some(inventory.path.as_path()))
            });
            self.work.push_front(Work::Directory {
                root: inventory.root,
                id,
                generation,
                path: inventory.path,
                recursive: inventory.recursive,
                remaining: None,
            });
            return Ok(());
        }
        self.observe_inventory(&mut inventory)?;
        if !inventory.entries.is_empty() {
            self.work.push_back(Work::Inventory(Box::new(inventory)));
            return Ok(());
        }
        let remaining = self
            .database()?
            .unseen_children(id, generation, &inventory.path)?;
        self.finish_directory_inventory(
            inventory.root,
            id,
            generation,
            inventory.path,
            remaining.into(),
        )
    }
    fn observe_inventory(&mut self, inventory: &mut InventoryWork) -> Result<(), String> {
        let started = Instant::now();
        let before = inventory.entries.len();
        let cancel = self.dispatch.lock().unwrap().cancel.clone();
        for _ in 0..64 {
            if cancel.load(Ordering::Relaxed) || started.elapsed() >= Duration::from_millis(8) {
                break;
            }
            let Some(entry) = inventory.entries.pop_front() else {
                break;
            };
            inventory.remaining_bytes = inventory
                .remaining_bytes
                .saturating_sub(reader::Inventory::entry_storage_bytes(&entry)?);
            if self.index_excludes(&entry.path, &inventory.root.path) {
                continue;
            }
            if entry.directory
                && !inventory.recursive
                && self
                    .database()?
                    .known_directory(inventory.id, &entry.path)?
            {
                continue;
            }
            self.observe_entry(
                &inventory.root,
                inventory.id,
                inventory.generation,
                entry,
                &inventory.pipeline,
            )?;
        }
        tracing::debug!(target: "little_clip_search_timing",
            rows = before - inventory.entries.len(), remaining = inventory.entries.len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
            "directory inventory batch");
        Ok(())
    }
    fn read_inventory(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        path: &Path,
    ) -> Result<Option<reader::Inventory>, String> {
        self.snapshot.lock().unwrap().message =
            format!("Scanning {}", filesystem::display_path(path));
        match self.watch_directory(root, id, generation, path) {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => {
                self.cancel_reads(Some(id));
                self.monitor.failed(id);
                self.snapshot.lock().unwrap().error = Some(format!(
                    "Change monitoring incomplete: {error}; periodic reconciliation remains enabled"
                ));
            }
        }
        let Some(reply) = self.readers.request(
            id,
            generation,
            path,
            Request::List {
                output: None,
                root: root.clone(),
                path: path.into(),
                hidden: self.config.include_hidden,
                exclusions: self.exclusions.clone(),
            },
        )?
        else {
            return Ok(None);
        };
        let Reply::Directory(inventory) = reply else {
            return Err("Invalid directory reply".into());
        };
        if inventory.directory.skipped > 0 {
            return Err("Directory enumeration incomplete; existing inventory was retained".into());
        }
        // The worker reserves the complete inventory before any row is written.
        if !self.admit(None, 0, inventory.storage_bytes)? {
            return Ok(None);
        }
        Ok(Some(inventory))
    }
    fn watch_directory(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        path: &Path,
    ) -> Result<bool, String> {
        let instance = match self.readers.watch_handle(id) {
            Some(instance) => instance,
            None if self.readers.can_start(id) => {
                self.monitor.prepare_watch(self.readers.watch_count())?
            }
            None => return Ok(true), // An inventory request is already pending.
        };
        let reply = match self
            .readers
            .watch(id, generation, path, root.clone(), instance.clone())
        {
            Ok(None) => return Ok(false),
            Ok(Some(registration)) => Ok(registration),
            Err(error) => Err(error),
        };
        self.monitor.complete_watch(id, path, &instance, reply)?;
        Ok(true)
    }
    fn finish_directory_inventory(
        &mut self,
        root: SourceRoot,
        id: i64,
        generation: i64,
        path: PathBuf,
        mut remaining: VecDeque<PathBuf>,
    ) -> Result<(), String> {
        if let Some(unseen) = remaining.pop_front() {
            let excluded = self.index_excludes(&unseen, &root.path);
            // A policy change retires generated references even when the source
            // still exists. Ordinary removals continue to require absence proof.
            let absent = if excluded {
                true
            } else {
                match self.readers.request(
                    id,
                    generation,
                    &path,
                    Request::Absent {
                        root: root.clone(),
                        path: unseen.clone(),
                    },
                ) {
                    Ok(Some(Reply::Verified)) => true,
                    Ok(None) => {
                        remaining.push_front(unseen.clone());
                        false
                    }
                    _ => false,
                }
            };
            if absent {
                self.database()?.retire_path(id, &unseen)?;
            }
        }
        if remaining.is_empty() {
            self.database()?.directory_complete(id, generation, &path)
        } else {
            self.work.push_front(Work::Directory {
                root,
                id,
                generation,
                path,
                recursive: false,
                remaining: Some(remaining),
            });
            Ok(())
        }
    }
    fn observe_entry(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        entry: Entry,
        pipeline: &str,
    ) -> Result<(), String> {
        // Reserve one slot for this directory's retirement continuation.
        if self.work.len() >= 99_999 {
            return Err(
                "Index work queue reached its 100,000-entry bound; coverage is incomplete".into(),
            );
        }
        if entry.directory {
            self.work.push_back(Work::Directory {
                root: root.clone(),
                id,
                generation,
                path: entry.path,
                recursive: true,
                remaining: None,
            });
        } else if let Some(object) = entry.identity.map(|id| id.object()) {
            let (file_id, pending) = self
                .database()?
                .observe_object(id, generation, &entry, object, pipeline)?;
            if pending {
                let file = FileWork::new(root.clone(), id, generation, entry, file_id);
                if self.retry_ready(&file)? {
                    self.work.push_back(Work::File(Box::new(file)));
                }
            }
        }
        Ok(())
    }
    fn index_file(&mut self, mut file: FileWork) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("Search cancelled".into());
        }
        if self.index_excludes(&file.entry.path, &file.root.path) {
            return self.database()?.retire_path(file.root_id, &file.entry.path);
        }
        if !file.publishing && file.segments.is_empty() && !self.retry_ready(&file)? {
            return Ok(());
        }
        if file.publishing {
            return self.publish_file(file);
        }
        let resume = matches!(file.source_check, SourceCheck::Pending);
        let ready = match self.check_file_content(&mut file) {
            Ok(ready) => ready,
            Err(error) => return self.file_failure(file, error),
        };
        if resume && ready {
            self.resume_file(&mut file)?;
        }
        if !ready || !self.checkpoint_file(&mut file)? {
            self.work.push_back(Work::File(Box::new(file)));
            return Ok(());
        }
        if matches!(file.source_check, SourceCheck::Reuse(_)) {
            if !self.admit(Some(&file), 0, 0)? {
                self.work.push_front(Work::File(Box::new(file)));
                return Ok(());
            }
            return self.publish_file(file);
        }
        self.extract_file(file)
    }
    fn reserve_file_staging(&mut self, file: &mut FileWork) -> bool {
        if file.staging.is_some() {
            return true;
        }
        self.staged_files.retain(|slot| slot.strong_count() != 0);
        if self.staged_files.len() == MAX_STAGED_FILES {
            return false;
        }
        let permit = Arc::new(());
        self.staged_files.push(Arc::downgrade(&permit));
        file.staging = Some(permit);
        true
    }
    fn extract_file(&mut self, mut file: FileWork) -> Result<(), String> {
        if file.segments.len() >= MAX_FILE_SEGMENTS {
            file.coverage = format!(
                "partial: segment limit {MAX_FILE_SEGMENTS}; {}",
                file.coverage
            );
            return self.publish_file(file);
        }
        self.snapshot.lock().unwrap().message = format!("Indexing {}", file.entry.name);
        if file.prepared.is_none() {
            let result = self.read_prepared(&mut file);
            let prepared = match result {
                Ok(Some(prepared)) => prepared,
                Ok(None) => {
                    self.work.push_back(Work::File(Box::new(file)));
                    return Ok(());
                }
                Err(error) => return self.file_failure(file, error),
            };
            file.coverage = prepared.coverage.clone();
            file.prepared = Some(prepared);
        }
        let prepared = file.prepared.as_ref().unwrap();
        if file.text_cursor < prepared.text.len() {
            if !self.admit(Some(&file), 1, 0)? {
                self.work.push_front(Work::File(Box::new(file)));
                return Ok(());
            }
            let segment = match self.text_segment(&mut file) {
                Ok(segment) => segment,
                Err(error) => {
                    self.model = None;
                    self.snapshot.lock().unwrap().backend = None;
                    return self.file_failure(file, error);
                }
            };
            file.segments.push(segment);
            self.work.push_front(Work::File(Box::new(file)));
            return Ok(());
        }
        if prepared.data.is_some() && !self.admit(Some(&file), 1, 0)? {
            self.work.push_front(Work::File(Box::new(file)));
            return Ok(());
        }
        let prepared = file.prepared.take().unwrap();
        self.finish_file(file, prepared)
    }
    fn check_file_content(&mut self, file: &mut FileWork) -> Result<bool, String> {
        if !matches!(file.source_check, SourceCheck::Pending) {
            return Ok(true);
        }
        let digest = match self.read_file(
            file,
            Request::Fingerprint {
                root: file.root.clone(),
                entry: file.entry.clone(),
            },
        )? {
            None => return Ok(false),
            Some(Reply::Fingerprint(digest)) => digest,
            _ => return Err("Invalid source fingerprint reply".into()),
        };
        file.source_check = SourceCheck::Extract(digest);
        if let Some(digest) = digest {
            let pipeline = self.pipeline.clone();
            if let Some((segments, coverage)) =
                self.database()?
                    .reusable_source(&file.entry, &pipeline, &digest)?
            {
                file.segments = segments;
                file.coverage = coverage;
                file.source_check = SourceCheck::Reuse(digest);
            }
        }
        Ok(true)
    }
    fn text_segment(&mut self, file: &mut FileWork) -> Result<Segment, String> {
        let prepared = file.prepared.as_ref().unwrap();
        let start = file.text_cursor;
        let mut limit = (start + 8192).min(prepared.text.len());
        while !prepared.text.is_char_boundary(limit) {
            limit -= 1;
        }
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        let (end, next) = self
            .model(Role::Embedding, &stop)?
            .passage(&prepared.text[start..limit], &stop)?;
        let text = prepared.text[start..start + end].to_owned();
        let next_cursor = if start + end == prepared.text.len() {
            start + end
        } else {
            start + next
        };
        let location = format!(
            "{} · UTF-8 bytes {}..{}",
            prepared.location,
            start,
            start + end
        );
        let input = format!("title: none | text: {text}");
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        let input_hash = Sha256::digest(input.as_bytes()).to_vec();
        let vector = match self.cached_vector("text", &input_hash)? {
            Some(vector) => vector,
            None => {
                let vector = self
                    .model(Role::Embedding, &stop)?
                    .texts(std::slice::from_ref(&input), false, &stop)?
                    .remove(0);
                self.snapshot.lock().unwrap().generated_vectors += 1;
                vector
            }
        };
        // Commit the cursor only together with a complete vector. Preemption
        // must retry this passage rather than silently dropping its text.
        file.text_cursor = next_cursor;
        Ok(Segment {
            modality: "text".into(),
            location,
            text,
            input_hash,
            vector: Some(vector),
        })
    }
    fn cached_vector(&mut self, modality: &str, hash: &[u8]) -> Result<Option<Vec<f32>>, String> {
        let pipeline = self.pipeline.clone();
        let vector = self
            .database()?
            .reusable_vector(&pipeline, modality, hash)?;
        if vector.is_some() {
            self.snapshot.lock().unwrap().reused_vectors += 1;
        }
        Ok(vector)
    }

    fn media_segment(&mut self, prepared: &Prepared) -> Result<Segment, String> {
        let bytes = STANDARD
            .decode(prepared.data.as_ref().unwrap())
            .map_err(|e| e.to_string())?;
        let budget = prepared.vision_tokens.unwrap_or(280);
        if !matches!(budget, 140 | 280) {
            return Err("Unsupported vision token budget".into());
        }
        let input_hash = media_hash(&bytes, &prepared.modality, budget);
        let stop = self.dispatch.lock().unwrap().cancel.clone();
        let vector = match self.cached_vector(&prepared.modality, &input_hash)? {
            Some(vector) => vector,
            None => {
                let vector = self.model(Role::Embedding, &stop)?.media(
                    &bytes,
                    prepared.modality == "image",
                    budget,
                    &stop,
                )?;
                self.snapshot.lock().unwrap().generated_vectors += 1;
                vector
            }
        };
        Ok(Segment {
            modality: prepared.modality.clone(),
            location: prepared.location.clone(),
            text: String::new(),
            input_hash,
            vector: Some(vector),
        })
    }
    fn finish_file(&mut self, mut file: FileWork, prepared: Prepared) -> Result<(), String> {
        if prepared.data.is_some() {
            match self.media_segment(&prepared) {
                Ok(segment) => file.segments.push(segment),
                Err(error) => {
                    self.model = None;
                    self.snapshot.lock().unwrap().backend = None;
                    if self.background_interrupted(&error) {
                        file.prepared = Some(prepared);
                    }
                    return self.file_failure(file, error);
                }
            }
        }
        if let Some(next) = prepared.next {
            file.cursor = next;
            file.text_cursor = 0;
            self.work.push_front(Work::File(Box::new(file)));
            Ok(())
        } else {
            self.publish_file(file)
        }
    }
    fn retry_ready(&mut self, file: &FileWork) -> Result<bool, String> {
        match self.database()?.file_retry(file.id)? {
            super::store::Retry::Ready => Ok(true),
            super::store::Retry::After(delay) => {
                if let Some(parent) = file.entry.path.parent() {
                    self.monitor.retry_directory(file.root_id, parent, delay);
                }
                Ok(false)
            }
            super::store::Retry::Stopped => Ok(false),
        }
    }
    fn file_failure(&mut self, mut file: FileWork, error: String) -> Result<(), String> {
        // A stopped reader/model cannot validate or publish a partial result.
        // Keep its last committed state, while retaining genuine failures for
        // the root error handler instead of converting them to cancellation.
        if self.stop.load(Ordering::Relaxed) {
            return Err(error);
        }
        if self.background_interrupted(&error) {
            self.work.push_front(Work::File(Box::new(file)));
            return Ok(());
        }
        let pipeline = self.pipeline.clone();
        if !self.database()?.record_file_failure(
            file.id,
            file.entry.revision,
            &pipeline,
            &error,
            !file.segments.is_empty(),
        )? {
            return Ok(());
        }
        self.retry_ready(&file)?;
        file.failed = true;
        if !file.segments.is_empty() {
            file.coverage = format!(
                "partial: {}; retained {} completed segments; planned coverage: {}",
                error.chars().take(240).collect::<String>(),
                file.segments.len(),
                file.coverage
            );
            let published = self.publish_file(file);
            return match published {
                Err(cancelled) if self.shutdown_cancelled(&cancelled) => Err(error),
                result => result,
            };
        }
        Ok(())
    }
    fn background_interrupted(&self, error: &str) -> bool {
        let interrupted = self.dispatch.lock().unwrap().cancel.load(Ordering::Relaxed)
            && matches!(
                error,
                "Model loading cancelled" | "Search model request cancelled"
            );
        if interrupted {
            self.snapshot.lock().unwrap().backend = None;
        }
        interrupted
    }
    fn shutdown_cancelled(&self, error: &str) -> bool {
        self.stop.load(Ordering::Relaxed)
            && matches!(
                error,
                "command cancelled"
                    | "Search cancelled"
                    | "Model loading cancelled"
                    | "Search model request cancelled"
            )
    }
    fn publish_file(&mut self, mut file: FileWork) -> Result<(), String> {
        file.publishing = true;
        if !self.verify_index_file(&file)? {
            self.work.push_back(Work::File(Box::new(file)));
            return Ok(());
        }
        let pipeline = self.pipeline.clone();
        let published = self.database()?.publish(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.segments,
            &file.coverage,
        )?;
        if published {
            if !file.failed {
                self.database()?.clear_file_retry(file.id)?;
            }
            if matches!(file.source_check, SourceCheck::Reuse(_)) {
                self.snapshot.lock().unwrap().reused_vectors += file
                    .segments
                    .iter()
                    .filter(|segment| segment.vector.is_some())
                    .count() as u64;
            }
            let digest = match file.source_check {
                SourceCheck::Pending => None,
                SourceCheck::Extract(digest) => digest,
                SourceCheck::Reuse(digest) => Some(digest),
            };
            if let Some(digest) = digest {
                self.database()?.remember_source(
                    file.id,
                    file.entry.revision,
                    &pipeline,
                    &file.entry,
                    &digest,
                )?;
            }
        }
        Ok(())
    }
    fn read_prepared(&mut self, file: &mut FileWork) -> Result<Option<Prepared>, String> {
        let Some(reply) = self.read_file(
            file,
            Request::Extract {
                root: file.root.clone(),
                entry: file.entry.clone(),
                cursor: file.cursor,
                text_only: false,
            },
        )?
        else {
            return Ok(None);
        };
        match reply {
            Reply::Prepared(prepared) => {
                if let Some(expected) = file.resume_hash.take()
                    && (Sha256::digest(prepared.text.as_bytes()).as_slice() != expected
                        || !prepared.text.is_char_boundary(file.text_cursor))
                {
                    // A decoder's output is not an authoritative saved cursor.
                    // Re-extract and reuse only freshly matched canonical inputs.
                    file.cursor = 0;
                    file.text_cursor = 0;
                    file.published = 0;
                    file.segments.clear();
                    let pipeline = self.pipeline.clone();
                    self.database()?
                        .discard_checkpoint(file.id, file.entry.revision, &pipeline)?;
                    return Ok(None);
                }
                Ok(Some(prepared))
            }
            _ => Err("Invalid extraction reply".into()),
        }
    }
    fn verify_index_file(&mut self, file: &FileWork) -> Result<bool, String> {
        match self.read_file(
            file,
            Request::Verify {
                root: file.root.clone(),
                entry: file.entry.clone(),
            },
        )? {
            None => Ok(false),
            Some(Reply::Verified) => Ok(true),
            _ => Err("Invalid source verification reply".into()),
        }
    }
    fn read_file(&mut self, file: &FileWork, request: Request) -> Result<Option<Reply>, String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("Search cancelled".into());
        }
        self.readers
            .request(file.root_id, file.generation, &file.entry.path, request)
    }
    fn resume_file(&mut self, file: &mut FileWork) -> Result<(), String> {
        let SourceCheck::Extract(digest) = file.source_check else {
            return Ok(());
        };
        let pipeline = self.pipeline.clone();
        if let Some((progress, segments)) = self.database()?.resume_file(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.entry,
            digest,
        )? {
            file.cursor = progress.cursor;
            file.text_cursor = progress.text_cursor;
            file.coverage = progress.coverage;
            file.published = progress.segments;
            file.resume_hash = progress.prepared_hash;
            file.segments = segments;
            tracing::debug!(target: "little_clip_search_timing", segments = file.published,
                "resumed verified indexing checkpoint");
        }
        Ok(())
    }
    fn checkpoint_file(&mut self, file: &mut FileWork) -> Result<bool, String> {
        if !matches!(file.source_check, SourceCheck::Extract(_))
            || file.segments.len().saturating_sub(file.published) < 8
        {
            return Ok(true);
        }
        if !self.verify_index_file(file)? {
            return Ok(false);
        }
        let digest = match file.source_check {
            SourceCheck::Extract(digest) => digest,
            _ => return Err("Checkpoint requires a verified extracted source".into()),
        };
        let progress = Checkpoint {
            cursor: file.cursor,
            text_cursor: file.text_cursor,
            coverage: file.coverage.clone(),
            format: super::store::source_format(&file.entry),
            digest,
            prepared_hash: file
                .prepared
                .as_ref()
                .filter(|_| file.text_cursor > 0)
                .map(|p| Sha256::digest(p.text.as_bytes()).into()),
            segments: file.published,
        };
        let pipeline = self.pipeline.clone();
        if !self.database()?.checkpoint(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.segments,
            &progress,
        )? {
            return Err("Index source changed before checkpoint publication".into());
        }
        file.published = file.segments.len();
        Ok(true)
    }
    fn search(
        &mut self,
        id: u64,
        path: &Path,
        query: &Query,
        hidden: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::Results, String> {
        match &query.operator {
            Operator::Embedding(text) => self.semantic(id, path, query, text, hidden, cancel),
            Operator::Name | Operator::Regex(_) | Operator::Content(_) => {
                self.on_demand(path, query, hidden, cancel)
            }
            Operator::Answer(_) => {
                Err("Answer commands create session columns rather than search results".into())
            }
        }
    }
    fn on_demand(
        &mut self,
        path: &Path,
        query: &Query,
        hidden: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::Results, String> {
        let configured = self
            .config
            .roots
            .iter()
            .filter_map(|address| filesystem::decode_path(address).ok())
            .collect::<std::collections::HashSet<_>>();
        let mut roots = if query.scope == Scope::Indexed && configured.is_empty() {
            Vec::new()
        } else if query.scope == Scope::Indexed {
            self.database()?
                .monitor_roots()?
                .into_iter()
                .filter(|(_, root)| configured.contains(&root.path))
                .map(|(_, root)| super::walk::ScopeRoot::Indexed(self.monitor.bind_source(root)))
                .collect()
        } else {
            vec![super::walk::ScopeRoot::Folder(path.into())]
        };
        if query.progressive && query.scope == Scope::Recursive {
            roots.extend(self.config.always_search.iter().filter_map(|address| {
                filesystem::decode_path(address)
                    .ok()
                    .filter(|path| path.is_absolute())
                    .map(super::walk::ScopeRoot::Folder)
            }));
        }
        super::walk::run(
            roots,
            &self.paths.data,
            self.store.as_ref(),
            &self.exclusions,
            query,
            hidden,
            cancel,
        )
    }
    fn semantic(
        &mut self,
        id: u64,
        path: &Path,
        query: &Query,
        text: &str,
        hidden: bool,
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::Results, String> {
        if self.config.roots.is_empty() && query.scope == Scope::Indexed {
            return Ok(super::Results {
                entries: Vec::new(),
                sources: Vec::new(),
                explanations: Default::default(),
                message: "No folders enrolled. Click Index to enable content and semantic search."
                    .into(),
            });
        }
        let current_enrolled = self
            .config
            .roots
            .iter()
            .filter_map(|address| filesystem::decode_path(address).ok())
            .any(|root| path.starts_with(root));
        if self.config.roots.is_empty() || (query.scope == Scope::Here && !current_enrolled) {
            let mut names = query.clone();
            names.operator = Operator::Name;
            names.name = text.into();
            let mut result = self.on_demand(path, &names, hidden, cancel)?;
            let coverage = if self.config.roots.is_empty() {
                "no folders enrolled"
            } else {
                "current folder not enrolled"
            };
            result.message = format!(
                "Filename results only · {coverage}. Click Index to enable content and semantic search."
            );
            return Ok(result);
        }
        let model_resident = self
            .model
            .as_ref()
            .is_some_and(|model| model.role == Role::Embedding);
        tracing::debug!(target: "little_clip_search_timing",
            request_id = id, model_resident, preliminary_lexical = !model_resident,
            "semantic search scheduling");
        // Cold loads still publish usable lexical results before loading the
        // model. A resident helper goes straight to the full fused pass, which
        // repeats all eligibility/source checks; don't scan and verify twice.
        if !model_resident {
            let mut lexical = self.ranked_results(path, query, text, hidden, &[], cancel)?;
            lexical.message = lexical
                .message
                .replace("Semantic candidates", "Lexical candidates")
                .replace(
                    "hybrid filename/keyword/meaning",
                    "filename/keyword · loading semantic model",
                );
            publish_search_result(
                &self.snapshot,
                SearchResult {
                    id,
                    complete: false,
                    result: Ok(lexical),
                },
            );
        }
        let input = format!("task: search result | query: {text}");
        let vector = self
            .model(Role::Embedding, cancel)
            .and_then(|model| model.texts(&[input], true, cancel));
        match vector {
            Ok(mut vectors) => {
                self.snapshot.lock().unwrap().progress = Some(SearchProgress {
                    id,
                    checked: Arc::new(AtomicU64::new(0)),
                });
                self.ranked_results(path, query, text, hidden, &vectors.remove(0), cancel)
            }
            Err(error) if !cancel.load(Ordering::Relaxed) => {
                self.model = None;
                self.snapshot.lock().unwrap().backend = None;
                // Model loading may have outlived a source edit. Refresh the
                // fallback instead of returning the pre-load verification.
                let mut lexical = self.ranked_results(path, query, text, hidden, &[], cancel)?;
                lexical.message = lexical
                    .message
                    .replace("Semantic candidates", "Lexical candidates")
                    .replace(
                        "hybrid filename/keyword/meaning",
                        "filename/keyword · semantic model unavailable",
                    );
                lexical.message.push_str(&format!(" · {error}"));
                Ok(lexical)
            }
            Err(error) => Err(error),
        }
    }

    fn ranked_results(
        &mut self,
        path: &Path,
        query: &Query,
        text: &str,
        hidden: bool,
        vector: &[f32],
        cancel: &Arc<AtomicBool>,
    ) -> Result<super::Results, String> {
        let started = Instant::now();
        // Lexical text is reusable across model versions, but only from current
        // extraction records. A vector always requires its exact model pipeline.
        let pipeline = if vector.is_empty() {
            String::new()
        } else {
            self.pipeline.clone()
        };
        let floors = self.config.semantic_floors.clone();
        let exclusions = self.exclusions.clone();
        let index_hidden = self.config.include_hidden;
        let show_offline = self.config.show_offline_results;
        let roots = self
            .database()?
            .source_roots()?
            .into_iter()
            .map(|root| self.monitor.bind_source(root))
            .collect::<Vec<_>>();
        let progress = self
            .snapshot
            .lock()
            .unwrap()
            .progress
            .as_ref()
            .filter(|_| !vector.is_empty())
            .map(|progress| progress.checked.clone());
        let store = self.database()?;
        let retrieval_started = Instant::now();
        let hits = super::retrieval::Retrieval {
            progress: progress.as_deref(),
            store,
            pipeline: &pipeline,
            vector,
            path,
            query,
            text,
            hidden,
            exclusions: &exclusions,
            index_hidden,
            show_offline,
            floors: &floors,
            cancel,
        }
        .search()?;
        let retrieval_ms = retrieval_started.elapsed().as_secs_f64() * 1000.0;
        let sources = roots.clone();
        let ranked = hits.iter().map(|hit| hit.entry.clone()).collect::<Vec<_>>();
        let offline = if show_offline {
            store.last_known_paths(&ranked)?
        } else {
            Default::default()
        };
        let live = ranked
            .into_iter()
            .filter(|entry| !offline.contains(&entry.path))
            .collect();
        let verification_started = Instant::now();
        let (mut entries, mut message) = verify_candidates(live, roots, cancel)?;
        let verification_ms = verification_started.elapsed().as_secs_f64() * 1000.0;
        // Preserve fused order while admitting explicitly labelled last-known
        // records. A stale candidate in an available root never takes this path.
        entries.extend(
            hits.iter()
                .filter(|hit| offline.contains(&hit.entry.path))
                .map(|hit| hit.entry.clone()),
        );
        let order = hits
            .iter()
            .enumerate()
            .map(|(i, hit)| (hit.entry.path.clone(), i))
            .collect::<std::collections::HashMap<_, _>>();
        entries.sort_by_key(|entry| order[&entry.path]);
        let evidence_started = Instant::now();
        let mut explanations =
            super::explanation::read(store, &hits, &entries, &pipeline, text, cancel)?;
        let evidence_ms = evidence_started.elapsed().as_secs_f64() * 1000.0;
        for path in &offline {
            explanations.entry(path.clone()).or_default().unavailable = true;
        }
        if !offline.is_empty() {
            message.push_str(&format!(
                " · {} unavailable last-known matches · metadata list",
                offline.len()
            ));
        }
        tracing::debug!(target: "little_clip_search_timing",
            semantic = !vector.is_empty(), retrieval_ms, verification_ms, evidence_ms,
            total_ms = started.elapsed().as_secs_f64() * 1000.0,
            candidates = hits.len(), entries = entries.len(), "search ranking phases");
        Ok(super::Results {
            entries,
            message,
            sources,
            explanations,
        })
    }
}

fn verify_candidates(
    ranked: Vec<Entry>,
    roots: Vec<SourceRoot>,
    stop: &AtomicBool,
) -> Result<(Vec<Entry>, String), String> {
    let verified = reader::verification::current(&ranked, &roots, stop)?;
    let stale = ranked.len() - verified.len();
    Ok((
        ranked
            .into_iter()
            .filter(|entry| verified.contains(&entry.path))
            .collect(),
        format!(
            "Semantic candidates (up to 500) · hybrid filename/keyword/meaning · indexed coverage only · {stale} stale/unavailable omitted"
        ),
    ))
}

// Preserve compatible audio/280-token image cache keys, distinguish frame inputs.
fn media_hash(bytes: &[u8], modality: &str, budget: u32) -> Vec<u8> {
    let mut hash = Sha256::new();
    if modality == "image" && budget != 280 {
        hash.update(format!("vision-tokens:{budget}\0").as_bytes());
    }
    hash.update(bytes);
    hash.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::super::store::TestFixture;
    use super::*;

    fn test_worker(path: &Path, store: Option<Store>) -> Worker {
        Worker {
            paths: Paths {
                data: path.join("unused-search-cache"),
                helpers: PathBuf::new(),
            },
            config: FileSearchConfig::default(),
            exclusions: Vec::new(),
            store,
            model: None,
            dispatch: Default::default(),
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            stop: Arc::new(AtomicBool::new(false)),
            work: VecDeque::new(),
            pipeline: "model".into(),
            last_snapshot: Instant::now(),
            roots_dirty: false,
            finishing: Default::default(),
            pruning: None,
            answers_until: None,
            monitor: super::super::monitor::Monitor::default(),
            capacity_blocked: false,
            readers: Default::default(),
            staged_files: Vec::new(),
            next_probe: 0,
        }
    }

    #[test]
    fn failed_files_wait_without_queued_work_and_manual_rebuild_resets_retries() {
        let fixture = TestFixture::new();
        let root = SourceRoot::from_root(&fixture.root);
        let id = fixture.store.roots().unwrap()[0].id;
        let entry = fixture.entry;
        let file = fixture.file;
        let mut worker = test_worker(&root.path, Some(fixture.store));
        worker.monitor.register(id, root.clone());
        worker
            .file_failure(
                FileWork::new(root.clone(), id, 1, entry.clone(), file),
                "decoder failed".into(),
            )
            .unwrap();
        worker.observe_entry(&root, id, 1, entry, "model").unwrap();
        assert!(worker.work.is_empty(), "backoff retained runnable work");
        assert!(matches!(
            worker.store.as_ref().unwrap().file_retry(file).unwrap(),
            super::super::store::Retry::After(_)
        ));
        let refresh = worker
            .monitor
            .due(&HashSet::new(), Instant::now() + Duration::from_secs(61));
        assert_eq!(refresh.len(), 1);
        assert_eq!(
            refresh[0].directories.as_ref().unwrap(),
            &vec![root.path.clone()]
        );
        worker.command(Command::Rebuild(root.path.clone())).unwrap();
        assert_eq!(
            worker.store.as_ref().unwrap().file_retry(file).unwrap(),
            super::super::store::Retry::Ready
        );
        assert!(matches!(worker.work.front(), Some(Work::Probe { .. })));
    }

    #[test]
    fn queued_file_payloads_keep_staging_admission_until_their_owner_is_dropped() {
        let fixture = TestFixture::new();
        let root = SourceRoot::from_root(&fixture.root);
        let mut worker = test_worker(&root.path, None);
        for id in 1..=MAX_STAGED_FILES as i64 {
            let mut file = FileWork::new(root.clone(), id, 1, fixture.entry.clone(), id);
            assert!(worker.reserve_file_staging(&mut file));
            assert!(
                worker.reserve_file_staging(&mut file),
                "Existing reservation is reused"
            );
            worker.work.push_back(Work::File(Box::new(file)));
        }
        let mut waiting = FileWork::new(root.clone(), 99, 1, fixture.entry.clone(), 99);
        assert!(!worker.reserve_file_staging(&mut waiting));
        worker.work.push_front(Work::File(Box::new(waiting)));
        assert_eq!(
            worker.ready_work(),
            Some(1),
            "A staged file can still make progress"
        );
        worker.work.push_front(Work::Probe {
            id: -1,
            path: root.path.clone(),
            persist: false,
        });
        assert!(
            matches!(worker.next_work(), Some(Work::Probe { .. })),
            "Metadata probes do not retain content buffers"
        );
        // Cancelling/removing an owner releases only its retained-file permit;
        // existing reader retirement keeps its independent two-slot admission.
        worker.work.retain(|work| work.root_id() != 1);
        assert_eq!(worker.ready_work(), Some(0));
        let Some(Work::File(mut waiting)) = worker.next_work() else {
            panic!("Expected the waiting file");
        };
        assert!(
            waiting.staging.is_some(),
            "Scheduling reserves before any source/cache read"
        );
        assert!(worker.reserve_file_staging(&mut waiting));
        assert_eq!(worker.staged_files.len(), MAX_STAGED_FILES);
        let mut another = FileWork::new(root.clone(), 100, 1, fixture.entry.clone(), 100);
        assert!(!worker.reserve_file_staging(&mut another));
        worker.work.clear();
        drop(waiting);
        assert!(worker.reserve_file_staging(&mut another));
        assert_eq!(worker.staged_files.len(), 1);
    }

    fn inventory_fixture(count: usize) -> (TestFixture, Worker, InventoryWork) {
        let mut fixture = TestFixture::new();
        let root = SourceRoot::from_root(&fixture.root);
        let id = fixture.store.roots().unwrap()[0].id;
        let generation = fixture.store.begin_scan(id).unwrap();
        // Synthetic aliases exercise catalog admission, not source I/O.
        let mut entries = VecDeque::from([fixture.entry.clone()]);
        entries.extend((0..count).map(|index| {
            let mut entry = fixture.entry.clone();
            entry.name = format!("alias-{index:06}.txt");
            entry.path = root.path.join(&entry.name);
            entry
        }));
        let remaining_bytes = entries
            .iter()
            .map(reader::Inventory::entry_storage_bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .into_iter()
            .sum();
        let inventory = InventoryWork {
            remaining_bytes,
            _permit: None,
            path: root.path.clone(),
            root,
            id,
            generation,
            recursive: true,
            invalidated: false,
            entries,
            pipeline: "model".into(),
        };
        let worker = test_worker(&inventory.path, None);
        (fixture, worker, inventory)
    }

    #[test]
    fn inventory_application_yields_without_completing_or_retiring_partial_coverage() {
        let (fixture, mut worker, inventory) = inventory_fixture(513);
        worker.store = Some(fixture.store);
        let id = inventory.id;
        let generation = inventory.generation;
        let path = inventory.path.clone();
        let cancel = worker.dispatch.lock().unwrap().cancel.clone();
        cancel.store(true, Ordering::Relaxed);
        worker.apply_inventory(inventory).unwrap();
        assert_eq!(
            worker
                .store
                .as_ref()
                .unwrap()
                .connection
                .query_row("SELECT COUNT(*) FROM paths", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            1
        );
        assert!(worker.dispatch.lock().unwrap().begin_background());
        let mut observed = 1;
        for _ in 0..2000 {
            let Some(Work::Inventory(inventory)) = worker.work.pop_front() else {
                break;
            };
            worker.apply_inventory(*inventory).unwrap();
            let store = worker.store.as_ref().unwrap();
            let count = store
                .connection
                .query_row("SELECT COUNT(*) FROM paths", [], |row| {
                    row.get::<_, usize>(0)
                })
                .unwrap();
            assert!(
                count.saturating_sub(observed) <= 64,
                "unbounded metadata application"
            );
            observed = count;
            assert_eq!(
                store.known_directory(id, &path).unwrap(),
                worker.work.is_empty()
            );
        }
        assert!(worker.work.is_empty());
        assert_eq!(observed, 514);
        let store = worker.store.as_ref().unwrap();
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM memberships WHERE generation=?1 AND verified=1",
                    [generation],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            514
        );
    }

    #[test]
    fn events_abandon_buffered_inventory_and_restart_with_a_new_generation() {
        for whole_root in [false, true] {
            let (fixture, mut worker, inventory) = inventory_fixture(100);
            worker.store = Some(fixture.store);
            let id = inventory.id;
            let generation = inventory.generation;
            let path = inventory.path.clone();
            worker.apply_inventory(inventory).unwrap();
            let Some(Work::Inventory(inventory)) = worker.work.front() else {
                panic!("inventory did not yield")
            };
            let before = inventory.entries.len();
            worker.invalidate_reads(vec![(id, Some(path.join("unrelated")))]);
            assert!(
                matches!(worker.work.front(), Some(Work::Inventory(i)) if !i.invalidated),
                "unrelated notifications must not restart this directory"
            );
            let root = SourceRoot::from_root(&fixture.root);
            let pending = FileWork::new(
                root.clone(),
                id,
                generation,
                fixture.entry.clone(),
                fixture.file,
            );
            worker.work.push_back(Work::File(Box::new(pending)));
            let mut other = fixture.entry.clone();
            other.path = path.join("unrelated/note.txt");
            worker.work.push_back(Work::File(Box::new(FileWork::new(
                root,
                id,
                generation,
                other.clone(),
                fixture.file,
            ))));
            worker.invalidate_reads(vec![(id, (!whole_root).then_some(path.clone()))]);
            let Some(Work::Inventory(inventory)) = worker.work.pop_front() else {
                unreachable!()
            };
            worker.apply_inventory(*inventory).unwrap();
            let Some(Work::Directory {
                generation: replacement,
                remaining: None,
                ..
            }) = worker.work.front()
            else {
                panic!("old snapshot was not replaced")
            };
            assert!(*replacement > generation);
            assert_eq!(
                worker.work.len(),
                2,
                "restart drops pending files only in this directory"
            );
            assert!(
                matches!(worker.work.back(), Some(Work::File(file)) if file.entry.path == other.path)
            );
            let store = worker.store.as_ref().unwrap();
            let total = store
                .connection
                .query_row("SELECT COUNT(*) FROM paths", [], |row| {
                    row.get::<_, usize>(0)
                })
                .unwrap();
            assert_eq!(
                total,
                101 - before,
                "no old entries applied after invalidation"
            );
            assert_eq!(
                store
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM memberships WHERE verified=1",
                        [],
                        |row| row.get::<_, usize>(0)
                    )
                    .unwrap(),
                0
            );
            assert!(!store.known_directory(id, &path).unwrap());
            assert!(
                !store
                    .unseen_children(id, *replacement, &path)
                    .unwrap()
                    .is_empty(),
                "fresh generation must reconsider previously observed removals"
            );
        }
    }

    #[test]
    fn unsupported_indexing_queues_only_final_validation_without_digest_or_decoder() {
        let mut fixture = TestFixture::new();
        let path = fixture.root.path.join("archive.zip");
        std::fs::write(&path, b"owned archive-shaped fixture").unwrap();
        let entry = filesystem::read_directory(&fixture.root.path, &AtomicBool::new(false))
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.path == path)
            .unwrap();
        let root = SourceRoot::from_root(&fixture.root);
        let id = fixture.store.roots().unwrap()[0].id;
        let generation = fixture.store.begin_scan(id).unwrap();
        let (file_id, _) = fixture
            .store
            .observe_object(
                id,
                generation,
                &entry,
                entry.identity.unwrap().object(),
                "model",
            )
            .unwrap();
        let file = FileWork::new(root.clone(), id, generation, entry.clone(), file_id);
        let mut worker = test_worker(&root.path, Some(fixture.store));
        worker.index_file(file).unwrap();
        let Some(Work::File(pending)) = worker.work.front() else {
            panic!("validation was not retained")
        };
        assert!(pending.publishing);
        assert_eq!(pending.coverage, reader::NAME_ONLY_UNSUPPORTED);
        assert!(pending.segments.is_empty() && pending.prepared.is_none());
        assert!(matches!(pending.source_check, SourceCheck::Extract(None)));
        assert!(
            worker
                .readers
                .request(
                    id,
                    generation,
                    &entry.path,
                    Request::Fingerprint {
                        root: root.clone(),
                        entry: entry.clone(),
                    }
                )
                .is_err(),
            "a fingerprint must not replace the owned final-validation request"
        );
        assert!(
            worker
                .readers
                .request(
                    id,
                    generation,
                    &entry.path,
                    Request::Verify {
                        root,
                        entry: entry.clone()
                    }
                )
                .unwrap()
                .is_none()
        );
        worker.cancel_reads(None);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !worker.readers.is_empty() {
            assert!(Instant::now() < deadline);
            worker.readers.poll();
            std::thread::yield_now();
        }
        assert_eq!(worker.snapshot.lock().unwrap().generated_vectors, 0);
        let job = AnswerJob {
            id: 1,
            root: SourceRoot::from_root(&fixture.root),
            entry,
            question: "What is the score?".into(),
            kind: super::super::query::AnswerType::Number,
            cancel: Arc::new(AtomicBool::new(false)),
            plans: Default::default(),
        };
        let answer = worker.answer_file(&job).unwrap();
        assert!(matches!(answer.state, State::Unsupported));
        assert!(
            worker.model.is_none() && worker.readers.is_empty(),
            "unsupported questions must not read a header or start inference"
        );
        worker.exclusions.push(job.entry.path.clone());
        assert!(matches!(
            worker.answer_file(&job).unwrap().state,
            State::Excluded
        ));
    }

    #[test]
    fn pending_enrollment_is_removable_while_indexing_is_paused_without_creating_a_catalog() {
        let fixture = tempfile::tempdir().unwrap();
        let mut worker = test_worker(fixture.path(), None);
        worker.config.paused = true;
        worker.capacity_blocked = true;
        worker.answers_until = Some(Instant::now() + Duration::from_secs(60));
        worker.enroll(fixture.path(), false).unwrap();
        worker.enroll(fixture.path(), true).unwrap();
        assert_eq!(worker.work.len(), 1);
        let id = worker.work[0].root_id();
        assert!(id < 0 && worker.background_ready());
        assert!(matches!(worker.work[0], Work::Probe { persist: true, .. }));
        assert_eq!(worker.snapshot.lock().unwrap().roots[0].state, "checking");
        worker
            .command(Command::Remove(fixture.path().into()))
            .unwrap();
        assert!(worker.work.is_empty());
        assert!(worker.snapshot.lock().unwrap().roots.is_empty());
        assert!(worker.config.roots.is_empty() && worker.store.is_none() && worker.model.is_none());
        assert!(!worker.paths.data.exists());
    }

    #[test]
    fn removal_resolves_the_stored_root_after_a_temporary_probe_has_completed() {
        let fixture = TestFixture::new();
        let path = fixture.root.path.clone();
        let stored = fixture.store.root_id(&path).unwrap().unwrap();
        let mut worker = test_worker(&path, None);
        worker.enroll(&path, true).unwrap();
        let temporary = worker.enrollment_id(&path).unwrap().unwrap();
        assert!(temporary < 0);
        // The UI still holds its old temporary snapshot while the coordinator
        // completes enrollment. A path-based removal resolves the actual root.
        worker.work.clear();
        worker.store = Some(fixture.store);
        assert_eq!(worker.snapshot.lock().unwrap().roots[0].id, temporary);
        assert_eq!(worker.enrollment_id(&path).unwrap(), Some(stored));
    }

    #[test]
    fn queued_reprobe_invalidates_membership_without_retiring_canonical_vectors() {
        let fixture = TestFixture::new();
        let path = fixture.root.path.clone();
        let id = fixture.store.root_id(&path).unwrap().unwrap();
        let file = fixture.file;
        let vector: Vec<u8> = fixture
            .store
            .connection
            .query_row(
                "SELECT vector FROM segments WHERE file_id=?1",
                [file],
                |row| row.get(0),
            )
            .unwrap();
        let mut worker = test_worker(&path, Some(fixture.store));
        worker.enroll(&path, false).unwrap();
        assert_eq!(worker.work[0].root_id(), id);
        let store = worker.store.as_ref().unwrap();
        let retained: Vec<u8> = store
            .connection
            .query_row(
                "SELECT vector FROM segments WHERE file_id=?1",
                [file],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, vector);
        let verified: i64 = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM memberships WHERE root_id=?1 AND verified=1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(verified, 0);
        assert_eq!(store.roots().unwrap()[0].state, "checking");
        assert!(worker.background_ready());
    }

    #[test]
    fn excluded_enrollment_never_queues_a_probe_or_creates_a_catalog() {
        let fixture = tempfile::tempdir().unwrap();
        let mut worker = test_worker(fixture.path(), None);
        assert!(worker.enroll(Path::new("/proc"), true).is_err());
        worker.exclusions.push(fixture.path().into());
        assert!(worker.enroll(fixture.path(), true).is_err());
        assert!(worker.work.is_empty() && worker.store.is_none());
        assert!(!worker.paths.data.exists());
    }

    #[test]
    fn shutdown_during_indexing_preserves_pending_and_checkpointed_files() {
        for partial in [false, true] {
            let mut fixture = TestFixture::new();
            let root = SourceRoot::from_root(&fixture.root);
            let root_id = fixture.store.roots().unwrap()[0].id;
            let segments: Vec<Segment> = if partial {
                (0..8)
                    .map(|index| Segment {
                        modality: "text".into(),
                        location: format!("part {index}"),
                        text: format!("text {index}"),
                        input_hash: vec![index as u8; 32],
                        vector: Some(vec![1.0; super::super::store::DIMENSION]),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if partial {
                let checkpoint = Checkpoint {
                    cursor: 0,
                    text_cursor: 5,
                    coverage: "complete".into(),
                    format: super::super::store::source_format(&fixture.entry),
                    digest: None,
                    prepared_hash: Some([9; 32]),
                    segments: 0,
                };
                assert!(
                    fixture
                        .store
                        .checkpoint(
                            fixture.file,
                            fixture.entry.revision,
                            "model",
                            &segments,
                            &checkpoint
                        )
                        .unwrap()
                );
            } else {
                fixture
                    .store
                    .connection
                    .execute(
                        "UPDATE files SET state='pending' WHERE id=?1",
                        [fixture.file],
                    )
                    .unwrap();
            }
            let file = FileWork {
                published: segments.len(),
                segments,
                ..FileWork::new(
                    root.clone(),
                    root_id,
                    1,
                    fixture.entry.clone(),
                    fixture.file,
                )
            };
            let mut worker = test_worker(&root.path, Some(fixture.store));
            worker.stop.store(true, Ordering::Relaxed);
            worker.advance(Work::File(Box::new(file)));
            let store = worker.store.as_ref().unwrap();
            let (state, error): (String, Option<String>) = store
                .connection
                .query_row(
                    "SELECT state,error FROM files WHERE id=?1",
                    [fixture.file],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(state, if partial { "ready" } else { "pending" });
            assert!(error.is_none());
            let root_state = &store.roots().unwrap()[0];
            assert_eq!(root_state.state, "building");
            assert!(root_state.error.is_none());
            let count: i64 = store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM file_checkpoints WHERE file_id=?1",
                    [fixture.file],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, i64::from(partial));
            if !partial {
                let fresh_file = || {
                    FileWork::new(
                        root.clone(),
                        root_id,
                        1,
                        fixture.entry.clone(),
                        fixture.file,
                    )
                };
                let failure = worker.file_failure(fresh_file(), "decoder failed".into());
                assert_eq!(failure.unwrap_err(), "decoder failed");
                assert!(!worker.shutdown_cancelled("decoder failed"));
                worker.stop.store(false, Ordering::Relaxed);
                worker
                    .file_failure(fresh_file(), "decoder failed".into())
                    .unwrap();
                let (state, error): (String, Option<String>) = worker
                    .store
                    .as_ref()
                    .unwrap()
                    .connection
                    .query_row(
                        "SELECT state,error FROM files WHERE id=?1",
                        [fixture.file],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .unwrap();
                assert_eq!(state, "failed");
                assert_eq!(error.as_deref(), Some("decoder failed"));
            }
        }
    }
    #[test]
    fn media_reuse_separates_vision_token_budgets_but_preserves_compatible_inputs() {
        let bytes = b"same canonical PNG bytes";
        assert_ne!(
            media_hash(bytes, "image", 140),
            media_hash(bytes, "image", 280)
        );
        assert_eq!(
            media_hash(bytes, "image", 280),
            Sha256::digest(bytes).to_vec()
        );
        assert_eq!(
            media_hash(bytes, "audio", 280),
            Sha256::digest(bytes).to_vec()
        );
    }
    fn queued_service() -> (Service, mpsc::Receiver<Command>) {
        let (commands, receiver) = mpsc::sync_channel(1);
        (
            Service {
                commands,
                dispatch: Default::default(),
                snapshot: Arc::new(Mutex::new(Snapshot::default())),
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            },
            receiver,
        )
    }
    #[test]
    fn graceful_close_rejects_new_admission_but_preserves_accepted_maintenance() {
        let (mut service, receiver) = queued_service();
        service.send(Command::Clear).unwrap();
        assert!(!service.allow_close());
        assert!(
            !service.stop.load(Ordering::Relaxed),
            "accepted maintenance was cancelled"
        );
        assert!(service.send(Command::Setup).is_err());
        assert!(matches!(receiver.recv().unwrap(), Command::Clear));
        service.dispatch.lock().unwrap().queued -= 1;
        assert!(!service.allow_close(), "active maintenance was discarded");
        service.snapshot.lock().unwrap().maintenance = false;
        assert!(service.allow_close());
        assert!(service.stop.load(Ordering::Relaxed));
    }

    #[test]
    fn graceful_close_waits_for_the_coordinator_to_reap_its_model() {
        let fixture = tempfile::tempdir().unwrap();
        let mut worker = test_worker(fixture.path(), None);
        model_preemption_fixture(&mut worker, "unused");
        let model = worker.model(Role::Answer, &AtomicBool::new(false)).unwrap();
        let pid = model.info["pid"].as_u64().unwrap();
        let process = PathBuf::from(format!("/proc/{pid}"));
        assert!(process.exists());
        let (mut service, receiver) = queued_service();
        worker.dispatch = service.dispatch.clone();
        worker.snapshot = service.snapshot.clone();
        worker.stop = service.stop.clone();
        service.worker = Some(std::thread::spawn(move || worker.run(receiver)));
        let deadline = Instant::now() + Duration::from_secs(3);
        while !service.allow_close() {
            assert!(
                Instant::now() < deadline,
                "coordinator did not finish shutdown"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            !process.exists(),
            "close was approved before the helper was reaped"
        );
        assert!(service.worker.is_none());
    }
    fn interrupt_catalog(
        worker: &mut Worker,
        table: &str,
    ) -> (mpsc::Receiver<Command>, Arc<AtomicBool>) {
        assert!(matches!(table, "roots" | "files" | "paths"));
        let (service, receiver) = queued_service();
        worker.dispatch = service.dispatch.clone();
        worker.snapshot = service.snapshot.clone();
        worker.stop = service.stop.clone();
        worker.update_roots(true);
        let service = Arc::new(service);
        let admitted = service.clone();
        let read_completed = Arc::new(AtomicBool::new(false));
        let completed = read_completed.clone();
        worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .create_scalar_function(
                "clip_test_read_finished",
                1,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |context| {
                    completed.store(true, Ordering::Relaxed);
                    context.get::<i64>(0)
                },
            )
            .unwrap();
        worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .create_scalar_function(
                "clip_test_accept_command",
                0,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                move |_| {
                    admitted.send(Command::Setup).unwrap();
                    Ok(true)
                },
            )
            .unwrap();
        // Only this owned fixture changes its schema. The callback admits an
        // actual command before a deliberately expensive read reaches a result.
        worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .execute_batch(&format!(
                "ALTER TABLE {table} RENAME TO clip_test_records;
             CREATE VIEW {table} AS SELECT * FROM clip_test_records
             WHERE (WITH RECURSIVE cost(n) AS (VALUES(clip_test_accept_command()) UNION ALL SELECT n+1 FROM cost WHERE n<1000000)
                SELECT clip_test_read_finished(sum(n)) FROM cost)>0;"
            ))
            .unwrap();
        (receiver, read_completed)
    }

    #[test]
    fn accepted_command_interrupts_root_summary_and_retry_preserves_snapshot_and_writes() {
        let fixture = TestFixture::new();
        let mut worker = test_worker(&fixture.root.path, Some(fixture.store));
        let (receiver, read_completed) = interrupt_catalog(&mut worker, "roots");
        assert_eq!(worker.snapshot.lock().unwrap().roots[0].vectors, 1);
        worker.update_roots(true);
        assert!(
            !read_completed.load(Ordering::Relaxed),
            "accepted command must interrupt before the expensive read completes"
        );
        assert!(worker.roots_dirty);
        assert!(worker.snapshot.lock().unwrap().error.is_none());
        assert_eq!(worker.snapshot.lock().unwrap().roots[0].vectors, 1);
        assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
        assert_eq!(worker.dispatch.lock().unwrap().queued, 1);
        // The guard must already be gone: accepted writes cannot inherit the
        // cancelled read's progress handler, even while its token remains set.
        worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .execute_batch(
                "UPDATE clip_test_records SET generation=(WITH RECURSIVE cost(n) AS
               (VALUES(1) UNION ALL SELECT n+1 FROM cost WHERE n<10000) SELECT sum(n) FROM cost);
             DROP VIEW roots; ALTER TABLE clip_test_records RENAME TO roots;",
            )
            .unwrap();
        worker.update_roots(true);
        assert!(
            worker.roots_dirty,
            "queued command must keep summary work deferred"
        );
        worker.dispatch.lock().unwrap().queued -= 1;
        worker.last_snapshot = Instant::now() - Duration::from_secs(1);
        worker.update_roots(false);
        assert!(!worker.roots_dirty);
        let snapshot = worker.snapshot.lock().unwrap();
        assert!(snapshot.error.is_none());
        assert_eq!((snapshot.roots[0].files, snapshot.roots[0].vectors), (1, 1));
    }

    #[test]
    fn accepted_command_interrupts_completion_without_publishing_or_losing_retry() {
        let fixture = TestFixture::new();
        let id = fixture.store.roots().unwrap()[0].id;
        let other = tempfile::tempdir().unwrap();
        let extra = fixture
            .store
            .enroll(&super::super::policy::Root::open(other.path()).unwrap())
            .unwrap();
        fixture
            .store
            .connection
            .execute("UPDATE files SET state='pending'", [])
            .unwrap();
        let mut worker = test_worker(&fixture.root.path, Some(fixture.store));
        let (receiver, completed) = interrupt_catalog(&mut worker, "files");
        worker.finishing.extend([id, extra]);
        worker.finish_root();
        assert!(!completed.load(Ordering::Relaxed));
        assert_eq!(worker.finishing, VecDeque::from([extra, id]));
        assert!(worker.snapshot.lock().unwrap().error.is_none());
        assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
        worker.store.as_ref().unwrap().connection.execute_batch("UPDATE clip_test_records SET state='ready',revision=revision+(WITH RECURSIVE cost(n) AS
            (VALUES(1) UNION ALL SELECT n+1 FROM cost WHERE n<10000) SELECT sum(n)*0 FROM cost);
            DROP VIEW files; ALTER TABLE clip_test_records RENAME TO files;").unwrap();
        worker.finish_root();
        assert_eq!(
            worker.finishing,
            VecDeque::from([extra, id]),
            "queued control did not defer completion"
        );
        worker.dispatch.lock().unwrap().queued -= 1;
        worker.finish_root();
        assert_eq!(worker.finishing, VecDeque::from([id]));
        worker.finish_root();
        assert!(worker.finishing.is_empty());
        assert_eq!(
            worker.store.as_ref().unwrap().roots().unwrap()[0].state,
            "ready"
        );
    }

    #[test]
    fn accepted_command_interrupts_prune_selection_and_writes_do_not_inherit_guard() {
        let fixture = TestFixture::new();
        fixture
            .store
            .connection
            .execute("DELETE FROM memberships", [])
            .unwrap();
        let mut worker = test_worker(&fixture.root.path, Some(fixture.store));
        let (receiver, completed) = interrupt_catalog(&mut worker, "paths");
        worker.pruning = Some(Default::default());
        worker.prune_idle();
        assert!(!completed.load(Ordering::Relaxed));
        assert!(worker.pruning.is_some());
        assert!(worker.snapshot.lock().unwrap().error.is_none());
        assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
        worker.store.as_ref().unwrap().connection.execute_batch("UPDATE clip_test_records SET parent=parent WHERE
            (WITH RECURSIVE cost(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM cost WHERE n<10000) SELECT sum(n) FROM cost)>0;
            DROP VIEW paths; ALTER TABLE clip_test_records RENAME TO paths;").unwrap();
        worker.prune_idle();
        assert!(worker.pruning.is_some());
        worker.dispatch.lock().unwrap().queued -= 1;
        for _ in 0..10 {
            worker.prune_idle();
        }
        assert!(worker.pruning.is_none());
        let rows: usize = worker
            .store
            .as_ref()
            .unwrap()
            .connection
            .query_row("SELECT count(*) FROM segments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn accepted_commands_preempt_background_but_queue_rejection_does_not() {
        let (service, receiver) = queued_service();
        let token = service.dispatch.lock().unwrap().cancel.clone();
        assert!(service.dispatch.lock().unwrap().begin_background());
        service.send(Command::Setup).unwrap();
        assert!(token.load(Ordering::Relaxed));
        assert!(!service.dispatch.lock().unwrap().begin_background());
        assert!(token.load(Ordering::Relaxed));
        receiver.recv().unwrap();
        service.dispatch.lock().unwrap().queued -= 1;
        assert!(service.dispatch.lock().unwrap().begin_background());
        assert!(!token.load(Ordering::Relaxed));
        service.send(Command::Setup).unwrap();
        // Rejection must neither add phantom pending work nor interrupt again.
        token.store(false, Ordering::Relaxed);
        assert!(service.send(Command::Setup).is_err());
        assert!(!token.load(Ordering::Relaxed));
        assert_eq!(service.dispatch.lock().unwrap().queued, 1);
        let dispatch = service.dispatch.clone();
        drop(service);
        assert!(token.load(Ordering::Relaxed));
        dispatch.lock().unwrap().queued = 0;
        assert!(!dispatch.lock().unwrap().begin_background());
        assert!(
            token.load(Ordering::Relaxed),
            "shutdown cancellation must not be reset"
        );
    }

    #[test]
    fn rejected_controls_preserve_interactive_work_and_accepted_controls_cancel_it() {
        let (service, receiver) = queued_service();
        let cancel = Arc::new(AtomicBool::new(false));
        service
            .send(Command::Search {
                id: 1,
                path: "/ordinary".into(),
                query: Query::parse(":emb:\"ordinary\"").unwrap(),
                hidden: false,
                cancel: cancel.clone(),
            })
            .unwrap();
        assert!(service.send(Command::Clear).is_err());
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(!service.maintenance_busy());
        receiver.recv().unwrap();
        service.send(Command::Clear).unwrap();
        assert!(cancel.load(Ordering::Relaxed));
        assert!(service.maintenance_busy());
        assert!(service.dispatch.lock().unwrap().interactive.is_none());
    }

    fn model_preemption_fixture(worker: &mut Worker, stage: &str) {
        let paths = &mut worker.paths;
        paths.helpers = paths.data.join("helpers");
        std::fs::create_dir_all(&paths.helpers).unwrap();
        std::fs::create_dir_all(paths.runtime().join("lib")).unwrap();
        std::fs::create_dir_all(paths.models()).unwrap();
        for name in [
            "embeddinggemma-2-Q8_0.gguf",
            "mmproj-embeddinggemma-2-Q8_0.gguf",
            "Qwen_Qwen3.5-9B-Q8_0.gguf",
        ] {
            std::fs::write(paths.models().join(name), b"").unwrap();
        }
        std::fs::write(paths.runtime().join("lib/libclip-search.so"), b"").unwrap();
        std::fs::write(paths.runtime().join("lib/libclip-answer.so"), b"").unwrap();
        std::fs::write(paths.helpers.join("setup.py"), b"").unwrap();
        std::fs::write(paths.helpers.join("stage"), stage).unwrap();
        std::fs::write(paths.helpers.join("worker.py"), r#"import base64,json,os,struct,sys,time
from pathlib import Path
root=Path(__file__).parent
stage=(root/'stage').read_text()
def hold(name):
    if stage==name and not (root/'release').exists():
        (root/'held.tmp').write_text(str(os.getpid()))
        (root/'held.tmp').replace(root/'held')
        time.sleep(30)
hold('load')
backend=sys.argv[sys.argv.index('--device')+1]
role=sys.argv[sys.argv.index('--role')+1]
pipeline={'provider':sys.argv[sys.argv.index('--answer-provider')+1]} if role=='answer' else 'fixture'
print(json.dumps({'ready':True,'role':role,'dimension':768,'backend':backend,'device':'test','pipeline':pipeline,'pid':os.getpid()}),flush=True)
for line in sys.stdin:
    request=json.loads(line)
    hold(request['op'])
    result={'id':request['id'],'ok':True}
    if request['op']=='passage':
        size=len(request['text'].encode())
        result.update(end=size,next=size)
    else:
        vector=struct.pack('<f',768**-0.5)*768
        result.update(dimension=768,count=1,vectors=base64.b64encode(vector).decode())
    print(json.dumps(result),flush=True)
"#).unwrap();
    }

    #[test]
    fn idle_model_release_ignores_unrelated_queued_index_work() {
        for role in [Role::Embedding, Role::Answer] {
            let (_fixture, mut worker, inventory) = inventory_fixture(1);
            model_preemption_fixture(&mut worker, "unused");
            let cancel = AtomicBool::new(false);
            let model = worker.model(role, &cancel).unwrap();
            let pid = model.info["pid"].as_u64().unwrap();
            let process = PathBuf::from(format!("/proc/{pid}"));
            assert!(process.exists());
            worker.work.push_back(Work::Inventory(Box::new(inventory)));
            // An admitted command prevents indexing from consuming this queue.
            worker.dispatch.lock().unwrap().queued = 1;

            worker.model.as_mut().unwrap().last_used = Instant::now();
            worker.background_tick();
            assert!(worker.model.is_some(), "recently used model was released");

            worker.model.as_mut().unwrap().last_used = Instant::now() - Duration::from_secs(301);
            worker.config.keep_model_loaded = true;
            worker.background_tick();
            assert!(worker.model.is_some(), "keep-loaded preference was ignored");
            assert!(process.exists());

            worker.config.keep_model_loaded = false;
            worker.background_tick();
            assert!(
                worker.model.is_none(),
                "queued metadata retained an idle model"
            );
            assert!(worker.snapshot.lock().unwrap().backend.is_none());
            assert!(!process.exists(), "idle helper was not killed and reaped");
            assert_eq!(worker.work.len(), 1, "model release discarded queued work");
        }
    }

    #[test]
    fn command_preempts_model_load_and_inference_without_losing_passages_or_media() {
        for stage in ["load", "text", "image"] {
            let fixture = TestFixture::new();
            let root = SourceRoot::from_root(&fixture.root);
            let root_id = fixture.store.roots().unwrap()[0].id;
            let mut file = FileWork::new(root.clone(), root_id, 1, fixture.entry, fixture.file);
            let prepared = Prepared {
                text: "first passage must survive interruption".into(),
                data: (stage == "image").then(|| STANDARD.encode(b"owned prepared image")),
                modality: "image".into(),
                location: "fixture".into(),
                next: None,
                coverage: "complete".into(),
                vision_tokens: Some(280),
            };
            file.prepared = Some(prepared);
            let mut worker = test_worker(&root.path, Some(fixture.store));
            model_preemption_fixture(&mut worker, stage);
            let (mut service, receiver) = queued_service();
            service.dispatch = worker.dispatch.clone();
            let helpers = worker.paths.helpers.clone();
            let command = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !helpers.join("held").exists() {
                    assert!(Instant::now() < deadline, "model did not reach {helpers:?}");
                    std::thread::sleep(Duration::from_millis(5));
                }
                let pid = std::fs::read_to_string(helpers.join("held")).unwrap();
                service.send(Command::Setup).unwrap();
                (service, pid)
            });
            let started = Instant::now();
            if stage == "image" {
                let prepared = file.prepared.take().unwrap();
                worker.finish_file(file, prepared).unwrap();
            } else {
                worker.extract_file(file).unwrap();
            }
            let (service, pid) = command.join().unwrap();
            assert!(started.elapsed() < Duration::from_secs(3));
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "child must be reaped before yielding"
            );
            assert!(matches!(receiver.recv().unwrap(), Command::Setup));
            worker.dispatch.lock().unwrap().queued -= 1;
            assert!(worker.dispatch.lock().unwrap().begin_background());
            let Some(Work::File(mut retry)) = worker.work.pop_front() else {
                panic!("lost interrupted file")
            };
            assert_eq!(retry.text_cursor, 0);
            assert!(retry.segments.is_empty());
            assert!(retry.prepared.is_some());
            assert!(worker.snapshot.lock().unwrap().error.is_none());
            let retries: usize = worker
                .store
                .as_ref()
                .unwrap()
                .connection
                .query_row("SELECT count(*) FROM file_retries", [], |row| row.get(0))
                .unwrap();
            assert_eq!(retries, 0, "command preemption consumed a file retry");
            std::fs::write(worker.paths.helpers.join("release"), b"").unwrap();
            let segment = if stage == "image" {
                worker
                    .media_segment(retry.prepared.as_ref().unwrap())
                    .unwrap()
            } else {
                let segment = worker.text_segment(&mut retry).unwrap();
                assert_eq!(
                    retry.text_cursor,
                    retry.prepared.as_ref().unwrap().text.len()
                );
                assert_eq!(segment.text, "first passage must survive interruption");
                segment
            };
            assert_eq!(
                segment.vector.unwrap().len(),
                super::super::store::DIMENSION
            );
            assert_eq!(worker.snapshot.lock().unwrap().generated_vectors, 1);
            drop(service);
        }
    }

    #[test]
    fn maintenance_admission_interrupts_a_foreground_model_and_reaps_before_return() {
        let fixture = tempfile::tempdir().unwrap();
        let mut worker = test_worker(fixture.path(), None);
        model_preemption_fixture(&mut worker, "answer");
        let (mut service, receiver) = queued_service();
        service.dispatch = worker.dispatch.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        service
            .send(Command::Search {
                id: 1,
                path: fixture.path().into(),
                query: Query::parse(":emb:\"ordinary\"").unwrap(),
                hidden: false,
                cancel: cancel.clone(),
            })
            .unwrap();
        receiver.recv().unwrap();
        worker.dispatch.lock().unwrap().queued -= 1;
        let helpers = worker.paths.helpers.clone();
        let admission = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !helpers.join("held").exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            let pid = std::fs::read_to_string(helpers.join("held")).unwrap();
            service.send(Command::Clear).unwrap();
            (service, pid)
        });
        let start = Instant::now();
        let error = worker
            .model(Role::Answer, &cancel)
            .unwrap()
            .answer(
                serde_json::json!({"op":"answer"}),
                Duration::from_secs(30),
                &cancel,
            )
            .unwrap_err();
        let (service, pid) = admission.join().unwrap();
        assert_eq!(error, "Search model request cancelled");
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        assert!(matches!(receiver.recv().unwrap(), Command::Clear));
        assert!(
            service.maintenance_busy(),
            "accepted mutation retains its completion lifecycle"
        );
    }

    #[test]
    fn search_publication_wakes_after_unlock_and_registration_cannot_miss_a_result() {
        let (service, _receiver) = queued_service();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |_| {});
        }
        assert!(!ctx.has_requested_repaint());
        let calls = Arc::new(AtomicU64::new(0));
        let observed = calls.clone();
        let snapshot = service.snapshot.clone();
        ctx.set_request_repaint_callback(move |_| {
            assert!(
                snapshot.try_lock().is_ok(),
                "callback ran under snapshot lock"
            );
            observed.fetch_add(1, Ordering::Relaxed);
        });
        service.snapshot.lock().unwrap().progress = Some(SearchProgress {
            id: 7,
            checked: Arc::new(AtomicU64::new(2048)),
        });
        let result = |complete| SearchResult {
            id: 7,
            complete,
            result: Ok(super::super::Results {
                entries: Vec::new(),
                sources: Vec::new(),
                message: String::new(),
                explanations: Default::default(),
            }),
        };
        // Completion before registration must still wake the first consumer.
        publish_search_result(&service.snapshot, result(false));
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            service.snapshot.lock().unwrap().checked_segments(7),
            Some(2048)
        );
        service.set_repaint_context(&ctx);
        assert!(ctx.has_requested_repaint());
        assert!(calls.load(Ordering::Relaxed) > 0);
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |_| {});
        }
        let previous = calls.load(Ordering::Relaxed);
        // Publication after registration clears final progress and wakes too.
        publish_search_result(&service.snapshot, result(true));
        assert!(calls.load(Ordering::Relaxed) > previous);
        let mut snapshot = service.snapshot.lock().unwrap();
        assert!(snapshot.result.as_ref().unwrap().complete);
        assert!(snapshot.progress.is_none());
        snapshot.repaint = None; // The test callback itself captures this snapshot.
    }
    #[test]
    fn progress_is_request_scoped_and_replaced_only_after_queue_admission() {
        let (service, receiver) = queued_service();
        let checked = Arc::new(AtomicU64::new(2048));
        service.snapshot.lock().unwrap().progress = Some(SearchProgress {
            id: 4,
            checked: checked.clone(),
        });
        service.send(Command::Setup).unwrap();
        let next = || Command::Search {
            id: 5,
            path: PathBuf::from("/unused"),
            query: Query::parse(":image").unwrap(),
            hidden: false,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        assert!(service.send(next()).is_err());
        {
            let snapshot = service.snapshot.lock().unwrap();
            assert_eq!(snapshot.checked_segments(4), Some(2048));
            assert_eq!(snapshot.checked_segments(5), None);
        }
        receiver.try_recv().unwrap();
        service.send(next()).unwrap();
        checked.store(8192, Ordering::Relaxed); // Retired worker may still own its counter.
        assert_eq!(service.snapshot.lock().unwrap().checked_segments(4), None);
        assert_eq!(service.snapshot.lock().unwrap().checked_segments(5), None);
    }

    #[test]
    fn pending_search_paints_progress_only_for_its_current_request() {
        let (service, _receiver) = queued_service();
        service.snapshot.lock().unwrap().progress = Some(SearchProgress {
            id: 7,
            checked: Arc::new(AtomicU64::new(4500)),
        });
        let directory = tempfile::tempdir().unwrap();
        let mut manager = crate::file_manager::FileManager::new(directory.path().into());
        manager.search.service = Some(service);
        manager.search.open = true;
        manager.search.draft = ":emb:\"abstract meaning\"".into();
        let ctx = egui::Context::default();
        for (request, pending, expected) in [(7, true, true), (8, true, false), (7, false, false)] {
            manager.search.request_id = request;
            manager.search.pending = pending;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| manager.search_ui(ui));
                },
            );
            let painted = output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text().contains("4500 indexed parts checked")));
            assert_eq!(painted, expected, "request {request}, pending {pending}");
        }
    }

    #[test]
    fn accepted_maintenance_blocks_close_before_worker_dispatch() {
        let (service, _receiver) = queued_service();
        service
            .send(Command::Backup(PathBuf::from("/unused/backup.sqlite")))
            .unwrap();
        assert!(service.maintenance_busy());
        assert!(service.send(Command::Clear).is_err());
        let directory = tempfile::tempdir().unwrap();
        let mut manager = crate::file_manager::FileManager::new(directory.path().into());
        manager.search.service = Some(service);
        assert!(manager.filesystem_busy());
    }
    #[test]
    fn rejected_maintenance_does_not_leave_close_blocked() {
        let (service, receiver) = queued_service();
        service.snapshot.lock().unwrap().maintenance_outcome =
            Some(Ok("Previous backup saved".into()));
        service.send(Command::Setup).unwrap();
        assert!(service.send(Command::Clear).is_err());
        assert!(!service.maintenance_busy());
        assert_eq!(
            service.snapshot.lock().unwrap().maintenance_outcome,
            Some(Ok("Previous backup saved".into()))
        );
        assert!(matches!(receiver.try_recv().unwrap(), Command::Setup));
    }
}
