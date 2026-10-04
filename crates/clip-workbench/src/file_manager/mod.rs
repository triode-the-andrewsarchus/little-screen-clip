//! Read-only folder browsing. Filesystem work stays on the shared bounded executor.

mod app;
mod filesystem;
pub use app::run;
#[cfg(test)]
mod tests;
mod tree;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use filesystem::{Directory, display_path, parse_address, read_directory};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use tree::FolderTree;

#[derive(Clone, Debug)]
struct Location {
    path: PathBuf,
    selected: Option<PathBuf>,
    scroll: f32,
}

#[derive(Clone, Copy, Debug)]
enum Navigation {
    New,
    History(usize),
    Refresh,
}

struct DirectoryJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<Result<Directory, String>, JobError>>,
}

enum Completion {
    Ready(Result<Directory, String>),
    Retry,
}

impl DirectoryJob {
    fn new(path: PathBuf, jobs: &BackgroundJobs) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::from_secs(120), move || {
            read_directory(&path, &token)
        });
        Self {
            ticket,
            cancelled,
            receiver,
        }
    }

    // Queue saturation retries through the caller's pending request; filesystem
    // errors are terminal. Dropping old receivers rejects already-published results.
    fn poll(&self) -> Option<Completion> {
        match self.receiver.try_recv() {
            Ok(Ok(result)) => Some(Completion::Ready(result)),
            Ok(Err(JobError::QueueFull)) => Some(Completion::Retry),
            Ok(Err(error)) => Some(Completion::Ready(Err(error.to_string()))),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Completion::Ready(Err(
                "Directory worker disconnected".into(),
            ))),
        }
    }
}

impl Drop for DirectoryJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}

struct Request {
    path: PathBuf,
    navigation: Navigation,
    job: Option<DirectoryJob>,
}

pub(crate) struct FileManager {
    home: PathBuf,
    history: Vec<Location>,
    index: Option<usize>,
    directory: Option<Directory>,
    visible: Vec<usize>,
    pending: Option<Request>,
    error: Option<String>,
    failed: Option<(PathBuf, Navigation)>,
    address: String,
    show_hidden: bool,
    tree: FolderTree,
    restore_scroll: bool,
    reveal_selection: bool,
    focus_address: bool,
    address_editing: bool,
}

impl Default for FileManager {
    fn default() -> Self {
        Self::new(
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/")),
        )
    }
}

impl FileManager {
    fn new(home: PathBuf) -> Self {
        Self {
            tree: FolderTree::new(home.clone()),
            home,
            history: Vec::new(),
            index: None,
            directory: None,
            visible: Vec::new(),
            pending: None,
            error: None,
            failed: None,
            address: String::new(),
            show_hidden: false,
            restore_scroll: false,
            reveal_selection: false,
            focus_address: false,
            address_editing: false,
        }
    }

    fn location(&self) -> Option<&Location> {
        self.index.map(|i| &self.history[i])
    }

    fn location_mut(&mut self) -> Option<&mut Location> {
        self.index.map(|i| &mut self.history[i])
    }

    fn path(&self) -> &Path {
        self.location()
            .map_or(&self.home, |location| &location.path)
    }

    fn navigate(&mut self, path: PathBuf, navigation: Navigation) {
        if matches!(navigation, Navigation::New) && self.directory.is_some() && path == self.path()
        {
            self.pending = None;
            self.error = None;
            self.failed = None;
            self.address = display_path(self.path());
            return;
        }
        self.pending = Some(Request {
            path,
            navigation,
            job: None,
        });
        self.error = None;
        self.failed = None;
    }

    fn history_step(&mut self, forward: bool) {
        let Some(index) = self.index else {
            return;
        };
        let target = if forward {
            index.checked_add(1)
        } else {
            index.checked_sub(1)
        };
        if let Some(target) = target.filter(|target| *target < self.history.len()) {
            self.navigate(
                self.history[target].path.clone(),
                Navigation::History(target),
            );
        }
    }

    fn parent(&mut self) {
        if let Some(parent) = self.path().parent() {
            self.navigate(parent.into(), Navigation::New);
        }
    }

