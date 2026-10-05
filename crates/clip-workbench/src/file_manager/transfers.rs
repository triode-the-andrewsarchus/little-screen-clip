//! Serialized batches, cooperative cancellation, and explicit conflict decisions.

pub(super) mod clipboard;
mod copy;
#[cfg(test)]
mod tests;
mod trash_support;
pub(super) mod ui;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use super::{BackgroundJobs, FileManager, JobError};

#[derive(Clone, Debug)]
pub(super) enum Kind {
    Copy(PathBuf),
    Move(PathBuf),
    Trash,
    Export(bool),
    ConsumeCut(Vec<PathBuf>, Vec<PathBuf>),
    Delete,
    NativeMoveCleanup(BTreeMap<PathBuf, super::filesystem::Identity>),
    Restore(Vec<trash::TrashItem>),
}

impl Kind {
    fn title(&self) -> &'static str {
        match self {
            Self::Copy(_) => "Copy",
            Self::Move(_) => "Move",
            Self::Trash => "Trash",
            Self::Export(_) | Self::ConsumeCut(_, _) => "Desktop clipboard",
            Self::Delete => "Permanent deletion",
            Self::NativeMoveCleanup(_) => "Move",
            Self::Restore(_) => "Restore",
        }
    }
}

#[derive(Clone)]
struct Batch {
    kind: Kind,
    sources: Vec<PathBuf>,
    expected: Option<BTreeMap<PathBuf, Option<super::filesystem::Identity>>>,
    native_drop: Option<u64>,
}

#[derive(Clone, Debug)]
pub(super) enum Decision {
    Replace,
    Skip,
    Name(std::ffi::OsString),
}

#[derive(Clone)]
pub(super) struct Conflict {
    source: PathBuf,
    destination: PathBuf,
}

#[derive(Default)]
pub(super) struct Progress {
    title: String,
    current: PathBuf,
    done: usize,
    total: usize,
    bytes: u64,
    conflict: Option<Conflict>,
    decision: Option<(Decision, bool)>,
}

#[derive(Default)]
struct Control {
    cancelled: AtomicBool,
    progress: Mutex<Progress>,
    decision_ready: Condvar,
}

