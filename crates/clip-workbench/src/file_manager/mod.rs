//! Folder browsing and file management on the shared bounded executor.

mod app;
mod binary_strip;
mod blend_preview;
mod bookmarks;
mod content;
mod edit_ui;
mod edits;
mod filesystem;
mod font_strip;
mod gallery;
mod image_strip;
mod material_strip;
mod media;
mod midi_strip;
mod music;
mod native_drag;
mod pdf_strip;
mod preview_job;
mod previews;
mod scene_strip;
mod selection;
mod shelf;
mod sorting;
pub(crate) mod system;
mod system_route;
mod text_editor;
mod text_strip;
mod transfers;
mod video_timeline;
mod viewers;
pub use app::run;
#[cfg(test)]
mod tests;
mod tree;
mod type_navigation;
mod ui;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use filesystem::{Directory, display_path, parse_address, read_directory};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use tree::FolderTree;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum BrowserView {
    #[default]
    List,
    Photos,
    Music,
    Videos,
}

const VIDEO_ROW_HEIGHT: f32 = 108.0;

#[derive(Clone, Debug)]
struct Location {
    path: PathBuf,
    selected: Option<PathBuf>,
    selection: BTreeSet<PathBuf>,
    anchor: Option<PathBuf>,
    selection_undo: Option<selection::SelectionSnapshot>,
    scroll: f32,
    manual_view: Option<BrowserView>,
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
        Self::guarded(path, None, jobs)
    }

    fn guarded(
        path: PathBuf,
        target: Option<system::BrowserTarget>,
        jobs: &BackgroundJobs,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::from_secs(120), move || match target {
            Some(target) => target.read_directory(&token),
            None => read_directory(&path, &token),
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
    browser_target: Option<system::BrowserTarget>,
    job: Option<DirectoryJob>,
}

pub(crate) struct FileManager {
    home: PathBuf,
    history: Vec<Location>,
    index: Option<usize>,
    directory: Option<Directory>,
    visible: Vec<usize>,
    sort: sorting::Sort,
    pending: Option<Request>,
    error: Option<String>,
    failed: Option<(PathBuf, Navigation)>,
    address: String,
    show_hidden: bool,
    tree: FolderTree,
    bookmarks: bookmarks::Bookmarks,
    bookmark_drop_rect: Option<egui::Rect>,
    native_drag_point: Option<egui::Pos2>,
    restore_scroll: bool,
    reveal_selection: bool,
    type_navigation: type_navigation::TypeNavigation,
    reveal_target: Option<PathBuf>,
    focus_address: bool,
    address_editing: bool,
    edits: edits::EditState,
    selection_revision: u64,
    transfers: transfers::Transfers,
    refresh_after_transfer: bool,
    watch: Option<DirectoryJob>,
    watch_started: std::time::Instant,
    drop_targets: Vec<(egui::Rect, PathBuf)>,
    was_modal: bool,
    shelf: shelf::Shelf,
    open_file: Option<PathBuf>,
    inspect_file: Option<PathBuf>,
    text_start: Option<(PathBuf, usize)>,
    pdf_start: Option<(PathBuf, usize)>,
    video_start: Option<(PathBuf, f64)>,
    previews: previews::EntryPreviews,
    gallery_previews: previews::EntryPreviews,
    view: BrowserView,
    music_threshold: u8,
    video_timeline_decimation: u8,
    music_previews: previews::EntryPreviews,
    video_previews: video_timeline::Previews,
    video_count: usize,
    video_offsets: Vec<f32>,
    list_offsets: Vec<f32>,
    thumbnail_edge: f32,
    gallery_columns: usize,
    gallery_layout: gallery::Gallery,
    gallery_selecting: bool,
    music: music::Music,
    midi: midi_strip::Preview,
    audio_entries: Vec<PathBuf>,
    music_offsets: Vec<f32>,
    music_selection: Option<ui::music::MusicRange>,
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
            bookmarks: bookmarks::Bookmarks::new(home.join(".config/gtk-3.0/bookmarks")),
            bookmark_drop_rect: None,
            native_drag_point: None,
            home,
            history: Vec::new(),
            index: None,
            directory: None,
            visible: Vec::new(),
            sort: sorting::Sort::default(),
            pending: None,
            error: None,
            failed: None,
            address: String::new(),
            show_hidden: false,
            restore_scroll: false,
            reveal_selection: false,
            type_navigation: type_navigation::TypeNavigation::default(),
            reveal_target: None,
            focus_address: false,
            address_editing: false,
            edits: edits::EditState::default(),
            selection_revision: 0,
            transfers: transfers::Transfers::default(),
            refresh_after_transfer: false,
            watch: None,
            watch_started: std::time::Instant::now(),
            drop_targets: Vec::new(),
            was_modal: false,
            shelf: shelf::Shelf::default(),
            open_file: None,
            inspect_file: None,
            text_start: None,
            pdf_start: None,
            video_start: None,
            previews: previews::EntryPreviews::default(),
            gallery_previews: previews::EntryPreviews::default(),
            view: BrowserView::List,
            music_threshold: 75,
            video_timeline_decimation: 1,
            music_previews: previews::EntryPreviews::default(),
            video_previews: Default::default(),
            video_count: 0,
            video_offsets: vec![0.0],
            list_offsets: vec![0.0],
            thumbnail_edge: 160.0,
            gallery_columns: 1,
            gallery_layout: Default::default(),
            gallery_selecting: false,
            music: music::Music::default(),
            midi: midi_strip::Preview::default(),
            audio_entries: Vec::new(),
            music_offsets: vec![0.0],
            music_selection: None,
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

    fn stop_previews(&mut self) {
        self.music.stop();
        self.midi.stop();
    }
    fn navigate(&mut self, path: PathBuf, navigation: Navigation) {
        self.type_navigation.clear();
        self.reveal_target = None;
        if path != self.path() {
            self.gallery_layout.retire();
            self.video_previews.clear();
            self.video_start = None;
            self.music_selection = None;
            self.midi.selection = None;
            self.stop_previews();
        }
        self.watch = None;
        self.watch_started = std::time::Instant::now();
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
            browser_target: None,
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
        self.shelf.tick();
        self.bookmarks.tick(jobs);
        if let Some(text) = self.bookmarks.report.take() {
            self.edits.report = Some(edits::EditReport { text, failed: true });
        }
        if self.directory.is_none() && self.pending.is_none() && self.error.is_none() {
            self.navigate(self.home.clone(), Navigation::New);
        }
        let mut completion = None;
        if let Some(request) = &mut self.pending {
            let job = request.job.get_or_insert_with(|| {
                DirectoryJob::guarded(request.path.clone(), request.browser_target.clone(), jobs)
            });
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
                    self.reveal_target = None;
                    // Retrying a rejected inspector relationship must not turn it
                    // into an unguarded path navigation to the replacement.
                    self.failed = request
                        .browser_target
                        .is_none()
                        .then_some((request.path, request.navigation));
                }
            }
            if !self.address_editing {
                self.address = display_path(self.path());
            }
        }
        self.tick_edits(jobs);
        self.tick_transfers(jobs);
        if self.refresh_after_transfer && self.pending.is_none() {
            self.refresh_after_transfer = false;
            self.refresh();
        }
        self.tick_watch(jobs);
        self.tree.tick(jobs);
    }

    fn append_location(&mut self, path: PathBuf) {
        self.history.push(Location {
            path,
            selected: None,
            selection: BTreeSet::new(),
            anchor: None,
            selection_undo: None,
            scroll: 0.0,
            manual_view: None,
        });
        self.index = Some(self.history.len() - 1);
    }

    fn commit(&mut self, directory: Directory, navigation: Navigation) {
        if self
            .directory
            .as_ref()
            .is_some_and(|previous| previous.path != directory.path)
        {
            self.previews.retain(&Default::default());
        }
        if !matches!(navigation, Navigation::Refresh) {
            self.selection_revision = self.selection_revision.wrapping_add(1);
        }
        match navigation {
            _ if self.index.is_none() => {
                self.append_location(directory.path.clone());
            }
            Navigation::New if directory.path != self.path() => {
                self.history.truncate(self.index.map_or(0, |i| i + 1));
                self.append_location(directory.path.clone());
            }
            Navigation::History(index) => self.index = Some(index),
            _ => {}
        }
        self.tree.reveal(&directory.path);
        self.directory = Some(directory);
        self.rebuild_visible();
        if let Some(path) = self.reveal_target.take() {
            // Directory reads canonicalize the parent; keep reveal paths in that
            // same namespace when the inspector followed a directory alias.
            if let Some(name) = path.file_name() {
                self.reveal_path(self.path().join(name));
            }
        }
        self.restore_scroll = true;
    }

    fn browse_system_target(&mut self, target: system::BrowserTarget) {
        // Even the current folder must pass the asynchronous relationship guard.
        self.navigate(target.folder.clone(), Navigation::Refresh);
        if let Some(request) = &mut self.pending {
            request.navigation = Navigation::New;
            self.reveal_target = target.reveal.clone();
            request.browser_target = Some(target);
        }
    }

    fn reveal_path(&mut self, path: PathBuf) {
        let Some(entry) = self
            .directory
            .as_ref()
            .and_then(|directory| directory.entries.iter().find(|entry| entry.path == path))
        else {
            return;
        };
        if entry.hidden && !self.show_hidden {
            self.show_hidden = true;
            self.rebuild_visible();
        }
        self.select(path);
        self.reveal_selection = true;
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
        if let Some(directory) = &self.directory {
            self.sort.apply(directory, &mut self.visible);
        }
        self.reconcile_selection();
        self.audio_entries.clear();
        self.video_count = 0;
        self.video_offsets.clear();
        self.video_offsets.push(0.0);
        self.music_offsets.clear();
        self.music_offsets.push(0.0);
        if let Some(directory) = &self.directory {
            for index in &self.visible {
                let entry = &directory.entries[*index];
                let audio =
                    entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path);
                let video = entry.is_video();
                self.video_count += usize::from(video);
                self.video_offsets.push(
                    self.video_offsets.last().copied().unwrap_or(0.0)
                        + if video { VIDEO_ROW_HEIGHT } else { 28.0 },
                );
                if audio {
                    self.audio_entries.push(entry.path.clone());
                }
                self.music_offsets.push(
                    self.music_offsets.last().copied().unwrap_or(0.0)
                        + if audio { 108.0 } else { 28.0 },
                );
            }
        }
        self.activate_media_view();
        self.rebuild_list_offsets();
    }

    fn rebuild_list_offsets(&mut self) {
        self.list_offsets.clear();
        self.list_offsets.push(0.0);
        if let Some(directory) = &self.directory {
            for index in &self.visible {
                let entry = &directory.entries[*index];
                let height = if entry.system.is_some()
                    || self.previews.is_text(entry)
                    || (entry.regular_file && font_strip::known(&entry.path))
                    || entry.is_playable_media()
                    || entry.is_scene()
                    || entry.is_blend()
                    || entry.is_material()
                    || entry.is_photo()
                    || (entry.regular_file && little_clip_core::pdf::is_pdf_path(&entry.path))
                    || (entry.regular_file
                        && little_clip_core::document::is_office_document(&entry.path))
                {
                    VIDEO_ROW_HEIGHT
                } else {
                    28.0
                };
                self.list_offsets
                    .push(self.list_offsets.last().copied().unwrap_or(0.0) + height);
            }
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.rebuild_visible();
    }

    fn move_selection(&mut self, down: bool, extend: bool) {
        let stride = if self.view == BrowserView::Photos {
            self.gallery_columns
        } else {
            1
        } as isize;
        self.move_selection_by(if down { stride } else { -stride }, extend);
    }

    fn move_selection_by(&mut self, delta: isize, extend: bool) {
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
        let index = match index {
            Some(i) => i.saturating_add_signed(delta).min(self.visible.len() - 1),
            None if delta >= 0 => 0,
            None => self.visible.len() - 1,
        };
        self.select_with(
            directory.entries[self.visible[index]].path.clone(),
            false,
            extend,
        );
        self.reveal_selection = true;
    }

    fn open_selected(&mut self) {
        if self.selection_count() != 1 {
            return;
        }
        let selected = self
            .location()
            .and_then(|location| location.selection.first());
        let path = self
            .directory
            .as_ref()
            .and_then(|directory| {
                directory
                    .entries
                    .iter()
                    .find(|entry| Some(&entry.path) == selected)
            })
            .map(|entry| entry.path.clone());
        if let Some(path) = path {
            self.open_path(path);
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        self.type_navigation.clear();
        if self.directory.as_ref().is_some_and(|dir| {
            dir.entries
                .iter()
                .any(|entry| entry.path == path && entry.directory)
        }) {
            self.navigate(path, Navigation::New);
        } else {
            self.open_file = Some(path);
        }
    }
}

impl FileManager {
    fn tick_watch(&mut self, jobs: &BackgroundJobs) {
        if self.pending.is_some() || self.directory.is_none() {
            return;
        }
        if self.watch.is_none() && self.watch_started.elapsed() >= Duration::from_secs(3) {
            self.watch = Some(DirectoryJob::new(self.path().into(), jobs));
            self.watch_started = std::time::Instant::now();
        }
        let Some(completion) = self.watch.as_ref().and_then(DirectoryJob::poll) else {
            return;
        };
        self.watch = None;
        match completion {
            Completion::Ready(Ok(directory)) if directory.path == self.path() => {
                if self
                    .directory
                    .as_ref()
                    .is_none_or(|current| current.signature != directory.signature)
                {
                    self.tree.refresh(directory.path.clone());
                    self.commit(directory, Navigation::Refresh);
                }
            }
            Completion::Ready(Err(_)) => {
                self.refresh();
            }
            _ => {}
        }
    }
}

// Flatten executor errors consistently for file-manager background requests.
fn poll_job<T>(
    receiver: &mpsc::Receiver<Result<Result<T, String>, JobError>>,
) -> Option<Result<T, String>> {
    match receiver.try_recv() {
        Ok(Ok(result)) => Some(result),
        Ok(Err(error)) => Some(Err(error.to_string())),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(_) => Some(Err("Background worker disconnected".into())),
    }
}