    fn submit_address(&mut self) {
        match parse_address(&self.address, self.path(), &self.home) {
            Ok(path) => self.navigate(path, Navigation::New),
            Err(error) => {
                self.pending = None;
                self.failed = None;
                self.error = Some(error);
            }
        }
    }

    fn refresh(&mut self) {
        self.tree.refresh(self.path().to_path_buf());
        self.navigate(self.path().to_path_buf(), Navigation::Refresh);
    }

    fn retry(&mut self) {
        if let Some((path, navigation)) = self.failed.take() {
            self.navigate(path, navigation);
        } else {
            self.refresh();
        }
    }

    fn tick(&mut self, jobs: &BackgroundJobs) {
        if self.directory.is_none() && self.pending.is_none() && self.error.is_none() {
            self.navigate(self.home.clone(), Navigation::New);
        }
        let mut completion = None;
        if let Some(request) = &mut self.pending {
            let job = request
                .job
                .get_or_insert_with(|| DirectoryJob::new(request.path.clone(), jobs));
            completion = job.poll();
        }
        if let Some(Completion::Retry) = completion {
            if let Some(request) = &mut self.pending {
                request.job = None;
            }
        } else if let Some(Completion::Ready(result)) = completion {
            let request = self.pending.take().expect("completion has a request");
            match result {
                Ok(directory) => self.commit(directory, request.navigation),
                Err(error) => {
                    self.error = Some(format!(
                        "Cannot open {}: {error}",
                        display_path(&request.path)
                    ));
                    self.failed = Some((request.path, request.navigation));
                }
            }
            if !self.address_editing {
                self.address = display_path(self.path());
            }
        }
        self.tree.tick(jobs);
    }

    fn commit(&mut self, directory: Directory, navigation: Navigation) {
        match navigation {
            _ if self.index.is_none() => {
                self.history.push(Location {
                    path: directory.path.clone(),
                    selected: None,
                    scroll: 0.0,
                });
                self.index = Some(0);
            }
            Navigation::New if directory.path != self.path() => {
                self.history.truncate(self.index.map_or(0, |i| i + 1));
                self.history.push(Location {
                    path: directory.path.clone(),
                    selected: None,
                    scroll: 0.0,
                });
                self.index = Some(self.history.len() - 1);
            }
            Navigation::History(index) => self.index = Some(index),
            _ => {}
        }
        self.tree.reveal(&directory.path);
        self.directory = Some(directory);
        self.rebuild_visible();
        self.restore_scroll = true;
    }

    fn rebuild_visible(&mut self) {
        self.visible = self
            .directory
            .as_ref()
            .map(|directory| {
                directory
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| self.show_hidden || !entry.hidden)
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        let exists = self
            .location()
            .and_then(|location| location.selected.as_ref())
            .is_some_and(|selected| {
                self.directory.as_ref().is_some_and(|directory| {
                    self.visible
                        .iter()
                        .any(|i| directory.entries[*i].path == *selected)
                })
            });
        if !exists && let Some(location) = self.location_mut() {
            location.selected = None;
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild_visible();
    }

    fn select(&mut self, path: PathBuf) {
        if let Some(location) = self.location_mut() {
            location.selected = Some(path);
        }
    }

    fn move_selection(&mut self, down: bool) {
        if self.visible.is_empty() {
            return;
        }
        let directory = self
            .directory
            .as_ref()
            .expect("visible entries have a directory");
        let selected = self
            .location()
            .and_then(|location| location.selected.as_ref());
        let index = self
            .visible
            .iter()
            .position(|i| Some(&directory.entries[*i].path) == selected);
        let index = match (index, down) {
            (Some(i), true) => (i + 1).min(self.visible.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
            (None, true) => 0,
            (None, false) => self.visible.len() - 1,
        };
        self.select(directory.entries[self.visible[index]].path.clone());
        self.reveal_selection = true;
    }

    fn open_selected(&mut self) {
        let selected = self
            .location()
            .and_then(|location| location.selected.as_ref());
        let path = self
            .directory
            .as_ref()
            .and_then(|directory| {
                directory
                    .entries
                    .iter()
                    .find(|entry| entry.directory && Some(&entry.path) == selected)
            })
            .map(|entry| entry.path.clone());
        if let Some(path) = path {
            self.navigate(path, Navigation::New);
        }
    }
}