impl Control {
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err("Cancelled".into())
        } else {
            Ok(())
        }
    }

    fn conflict(&self, source: &Path, destination: &Path) -> Result<(Decision, bool), String> {
        let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
        progress.conflict = Some(Conflict {
            source: source.into(),
            destination: destination.into(),
        });
        loop {
            self.check()?;
            if let Some(decision) = progress.decision.take() {
                progress.conflict = None;
                return Ok(decision);
            }
            progress = self
                .decision_ready
                .wait_timeout(progress, Duration::from_millis(100))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

#[derive(Default)]
pub(super) struct Report {
    title: String,
    completed: Vec<PathBuf>,
    skipped: Vec<PathBuf>,
    failed: Vec<(PathBuf, String)>,
    affected: BTreeSet<PathBuf>,
    cancelled: bool,
    unprocessed: usize,
    moved: Vec<(PathBuf, PathBuf)>,
    exported: bool,
    native_drop: Option<u64>,
}

struct Running {
    batch: Batch,
    control: Arc<Control>,
    receiver: Option<mpsc::Receiver<Result<Report, JobError>>>,
}

#[derive(Clone)]
pub(super) struct FileDrag {
    pub paths: Vec<PathBuf>,
}

#[derive(Clone)]
pub(super) struct FileClipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
    pending_export: bool,
}

pub(super) struct DeleteConfirmation {
    paths: Vec<PathBuf>,
    permanent: bool,
    expected: BTreeMap<PathBuf, Option<super::filesystem::Identity>>,
}

type ImportResult =
    mpsc::Receiver<Result<Result<super::content::Payload, clipboard::ReadError>, JobError>>;

#[derive(Default)]
pub(super) struct Transfers {
    queue: VecDeque<Batch>,
    pub(super) native_results: Vec<(u64, bool)>,
    running: Option<Running>,
    pub clipboard: Option<FileClipboard>,
    reports: VecDeque<Report>,
    confirmation: Option<DeleteConfirmation>,
    conflict_name: String,
    conflict_error: Option<String>,
    apply_all: bool,
    trash: trash_support::TrashBrowser,
    notice: Option<String>,
    import: Option<(PathBuf, ImportResult)>,
}

impl Transfers {
    pub(super) fn mutation_running(&self) -> bool {
        self.running.is_some()
    }
    pub(super) fn has_status(&self) -> bool {
        self.busy() || self.clipboard.is_some() || !self.reports.is_empty() || self.notice.is_some()
    }
    pub(super) fn needs_repaint(&self) -> bool {
        self.busy() || self.trash.pending.is_some() || self.import.is_some()
    }

    pub(super) fn busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty() || self.import.is_some()
    }

    fn enqueue(&mut self, kind: Kind, mut sources: Vec<PathBuf>) -> bool {
        if sources.is_empty() {
            return false;
        }
        if self.queue.len() >= 8 {
            self.notice = Some("The operation queue is full (8 waiting batches).".into());
            return false;
        }
        sources.sort();
        sources.dedup();
        let mut roots: Vec<PathBuf> = Vec::new();
        for path in sources {
            if roots.last().is_none_or(|parent| !path.starts_with(parent)) {
                roots.push(path);
            }
        }
        let sources = roots;
        self.queue.push_back(Batch {
            kind,
            sources,
            expected: None,
            native_drop: None,
        });
        true
    }

    pub(super) fn cancel_waiting(&mut self) {
        for batch in self.queue.drain(..) {
            if let Some(token) = batch.native_drop {
                self.native_results.push((token, false));
            }
            self.reports.push_front(Report {
                title: batch.kind.title().into(),
                cancelled: true,
                unprocessed: batch.sources.len(),
                ..Default::default()
            });
        }
        self.reports.truncate(10);
    }

    fn tick(&mut self, jobs: &BackgroundJobs, naming_busy: bool) -> Option<Report> {
        self.trash.tick(jobs);
        if self.running.is_none()
            && !naming_busy
            && let Some(batch) = self.queue.pop_front()
        {
            let control = Arc::new(Control::default());
            *control.progress.lock().unwrap_or_else(|e| e.into_inner()) = Progress {
                title: batch.kind.title().into(),
                total: batch.sources.len(),
                ..Default::default()
            };
            self.running = Some(Running {
                batch,
                control,
                receiver: None,
            });
        }
        let running = self.running.as_mut()?;
        if running.control.cancelled.load(Ordering::Relaxed) && running.receiver.is_none() {
            let report = Report {
                title: running.batch.kind.title().into(),
                cancelled: true,
                unprocessed: running.batch.sources.len(),
                native_drop: running.batch.native_drop,
                ..Default::default()
            };
            self.running = None;
            return Some(report);
        }
        if running.receiver.is_none() {
            let batch = running.batch.clone();
            let control = Arc::clone(&running.control);
            let (_, receiver) = jobs.submit(Duration::MAX, move || batch.execute(&control));
            running.receiver = Some(receiver);
        }
        match running
            .receiver
            .as_ref()
            .expect("submitted transfer")
            .try_recv()
        {
            Ok(Err(JobError::QueueFull)) => {
                running.receiver = None;
                None
            }
            Ok(Ok(report)) => {
                self.running = None;
                Some(report)
            }
            Ok(Err(error)) => self.unknown_completion(error.to_string()),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.unknown_completion("Worker disconnected".into())
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }

    fn unknown_completion(&mut self, message: String) -> Option<Report> {
        let running = self.running.take()?;
        let mut report = Report {
            title: running.batch.kind.title().into(),
            native_drop: running.batch.native_drop,
            ..Default::default()
        };
        for path in running.batch.sources {
            if let Some(parent) = path.parent() {
                report.affected.insert(parent.into());
            }
            report.failed.push((
                path,
                format!("Completion unknown: {message}; inspect before retrying"),
            ));
        }
        Some(report)
    }
}

impl FileManager {
    pub(super) fn filesystem_busy(&self) -> bool {
        self.edit_busy() || self.transfers.busy()
    }

    pub(super) fn tick_transfers(&mut self, jobs: &BackgroundJobs) {
        self.tick_clipboard_import();
        let Some(report) = self.transfers.tick(jobs, self.edit_busy()) else {
            return;
        };
        if let Some(token) = report.native_drop {
            self.transfers.native_results.push((
                token,
                report.failed.is_empty()
                    && report.skipped.is_empty()
                    && !report.cancelled
                    && report.unprocessed == 0,
            ));
        }
        if report.exported
            && let Some(clipboard) = &mut self.transfers.clipboard
            && clipboard.paths == report.completed
        {
            clipboard.pending_export = false;
        }
        for (source, destination) in &report.moved {
            self.follow_rename(source, destination);
        }
        for parent in &report.affected {
            self.tree.refresh(parent.clone());
        }
        if report.affected.contains(self.path()) {
            self.refresh_after_transfer = true;
        }
        self.consume_moved_references(&report);
        if report.title == "Move" {
            self.shelf.consume_moves(&report.completed);
        }
        self.transfers.trash.invalidate();
        self.transfers.reports.push_front(report);
        self.transfers.reports.truncate(10);
        self.transfers.conflict_name.clear();
        self.transfers.conflict_error = None;
    }

    fn consume_moved_references(&mut self, report: &Report) {
        if report.title != "Move" {
            return;
        }
        let Some(clipboard) = &mut self.transfers.clipboard else {
            return;
        };
        if !clipboard.cut {
            return;
        }
        let previous = clipboard.paths.clone();
        clipboard.paths.retain(|path| {
            !report
                .completed
                .iter()
                .any(|completed| path.starts_with(completed))
        });
        let remaining = clipboard.paths.clone();
        if previous != remaining {
            if remaining.is_empty() {
                self.transfers.clipboard = None;
            }
            self.transfers
                .enqueue(Kind::ConsumeCut(previous.clone(), remaining), previous);
        }
    }

    pub(super) fn selected_paths(&self) -> Vec<PathBuf> {
        self.location()
            .map(|location| location.selection.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub(super) fn copy_selection(&mut self, cut: bool) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            self.transfers.clipboard = Some(FileClipboard {
                paths: paths.clone(),
                cut,
                pending_export: true,
            });
            self.transfers.enqueue(Kind::Export(cut), paths);
        }
    }

    pub(super) fn paste_files(&mut self) {
        let consuming = self
            .transfers
            .queue
            .iter()
            .any(|batch| matches!(batch.kind, Kind::ConsumeCut(_, _)))
            || self
                .transfers
                .running
                .as_ref()
                .is_some_and(|running| matches!(running.batch.kind, Kind::ConsumeCut(_, _)));
        if consuming {
            if let Some(clipboard) = self.transfers.clipboard.clone() {
                self.queue_transfer(clipboard.paths, self.path().into(), clipboard.cut);
            }
            return;
        }
        let publishing = self
            .transfers
            .queue
            .iter()
            .any(|batch| matches!(batch.kind, Kind::Export(_)))
            || self
                .transfers
                .running
                .as_ref()
                .is_some_and(|running| matches!(running.batch.kind, Kind::Export(_)));
        if (publishing
            || self
                .transfers
                .clipboard
                .as_ref()
                .is_some_and(|clipboard| clipboard.pending_export))
            && let Some(clipboard) = self.transfers.clipboard.clone()
        {
            self.queue_transfer(clipboard.paths, self.path().into(), clipboard.cut);
        } else {
            self.import_desktop_files();
        }
    }

    pub(super) fn import_desktop_files(&mut self) {
        if self.transfers.import.is_some() {
            return;
        }
        let (_, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(15), clipboard::read_payload);
        self.transfers.import = Some((self.path().into(), receiver));
    }

    fn tick_clipboard_import(&mut self) {
        let Some((_, receiver)) = &self.transfers.import else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(clipboard::ReadError::Unavailable(error.to_string())),
            Err(mpsc::TryRecvError::Disconnected) => Err(clipboard::ReadError::Unavailable(
                "Clipboard reader disconnected".into(),
            )),
            Err(mpsc::TryRecvError::Empty) => return,
        };
        let (destination, _) = self.transfers.import.take().expect("clipboard completion");
        match result {
            Ok(super::content::Payload::Files { paths, cut }) => {
                self.transfers.clipboard = Some(FileClipboard {
                    paths: paths.clone(),
                    cut,
                    pending_export: false,
                });
                self.queue_transfer(paths, destination, cut);
            }
            Ok(super::content::Payload::Content(content)) => {
                self.name_content(content, destination);
            }
            Err(error) => {
                self.transfers.notice = Some(format!("Desktop clipboard: {error}"));
                if matches!(error, clipboard::ReadError::Unavailable(_))
                    && let Some(clipboard) = self.transfers.clipboard.clone()
                {
                    self.queue_transfer(clipboard.paths, destination, clipboard.cut);
                }
            }
        }
    }

    pub(super) fn queue_transfer(
        &mut self,
        sources: Vec<PathBuf>,
        destination: PathBuf,
        move_files: bool,
    ) {
        if self.directory.is_none() {
            return;
        }
        let kind = if move_files {
            Kind::Move(destination)
        } else {
            Kind::Copy(destination)
        };
        self.transfers.enqueue(kind, sources);
    }

    pub(super) fn confirm_delete(&mut self, permanent: bool) {
        let paths = self.selected_paths();
        if !paths.is_empty() {
            let selected: BTreeSet<_> = paths.iter().collect();
            let expected = self
                .directory
                .as_ref()
                .map(|directory| {
                    directory
                        .entries
                        .iter()
                        .filter(|entry| selected.contains(&entry.path))
                        .map(|entry| (entry.path.clone(), entry.identity))
                        .collect()
                })
                .unwrap_or_default();
            self.transfers.confirmation = Some(DeleteConfirmation {
                paths,
                permanent,
                expected,
            });
        }
    }
}

