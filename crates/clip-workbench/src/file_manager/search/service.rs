//! One background owner for index transactions, inference, and reconciliation work.
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
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
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
    Remove(i64),
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
pub(in crate::file_manager) struct Service {
    commands: mpsc::SyncSender<Command>,
    pub(super) snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    query_cancel: Mutex<Option<Arc<AtomicBool>>>,
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
        let mut worker = Worker {
            paths,
            config,
            exclusions,
            store: None,
            model: None,
            snapshot: snapshot.clone(),
            stop: stop.clone(),
            work: VecDeque::new(),
            pipeline: String::new(),
            last_snapshot: Instant::now(),
            answers_until: None,
            monitor: super::monitor::Monitor::default(),
            capacity_blocked: false,
        };
        std::thread::Builder::new()
            .name("file-search".into())
            .spawn(move || worker.run(incoming))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            snapshot,
            stop,
            query_cancel: Mutex::new(None),
        })
    }
    pub(in crate::file_manager) fn maintenance_busy(&self) -> bool {
        self.snapshot.lock().unwrap().maintenance
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
        if self.commands.try_send(command).is_err() {
            if maintenance {
                snapshot.maintenance = false;
            }
            return Err("Search is busy; retry shortly".into());
        }
        if maintenance {
            snapshot.maintenance_outcome = None;
        }
        if cancel.is_some() {
            snapshot.progress = None;
        }
        drop(snapshot);
        if let Some(cancel) = cancel
            && let Some(old) = self.query_cancel.lock().unwrap().replace(cancel)
        {
            old.store(true, Ordering::Relaxed);
        }
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
        if let Some(cancel) = self.query_cancel.lock().unwrap().take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

const MAX_FILE_SEGMENTS: usize = 2048;

#[derive(Clone, Copy)]
enum SourceCheck {
    Pending,
    Extract(Option<[u8; 32]>),
    Reuse([u8; 32]),
}
struct FileWork {
    root: SourceRoot,
    root_id: i64,
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
}
enum Work {
    Directory {
        root: SourceRoot,
        id: i64,
        generation: i64,
        path: PathBuf,
        recursive: bool,
    },
    File(Box<FileWork>),
}
impl Work {
    fn root_id(&self) -> i64 {
        match self {
            Self::Directory { id, .. } => *id,
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
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    work: VecDeque<Work>,
    pipeline: String,
    last_snapshot: Instant,
    answers_until: Option<Instant>,
    monitor: super::monitor::Monitor,
    capacity_blocked: bool,
}
impl Worker {
    fn run(&mut self, incoming: mpsc::Receiver<Command>) {
        self.restore_enrollment();
        while !self.stop.load(Ordering::Relaxed) {
            match incoming.recv_timeout(if self.background_ready() {
                Duration::ZERO
            } else {
                Duration::from_millis(100)
            }) {
                Ok(command) => {
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
            self.monitor_changes();
            self.check_storage_fault();
            if let Some(store) = &self.store {
                store.release_idle_vectors(Duration::from_secs(300));
            }
            if let Some(work) = self
                .background_ready()
                .then(|| self.work.pop_front())
                .flatten()
            {
                self.advance(work);
            }
            if !self.config.keep_model_loaded
                && (self.work.is_empty() || self.config.paused || self.capacity_blocked)
                && self
                    .model
                    .as_ref()
                    .is_some_and(|model| model.last_used.elapsed() > Duration::from_secs(300))
            {
                self.model = None;
                self.snapshot.lock().unwrap().backend = None;
            }
        }
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
                    if self.config.roots.iter().any(|address| {
                        filesystem::decode_path(address).ok().as_ref() == Some(&root.path)
                    }) {
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
        let changed = !invalidations.is_empty();
        if let Some(store) = &self.store {
            for (id, path) in invalidations {
                self.report(store.invalidate_directory(id, path.as_deref()));
            }
        }
        if changed {
            self.update_roots(true);
        }
        if self.config.paused
            || self.capacity_blocked
            || self
                .answers_until
                .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let busy = self.work.iter().map(Work::root_id).collect();
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
            });
        }
        self.update_roots(true);
        Ok(())
    }
    fn background_ready(&self) -> bool {
        !self.config.paused
            && !self.capacity_blocked
            && !self.work.is_empty()
            && self
                .answers_until
                .is_none_or(|until| Instant::now() >= until)
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
        let root_id = work.root_id();
        let admitted = self.admit(
            match &work {
                Work::File(file) => Some(file),
                _ => None,
            },
            0,
            0,
        );
        if !matches!(admitted, Ok(true)) {
            if let Err(error) = admitted {
                self.pause_capacity(format!("Cannot verify index storage: {error}"));
            }
            self.work.push_front(work);
            return;
        }
        if let Err(error) = self.step(work) {
            if self.store.as_ref().is_some_and(Store::storage_failed) {
                self.pause_capacity(format!("Index storage write failed: {error} · check disk space and retry in Search settings"));
            }
            if let Some(store) = &self.store {
                let _ = store.finish_scan(root_id, Some(&error));
            }
            self.work.retain(|work| work.root_id() != root_id);
            self.monitor.failed(root_id);
            self.model = None;
            self.snapshot.lock().unwrap().backend = None;
            self.report(Err(error));
        } else if !self.work.iter().any(|work| work.root_id() == root_id)
            && let Some(store) = &self.store
        {
            let _ = store.finish_scan(root_id, None);
            self.monitor.succeeded(root_id);
        }
        if self.work.is_empty()
            && let Some(store) = &mut self.store
        {
            let result = store.prune_unreferenced();
            self.report(result);
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
        if let Err(error) = result {
            self.snapshot.lock().unwrap().error = Some(error);
        }
    }
    fn update_roots(&mut self, force: bool) {
        if !force && self.last_snapshot.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_snapshot = Instant::now();
        if let Some(store) = &self.store {
            match store.roots() {
                Ok(mut roots) => {
                    for root in &mut roots {
                        root.mount = self
                            .monitor
                            .expected(&root.path)
                            .and_then(|source| source.mount);
                    }
                    self.snapshot.lock().unwrap().roots = roots;
                }
                Err(error) => self.report(Err(error)),
            }
        }
    }
    fn database(&mut self) -> Result<&mut Store, String> {
        if self.store.is_none() {
            let mut store = Store::open(&self.paths.data)?;
            let configured = self
                .config
                .roots
                .iter()
                .filter_map(|address| filesystem::decode_path(address).ok())
                .collect::<Vec<_>>();
            store.disable_unconfigured(&configured)?;
            self.store = Some(store);
            self.snapshot.lock().unwrap().has_index = true;
        }
        Ok(self.store.as_mut().unwrap())
    }
    fn model(&mut self, role: Role, cancel: &AtomicBool) -> Result<&mut Model, String> {
        if self.model.as_ref().is_some_and(|model| model.role != role) {
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
            self.snapshot.lock().unwrap().backend =
                model.info["device"].as_str().map(str::to_owned);
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
                self.enroll(&path, false)
            }
            Command::Remove(id) => {
                self.retry_capacity();
                self.work.retain(|work| work.root_id() != id);
                let path = self
                    .database()?
                    .monitor_roots()?
                    .into_iter()
                    .find(|(root, _)| *root == id)
                    .map(|(_, root)| root.path);
                self.database()?.remove_root(id)?;
                self.monitor.remove(id, path.as_deref());
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
                State::Unsupported,
                "Excluded by search preferences.",
            ));
        }
        if !job.entry.regular_file || job.entry.symlink || job.entry.is_playable_media() {
            return Ok(Answer::status(
                State::Unsupported,
                "This answer path supports readable documents and still images; audio/video questions are not supported.",
            ));
        }
        let started = Instant::now();
        let structured = self.answer_structured(job);
        let used_plan = matches!(&structured, Ok(answer::extraction::Attempt::Answer(_)));
        let answer = match structured {
            Ok(answer::extraction::Attempt::Answer(answer)) => answer,
            fallback => {
                let feedback = match &fallback {
                    Ok(answer::extraction::Attempt::Escalate(reason)) => Some(reason.as_str()),
                    _ => None,
                };
                let mut answer = self.answer_document(job, feedback)?;
                match fallback {
                    Ok(answer::extraction::Attempt::Escalate(reason)) => {
                        answer.reason.push_str(&format!(
                            " Metadata candidate review requested full-document answering: {reason}"
                        ));
                    }
                    Err(_) => answer.reason.push_str(
                        " Structured extraction/review was unavailable; used document answering.",
                    ),
                    _ => {}
                }
                answer
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
        answer::extraction::try_answer(
            &source,
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
    ) -> Result<Answer, String> {
        let document = match answer::read_document(&job.root, &job.entry, &job.cancel) {
            Ok(document) => document,
            Err(error) => return Ok(Answer::status(State::Unavailable, error)),
        };
        if document.needs_vision()
            && !self
                .paths
                .models()
                .join("mmproj-Qwen_Qwen3.5-9B-f16.gguf")
                .is_file()
        {
            return Ok(Answer::status(
                State::Unsupported,
                "Vision projection is not installed. Open Index and choose Install answer model.",
            ));
        }
        answer::answer_prepared(
            document,
            &job.question,
            job.kind,
            &job.cancel,
            |mut request, timeout| {
                if let Some(feedback) = feedback {
                    request["review_feedback"] = serde_json::json!(feedback);
                }
                self.model(Role::Answer, &job.cancel)?
                    .answer(request, timeout, &job.cancel)
            },
        )
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
    fn enroll(&mut self, path: &Path, persist: bool) -> Result<(), String> {
        let result = self.start_scan(path, persist);
        if let Err(error) = &result {
            if let Some(store) = &self.store {
                match store.unavailable(path, error) {
                    Ok(Some(id)) => {
                        self.work.retain(|work| work.root_id() != id);
                        self.monitor.failed(id);
                    }
                    Ok(None) => {}
                    Err(error) => self.report(Err(error)),
                }
            }
            self.update_roots(true);
        }
        result
    }
    fn start_scan(&mut self, path: &Path, persist: bool) -> Result<(), String> {
        if super::policy::excluded_by_user(path, &self.exclusions) {
            return Err("Folder is excluded by search preferences".into());
        }
        let Reply::Root(root) = reader::call(&Request::Probe(path.into()), &self.stop)? else {
            return Err("Invalid enrollment reply".into());
        };
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
        self.work.retain(|work| work.root_id() != id);
        let generation = self.database()?.begin_scan(id)?;
        self.monitor.started(id, root.clone());
        self.work.push_back(Work::Directory {
            path: root.path.clone(),
            root,
            id,
            generation,
            recursive: true,
        });
        self.update_roots(true);
        Ok(())
    }
    fn step(&mut self, work: Work) -> Result<(), String> {
        match work {
            Work::Directory {
                root,
                id,
                generation,
                path,
                recursive,
            } => self.scan(root, id, generation, path, recursive),
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
        // Old watches or queued deltas can outlive a policy change. Retire
        // generated references without enumerating an excluded source again.
        if self.index_excludes(&path, &root.path) {
            return self.database()?.retire_path(id, &path);
        }
        let Some(directory) = self.read_inventory(&root, id, &path)? else {
            self.work.push_front(Work::Directory {
                root,
                id,
                generation,
                path,
                recursive,
            });
            return Ok(());
        };
        // Identify the pipeline before admitting current segment reuse.
        let stop = self.stop.clone();
        self.model(Role::Embedding, &stop)?;
        let pipeline = self.pipeline.clone();
        for entry in directory.entries {
            if self.index_excludes(&entry.path, &root.path) {
                continue;
            }
            if entry.directory && !recursive && self.database()?.known_directory(id, &entry.path)? {
                continue;
            }
            self.observe_entry(&root, id, generation, entry, &pipeline)?;
        }
        self.finish_directory_inventory(&root, id, generation, &path)
    }
    fn read_inventory(
        &mut self,
        root: &SourceRoot,
        id: i64,
        path: &Path,
    ) -> Result<Option<filesystem::Directory>, String> {
        self.snapshot.lock().unwrap().message =
            format!("Scanning {}", filesystem::display_path(path));
        if let Err(error) = self.monitor.watch(id, root, path, &self.stop) {
            self.monitor.failed(id);
            self.snapshot.lock().unwrap().error = Some(format!(
                "Change monitoring incomplete: {error}; periodic reconciliation remains enabled"
            ));
        }
        let Reply::Directory(directory) = reader::call(
            &Request::List {
                output: None,
                root: root.clone(),
                path: path.into(),
                hidden: self.config.include_hidden,
                exclusions: self.exclusions.clone(),
            },
            &self.stop,
        )?
        else {
            return Err("Invalid directory reply".into());
        };
        if directory.skipped > 0 {
            return Err("Directory enumeration incomplete; existing inventory was retained".into());
        }
        // Reserve the complete bounded inventory batch before any row writes;
        // per-file inference admission cannot bound a huge directory's metadata.
        let inventory_bytes = directory.entries.iter().try_fold(0u64, |sum, entry| {
            serde_json::to_vec(entry)
                .map(|json| {
                    sum.saturating_add(json.len() as u64)
                        .saturating_add(
                            (entry.path.as_os_str().as_encoded_bytes().len() as u64)
                                .saturating_mul(3),
                        )
                        .saturating_add(512)
                })
                .map_err(|error| error.to_string())
        })?;
        if !self.admit(None, 0, inventory_bytes)? {
            return Ok(None);
        }
        Ok(Some(directory))
    }
    fn finish_directory_inventory(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        path: &Path,
    ) -> Result<(), String> {
        for unseen in self.database()?.unseen_children(id, generation, path)? {
            let excluded = self.index_excludes(&unseen, &root.path);
            // A policy change retires generated references even when the source
            // still exists. Ordinary removals continue to require absence proof.
            if excluded
                || reader::call(
                    &Request::Absent {
                        root: root.clone(),
                        path: unseen.clone(),
                    },
                    &self.stop,
                )
                .is_ok()
            {
                self.database()?.retire_path(id, &unseen)?;
            }
        }
        self.database()?.directory_complete(id, generation, path)
    }
    fn observe_entry(
        &mut self,
        root: &SourceRoot,
        id: i64,
        generation: i64,
        entry: Entry,
        pipeline: &str,
    ) -> Result<(), String> {
        if self.work.len() >= 100_000 {
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
            });
        } else if let Some(object) = entry.identity.map(|id| id.object()) {
            let (file_id, pending) = self
                .database()?
                .observe_object(id, generation, &entry, object, pipeline)?;
            if pending {
                self.work.push_back(Work::File(Box::new(FileWork {
                    root: root.clone(),
                    root_id: id,
                    entry,
                    id: file_id,
                    cursor: 0,
                    prepared: None,
                    text_cursor: 0,
                    segments: Vec::new(),
                    coverage: "complete".into(),
                    source_check: SourceCheck::Pending,
                    published: 0,
                    resume_hash: None,
                })));
            }
        }
        Ok(())
    }
    fn index_file(&mut self, mut file: FileWork) -> Result<(), String> {
        if self.index_excludes(&file.entry.path, &file.root.path) {
            return self.database()?.retire_path(file.root_id, &file.entry.path);
        }
        if matches!(file.source_check, SourceCheck::Pending) {
            if let Err(error) = self.check_file_content(&mut file) {
                return self.file_failure(file, error);
            }
            self.resume_file(&mut file)?;
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
                    let pipeline = self.pipeline.clone();
                    self.database()?
                        .discard_checkpoint(file.id, file.entry.revision, &pipeline)?;
                    self.work.push_front(Work::File(Box::new(file)));
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
            self.checkpoint_file(&mut file)?;
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
    fn check_file_content(&mut self, file: &mut FileWork) -> Result<(), String> {
        let reply = reader::call(
            &Request::Fingerprint {
                root: file.root.clone(),
                entry: file.entry.clone(),
            },
            &self.stop,
        )?;
        let Reply::Fingerprint(digest) = reply else {
            return Err("Invalid source fingerprint reply".into());
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
        Ok(())
    }
    fn text_segment(&mut self, file: &mut FileWork) -> Result<Segment, String> {
        let prepared = file.prepared.as_ref().unwrap();
        let start = file.text_cursor;
        let mut limit = (start + 8192).min(prepared.text.len());
        while !prepared.text.is_char_boundary(limit) {
            limit -= 1;
        }
        let stop = self.stop.clone();
        let (end, next) = self
            .model(Role::Embedding, &stop)?
            .passage(&prepared.text[start..limit], &stop)?;
        let text = prepared.text[start..start + end].to_owned();
        file.text_cursor = if start + end == prepared.text.len() {
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
        let stop = self.stop.clone();
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
        let stop = self.stop.clone();
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
                    return self.file_failure(file, error);
                }
            }
        }
        if let Some(next) = prepared.next {
            file.cursor = next;
            file.text_cursor = 0;
            self.checkpoint_file(&mut file)?;
            self.work.push_front(Work::File(Box::new(file)));
            Ok(())
        } else {
            self.publish_file(file)
        }
    }
    fn file_failure(&mut self, mut file: FileWork, error: String) -> Result<(), String> {
        if !file.segments.is_empty() {
            file.coverage = format!(
                "partial: {}; retained {} completed segments; planned coverage: {}",
                error.chars().take(240).collect::<String>(),
                file.segments.len(),
                file.coverage
            );
            return self.publish_file(file);
        }
        self.database()?
            .connection
            .execute(
                "UPDATE files SET state='failed',error=?2 WHERE id=?1",
                params![file.id, error],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn publish_file(&mut self, file: FileWork) -> Result<(), String> {
        self.verify_index_file(&file)?;
        let pipeline = self.pipeline.clone();
        let published = self.database()?.publish(
            file.id,
            file.entry.revision,
            &pipeline,
            &file.segments,
            &file.coverage,
        )?;
        if published {
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
    fn read_prepared(&self, file: &mut FileWork) -> Result<Option<Prepared>, String> {
        let result = reader::call(
            &Request::Extract {
                root: file.root.clone(),
                entry: file.entry.clone(),
                cursor: file.cursor,
            },
            &self.stop,
        );
        match result? {
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
                    return Ok(None);
                }
                Ok(Some(prepared))
            }
            _ => Err("Invalid extraction reply".into()),
        }
    }
    fn verify_index_file(&self, file: &FileWork) -> Result<(), String> {
        reader::call(
            &Request::Verify {
                root: file.root.clone(),
                entry: file.entry.clone(),
            },
            &self.stop,
        )
        .map(|_| ())
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
    fn checkpoint_file(&mut self, file: &mut FileWork) -> Result<(), String> {
        if file.segments.len().saturating_sub(file.published) < 8 {
            return Ok(());
        }
        self.verify_index_file(file)?;
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
        Ok(())
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
        cancel: &AtomicBool,
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
    let mut verified = std::collections::HashSet::new();
    for root in roots {
        let entries = ranked
            .iter()
            .filter(|entry| entry.path.starts_with(&root.path))
            .cloned()
            .collect::<Vec<_>>();
        if entries.is_empty() {
            continue;
        }
        if let Ok(Reply::VerifiedBatch(indices)) = reader::call(
            &Request::VerifyBatch {
                root,
                entries: entries.clone(),
            },
            stop,
        ) {
            for index in indices {
                verified.insert(entries[index].path.clone());
            }
        }
    }
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
    use super::*;
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
                snapshot: Arc::new(Mutex::new(Snapshot::default())),
                stop: Arc::new(AtomicBool::new(false)),
                query_cancel: Mutex::new(None),
            },
            receiver,
        )
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