impl Batch {
    fn execute(&self, control: &Control) -> Report {
        let mut report = Report {
            title: self.kind.title().into(),
            native_drop: self.native_drop,
            ..Default::default()
        };
        if control.check().is_err() {
            report.cancelled = true;
            report.unprocessed = self.sources.len();
            return report;
        }
        if let Kind::Export(cut) = self.kind {
            match clipboard::write(&self.sources, cut) {
                Ok(()) => {
                    report.completed = self.sources.clone();
                    report.exported = true;
                }
                Err(error) => report
                    .failed
                    .push((PathBuf::from("desktop clipboard"), error)),
            }
            return report;
        }
        if let Kind::ConsumeCut(previous, remaining) = &self.kind {
            if let Err(error) = clipboard::consume_cut(previous, remaining) {
                report
                    .failed
                    .push((PathBuf::from("desktop clipboard"), error));
            }
            return report;
        }
        let mut policy = None;
        for (index, source) in self.sources.iter().enumerate() {
            if control.check().is_err() {
                report.cancelled = true;
                report.unprocessed = self.sources.len() - index;
                break;
            }
            control
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .current = source.clone();
            if let Some(parent) = source.parent() {
                report.affected.insert(parent.into());
            }
            match self.item(source, control, &mut policy, &mut report) {
                Ok(true) => report.completed.push(source.clone()),
                Ok(false) => report.skipped.push(source.clone()),
                Err(error) => report.failed.push((source.clone(), error)),
            }
            control
                .progress
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .done = index + 1;
        }
        report.cancelled |= control.cancelled.load(Ordering::Relaxed);
        report
    }

    // Returns true when a successful external move has already removed the source.
    fn validate_removal(&self, source: &Path) -> Result<bool, String> {
        if matches!(self.kind, Kind::NativeMoveCleanup(_))
            && std::fs::symlink_metadata(source)
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(true);
        }
        if matches!(
            self.kind,
            Kind::Trash | Kind::Delete | Kind::NativeMoveCleanup(_)
        ) {
            let expected = self
                .expected
                .as_ref()
                .and_then(|entries| entries.get(source))
                .and_then(|identity| *identity)
                .ok_or("Entry identity unavailable; refresh before removing it")?;
            let metadata = std::fs::symlink_metadata(source).map_err(|e| e.to_string())?;
            if super::filesystem::identity(&metadata) != expected {
                return Err("Entry changed after confirmation; refresh and select it again".into());
            }
        }
        Ok(false)
    }

    fn item(
        &self,
        source: &Path,
        control: &Control,
        policy: &mut Option<Decision>,
        report: &mut Report,
    ) -> Result<bool, String> {
        if self.validate_removal(source)? {
            return Ok(true);
        }
        match &self.kind {
            Kind::Copy(destination) | Kind::Move(destination) => {
                report.affected.insert(destination.clone());
                let target = copy::transfer(
                    source,
                    destination,
                    matches!(self.kind, Kind::Move(_)),
                    control,
                    policy,
                )?;
                if let Some(target) = target {
                    if matches!(self.kind, Kind::Move(_)) {
                        report.moved.push((source.into(), target));
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Kind::Export(_) | Kind::ConsumeCut(_, _) => Ok(false),
            Kind::Trash => {
                trash::delete(source).map_err(|e| e.to_string())?;
                Ok(true)
            }
            Kind::NativeMoveCleanup(snapshot) => {
                let entries = snapshot
                    .iter()
                    .filter(|(path, _)| path.starts_with(source))
                    .map(|(path, identity)| (path.clone(), *identity))
                    .collect();
                copy::remove_verified_source(source, &entries, control)?;
                Ok(true)
            }
            Kind::Delete => {
                copy::remove(source).map_err(|e| e.to_string())?;
                Ok(true)
            }
            Kind::Restore(items) => {
                let item = items
                    .iter()
                    .find(|item| Path::new(&item.id) == source)
                    .ok_or("Trash entry is unavailable")?;
                report.affected.insert(item.original_parent.clone());
                trash_support::restore(item)?;
                Ok(true)
            }
        }
    }
}

impl FileManager {
    pub(super) fn queue_native_drop(
        &mut self,
        token: u64,
        paths: Vec<PathBuf>,
        destination: PathBuf,
        moving: bool,
    ) -> bool {
        let kind = if moving {
            Kind::Move(destination)
        } else {
            Kind::Copy(destination)
        };
        if !self.transfers.enqueue(kind, paths) {
            return false;
        }
        self.transfers
            .queue
            .back_mut()
            .expect("enqueued drop")
            .native_drop = Some(token);
        true
    }
    pub(super) fn native_move_cleanup(
        &mut self,
        expected: BTreeMap<PathBuf, Option<super::filesystem::Identity>>,
        snapshot: BTreeMap<PathBuf, super::filesystem::Identity>,
    ) {
        if self.transfers.enqueue(
            Kind::NativeMoveCleanup(snapshot),
            expected.keys().cloned().collect(),
        ) {
            self.transfers
                .queue
                .back_mut()
                .expect("enqueued cleanup")
                .expected = Some(expected);
        }
    }
}
