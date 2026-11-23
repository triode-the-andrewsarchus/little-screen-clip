//! Folder browsing and file management on the shared bounded executor.

mod address;
mod app;
mod archive_browse;
mod binary_strip;
mod blend_preview;
mod bookmarks;
mod brush_strip;
mod calendar;
pub(super) mod constellation;
mod container_preview;
mod content;
mod database;
mod diagram_strip;
use little_clip_core::file_format as document_format;
mod ebook;
mod edit_ui;
mod edits;
mod embroidery;
mod external;
mod filesystem;
mod font_strip;
pub(crate) mod fuse_binwalk;
mod gallery;
mod gcode;
mod geography;
pub(crate) mod geography_downloader;
mod image_strip;
mod mail;
mod markdown_hover;
mod material_strip;
mod media;
mod midi_input;
mod midi_strip;
mod minecraft;
pub(crate) mod mounts;
mod multisample;
mod music;
mod native_drag;
mod notebook;
use little_clip_core::path_serde;
mod pdf_strip;
mod photo_ratings;
use little_clip_core::jobs::task as preview_job;
mod can_view;
mod capture_browse;
mod engineering_compare;
mod flowgraph_view;
mod iq_view;
mod logic_view;
mod metadata;
mod packets_view;
mod preview_copy;
mod previews;
mod pulse_view;
mod radio_view;
mod scene_strip;
mod scope_view;
pub(crate) mod search;
mod selection;
mod shelf;
mod sorting;
mod spreadsheet_strip;
mod subtitle_strip;
pub(crate) mod system;
mod system_route;
mod tags_view;
mod text_editor;
mod text_strip;
mod touchstone_view;
mod transfers;
mod video_playback;
mod video_timeline;
mod view_preferences;
mod viewers;
pub use app::{run, run_file, run_location};
#[cfg(test)]
mod tests;
mod tree;
mod type_navigation;
mod ui;
mod vcv_strip;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use filesystem::{Directory, display_path, parse_address, read_directory};
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use tree::FolderTree;

use crate::icons::Icon;

pub(crate) const STATUS_MIN_DURATION: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastStatus {
    pub icon: Icon,
    pub text: String,
    pub is_error: bool,
    pub timestamp: std::time::Instant,
}

impl LastStatus {
    pub fn new(icon: Icon, text: impl Into<String>, is_error: bool) -> Self {
        Self {
            icon,
            text: text.into(),
            is_error,
            timestamp: std::time::Instant::now(),
        }
    }

    pub fn is_active(&self, duration: Duration) -> bool {
        self.timestamp.elapsed() < duration
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DocumentPosition {
    VideoAudioTrack,
    VideoAudioRange {
        start_ms: u64,
        end_ms: u64,
        revision: u64,
    },
    ArchiveMember(usize, u64),
    TextLine(usize),
    PdfPage(usize),
    SpreadsheetSheet(usize),
    NotebookCell(usize),
    BookChapter(usize),
    MailAttachment(usize),
    CalendarEvent(usize),
    DatabaseTable(usize),
    GeographicFeature(usize),
    MinecraftMap,
    ToolpathLine(usize),
    SampleRegion(usize, usize),
    SubtitleCue(usize),
    ScopeSamples {
        first: u64,
        end: u64,
        revision: u64,
    },
    IqSamples {
        first: u64,
        end: u64,
        revision: u64,
        pair_revision: Option<u64>,
    },
    RawIq {
        revision: u64,
    },
    FlowgraphBlock {
        ordinal: u64,
        revision: u64,
    },
    RadioChannel {
        ordinal: usize,
        revision: u64,
    },
    TagRecord {
        ordinal: u64,
        revision: u64,
    },
    PulseSignal {
        ordinal: usize,
        revision: u64,
    },
    CanFrame {
        ordinal: u64,
        revision: u64,
    },
    CapturedPacket {
        ordinal: u64,
        revision: u64,
    },
    LogicRegion {
        start_bits: u64,
        end_bits: u64,
        revision: u64,
    },
    TouchstoneBand {
        start_hz_bits: u64,
        end_hz_bits: u64,
        revision: u64,
    },
}

impl DocumentPosition {
    fn source_revision(self) -> Option<u64> {
        match self {
            Self::ArchiveMember(_, revision)
            | Self::VideoAudioRange { revision, .. }
            | Self::ScopeSamples { revision, .. }
            | Self::TouchstoneBand { revision, .. }
            | Self::LogicRegion { revision, .. }
            | Self::CapturedPacket { revision, .. }
            | Self::CanFrame { revision, .. }
            | Self::PulseSignal { revision, .. }
            | Self::TagRecord { revision, .. }
            | Self::RadioChannel { revision, .. }
            | Self::FlowgraphBlock { revision, .. }
            | Self::IqSamples { revision, .. }
            | Self::RawIq { revision } => Some(revision),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::file_manager) enum BrowserView {
    #[default]
    #[serde(alias = "music", alias = "videos")]
    List,
    Columns,
    Photos,
    Constellation,
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
        Self::guarded(path, None, None, jobs)
    }

    fn guarded(
        path: PathBuf,
        target: Option<system::BrowserTarget>,
        search: Option<search::SourceGuard>,
        jobs: &BackgroundJobs,
    ) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let token = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::from_secs(120), move || match target {
            Some(target) => target.read_directory(&token),
            None => match search {
                Some(guard) => guard.read_directory(&path, &token),
                None => read_directory(&path, &token),
            },
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
    search_target: Option<search::SourceGuard>,
    job: Option<DirectoryJob>,
}

pub(crate) struct FileManager {
    home: PathBuf,
    history: Vec<Location>,
    index: Option<usize>,
    directory: Option<Directory>,
    visible: Vec<usize>,
    single_family: Option<sorting::Category>,
    list_groups: Vec<(usize, sorting::Category)>,
    sort: sorting::Sort,
    metadata: metadata::Metadata,
    pending: Option<Request>,
    error: Option<String>,
    failed: Option<(PathBuf, Navigation)>,
    address: String,
    address_completion: address::Completion,
    show_hidden: bool,
    tree: FolderTree,
    containers: container_preview::Previews,
    archives: archive_browse::Archives,
    bookmarks: bookmarks::Bookmarks,
    bookmark_drop_rect: Option<egui::Rect>,
    native_drag_point: Option<egui::Pos2>,
    restore_scroll: bool,
    reveal_selection: bool,
    type_navigation: type_navigation::TypeNavigation,
    search: search::Search,
    reveal_target: Option<PathBuf>,
    focus_address: bool,
    // Retain a keyboard handoff until egui's focus lock has settled.
    focus_list: bool,
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
    compare_selection: bool,
    external_file: Option<PathBuf>,
    inspect_file: Option<PathBuf>,
    document_start: Option<(PathBuf, DocumentPosition)>,
    video_start: Option<(PathBuf, f64)>,
    previews: previews::EntryPreviews,
    gallery_previews: previews::EntryPreviews,
    view: BrowserView,
    view_epoch: u64,
    view_preferences: view_preferences::Preferences,
    video_timeline_decimation: u8,
    music_previews: previews::EntryPreviews,
    video_previews: video_timeline::Previews,
    list_offsets: Vec<f32>,
    thumbnail_edge: f32,
    gallery_columns: usize,
    gallery_layout: gallery::Gallery,
    gallery_selecting: bool,
    gallery_last_scroll_y: f32,
    gallery_last_scroll_time: f64,
    gallery_hover: gallery::GalleryHover,
    pub(in crate::file_manager) photo_mode: gallery::similarity::PhotoMode,
    pub(in crate::file_manager) photo_recurse: bool,
    pub(in crate::file_manager) photo_recursive_limit: usize,
    pub(in crate::file_manager) photo_recursive_depth: usize,
    pub(in crate::file_manager) photo_recursive_timeout_seconds: u64,
    pub(in crate::file_manager) similarity_canvas: gallery::similarity::SimilarityCanvas,
    music: music::Music,
    midi: midi_strip::Preview,
    audio_entries: Vec<PathBuf>,
    music_selection: Option<ui::music::MusicRange>,
    preview_copies: preview_copy::Copies,
    large_video_copy: Option<(video_timeline::selection::Clip, u64)>,
    fuse_mounts: fuse_binwalk::MountManager,
    mounts: mounts::Mounts,
    pub(crate) constellation: constellation::ConstellationState,
    pub(crate) last_status: Option<LastStatus>,
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
            containers: Default::default(),
            archives: Default::default(),
            bookmarks: bookmarks::Bookmarks::new(home.join(".config/gtk-3.0/bookmarks")),
            bookmark_drop_rect: None,
            native_drag_point: None,
            home,
            history: Vec::new(),
            index: None,
            directory: None,
            visible: Vec::new(),
            single_family: None,
            list_groups: Vec::new(),
            sort: sorting::Sort::default(),
            metadata: metadata::Metadata::default(),
            pending: None,
            error: None,
            failed: None,
            address: String::new(),
            address_completion: address::Completion::default(),
            show_hidden: false,
            restore_scroll: false,
            reveal_selection: false,
            type_navigation: type_navigation::TypeNavigation::default(),
            search: search::Search::default(),
            reveal_target: None,
            focus_address: false,
            focus_list: false,
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
            compare_selection: false,
            external_file: None,
            inspect_file: None,
            document_start: None,
            video_start: None,
            previews: previews::EntryPreviews::default(),
            gallery_previews: previews::EntryPreviews::default(),
            view: BrowserView::List,
            view_epoch: 0,
            view_preferences: Default::default(),
            video_timeline_decimation: 1,
            music_previews: previews::EntryPreviews::default(),
            video_previews: Default::default(),
            list_offsets: vec![0.0],
            thumbnail_edge: 160.0,
            gallery_columns: 1,
            gallery_layout: Default::default(),
            gallery_selecting: false,
            gallery_last_scroll_y: 0.0,
            gallery_last_scroll_time: 0.0,
            gallery_hover: Default::default(),
            photo_mode: gallery::similarity::PhotoMode::default(),
            photo_recurse: false,
            photo_recursive_limit: 10_000,
            photo_recursive_depth: 16,
            photo_recursive_timeout_seconds: 3600,
            similarity_canvas: gallery::similarity::SimilarityCanvas::default(),
            music: music::Music::default(),
            midi: midi_strip::Preview::default(),
            audio_entries: Vec::new(),
            music_selection: None,
            preview_copies: Default::default(),
            large_video_copy: None,
            fuse_mounts: Default::default(),
            mounts: mounts::Mounts::new(),
            constellation: constellation::ConstellationState::new(),
            last_status: None,
        }
    }

    pub(crate) fn set_status(&mut self, icon: Icon, text: impl Into<String>, is_error: bool) {
        self.last_status = Some(LastStatus::new(icon, text, is_error));
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        let text = error.into();
        self.set_status(Icon::Alert, text.clone(), true);
        self.error = Some(text);
    }

    pub(crate) fn clear_status(&mut self) {
        self.last_status = None;
        self.error = None;
        self.edits.report = None;
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

    /// Retires the browser-owned streaming load after an internal rack stop.
    /// Consume hidden-view requests without cancelling a newly chosen Music track.
    fn retire_constellation_audition(&mut self) {
        if std::mem::take(&mut self.constellation.stop_map_audition)
            && self.view == BrowserView::Constellation
        {
            self.music.stop();
        }
    }

    fn stop_previews(&mut self) {
        self.metadata.retire();
        self.previews.multisample.clear();
        self.constellation.stop_playback();
        self.constellation.stop_map_audition = false;
        self.music.stop();
        self.midi.stop();
        self.gallery_hover.clear();
    }
    fn navigate(&mut self, path: PathBuf, navigation: Navigation) {
        self.archives.cancel();
        let path = self.archives.resolve(&path);
        self.containers.retire();
        if matches!(navigation, Navigation::Refresh) {
            self.similarity_canvas.invalidate();
        }
        self.type_navigation.clear();
        self.search.answers.leave();
        if !matches!(navigation, Navigation::Refresh) && self.search.open {
            self.close_search();
        }
        self.reveal_target = None;
        if path != self.path() {
            self.gallery_layout.retire();
            self.video_previews.clear();
            self.video_start = None;
            self.music_selection = None;
            self.previews.text_selection = Default::default();
            self.previews.pdf_pages.selection = Default::default();
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
            self.address = self.location_label();
            return;
        }
        self.pending = Some(Request {
            path,
            navigation,
            browser_target: None,
            search_target: None,
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
        if let Some(snapshot) = self.archive_context()
            && self.path() == snapshot.root
        {
            if let Some(parent) = snapshot.source.path().parent() {
                self.navigate(parent.to_path_buf(), Navigation::New);
            }
            return;
        }
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
                self.set_error(error);
            }
        }
    }

    fn refresh(&mut self) {
        fuse_binwalk::refresh_cached_mount_table();
        self.mounts.request_scan(BackgroundJobs::global());
        self.tree.refresh(self.path().to_path_buf());
        if !self.refresh_search_results() {
            self.navigate(self.path().to_path_buf(), Navigation::Refresh);
        }
    }

    fn retry(&mut self) {
        if let Some((path, navigation)) = self.failed.take() {
            self.navigate(path, navigation);
        } else {
            self.refresh();
        }
    }

    fn tick(&mut self, jobs: &BackgroundJobs) {
        self.tick_archives();
        if self
            .last_status
            .as_ref()
            .is_some_and(|status| !status.is_active(STATUS_MIN_DURATION))
        {
            self.last_status = None;
            self.edits.report = None;
        }
        self.tick_view_preferences(&view_preferences::IO);
        self.tick_search(jobs);
        self.shelf.tick();
        self.bookmarks.tick(jobs);
        self.constellation.tick();
        if self.view == BrowserView::Constellation
            && self.constellation.folder_has_audio == Some(false)
            && self.audio_entries.is_empty()
        {
            self.activate_media_view();
        }
        if let Some(text) = self.bookmarks.report.take() {
            self.set_status(Icon::Alert, text.clone(), true);
            self.edits.report = Some(edits::EditReport { text, failed: true });
        }
        if self.directory.is_none() && self.pending.is_none() && self.error.is_none() {
            self.navigate(self.home.clone(), Navigation::New);
        }
        let mut completion = None;
        if let Some(request) = &mut self.pending {
            let job = request.job.get_or_insert_with(|| {
                DirectoryJob::guarded(
                    request.path.clone(),
                    request.browser_target.clone(),
                    request.search_target.clone(),
                    jobs,
                )
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
                Ok(directory) => {
                    if request.search_target.is_some() {
                        self.close_search();
                    }
                    self.commit(directory, request.navigation);
                }
                Err(error) => {
                    self.set_error(format!(
                        "Cannot open {}: {error}",
                        display_path(&request.path)
                    ));
                    self.reveal_target = None;
                    // Retrying a rejected inspector relationship must not turn it
                    // into an unguarded path navigation to the replacement.
                    self.failed = request
                        .browser_target
                        .is_none()
                        .then_some(())
                        .filter(|_| request.search_target.is_none())
                        .map(|_| (request.path, request.navigation));
                }
            }
            if !self.address_editing {
                self.address = self.location_label();
            }
        }
        self.tick_edits(jobs);
        self.tick_transfers(jobs);
        self.tick_mounts(jobs);
        self.tick_fuse_binwalk(jobs);
        if self.refresh_after_transfer && self.pending.is_none() {
            self.refresh_after_transfer = false;
            self.refresh();
        }
        self.tick_watch(jobs);
        self.tree.tick(jobs);
    }

    fn navigate_outside(&mut self, unmounted_path: &Path) {
        if self.path().starts_with(unmounted_path) {
            let fallback = self
                .history
                .iter()
                .rev()
                .find(|loc| !loc.path.starts_with(unmounted_path))
                .map(|loc| loc.path.clone())
                .unwrap_or_else(|| self.home.clone());
            self.navigate(fallback, Navigation::New);
        } else {
            self.refresh();
        }
    }

    fn tick_mounts(&mut self, jobs: &BackgroundJobs) {
        let Some(event) = self.mounts.tick(jobs) else {
            return;
        };
        match event {
            mounts::MountEvent::Mounted(mount_point) => {
                fuse_binwalk::refresh_cached_mount_table();
                self.navigate(mount_point.clone(), Navigation::New);
                let text = format!("Mounted '{}'", display_path(&mount_point));
                self.set_status(Icon::Drive, text.clone(), false);
                self.edits.report = Some(edits::EditReport {
                    text,
                    failed: false,
                });
            }
            mounts::MountEvent::Unmounted(mount_point) => {
                fuse_binwalk::refresh_cached_mount_table();
                self.navigate_outside(&mount_point);
                let text = format!("Unmounted '{}'", display_path(&mount_point));
                self.set_status(Icon::Eject, text.clone(), false);
                self.edits.report = Some(edits::EditReport {
                    text,
                    failed: false,
                });
            }
            mounts::MountEvent::Error(text) => {
                self.set_status(Icon::Alert, text.clone(), true);
                self.edits.report = Some(edits::EditReport { text, failed: true });
            }
        }
    }

    fn tick_fuse_binwalk(&mut self, jobs: &BackgroundJobs) {
        let Some(outcome) = self.fuse_mounts.tick() else {
            return;
        };
        self.mounts.request_scan(jobs);
        match outcome {
            fuse_binwalk::MountOutcome::Mounted {
                binary_path,
                mount_point,
                writable,
            } => {
                self.navigate(mount_point, Navigation::New);
                let name = binary_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                let mode_str = if writable { "writable" } else { "read-only" };
                let text = format!("Mounted '{name}' ({mode_str}) via fuse-binwalk");
                self.set_status(Icon::BinaryDiff, text.clone(), false);
                self.edits.report = Some(edits::EditReport {
                    text,
                    failed: false,
                });
            }
            fuse_binwalk::MountOutcome::Unmounted { mount_point } => {
                self.navigate_outside(&mount_point);
                let text = "Unmounted binary contents".to_string();
                self.set_status(Icon::Eject, text.clone(), false);
                self.edits.report = Some(edits::EditReport {
                    text,
                    failed: false,
                });
            }
            fuse_binwalk::MountOutcome::Failed(text) => {
                self.set_status(Icon::Alert, text.clone(), true);
                self.edits.report = Some(edits::EditReport { text, failed: true });
            }
        }
    }

    fn append_location(&mut self, path: PathBuf) {
        let manual_view = (self.view == BrowserView::Columns
            && self.view_preferences.effective(&path).is_none())
        .then_some(BrowserView::Columns);
        self.history.push(Location {
            path,
            selected: None,
            selection: BTreeSet::new(),
            anchor: None,
            selection_undo: None,
            scroll: 0.0,
            manual_view,
        });
        self.index = Some(self.history.len() - 1);
    }

    fn commit(&mut self, directory: Directory, navigation: Navigation) {
        fuse_binwalk::refresh_cached_mount_table();
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
        if matches!(navigation, Navigation::Refresh) {
            self.containers.reconcile_roots(&directory.entries);
        } else {
            self.containers.clear();
        }
        self.view_preferences.reload();
        let tree_path = self
            .archives
            .export_directory(&directory.path)
            .unwrap_or_else(|| directory.path.clone());
        self.tree.reveal(&tree_path);
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
        self.single_family = None;
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
            self.search.filter(directory, &mut self.visible);
            self.single_family = self
                .search
                .applied
                .is_none()
                .then(|| sorting::single_family(directory, &self.visible))
                .flatten();
            self.search.answers.reconcile(directory, &self.visible);
            self.search.answers.sort(directory, &mut self.visible);
            self.metadata.reconcile(directory, &self.visible);
            if self.single_family.is_some() || self.grouped_by_type() {
                self.metadata.order(directory, &mut self.visible);
            }
            self.search.paginate(&mut self.visible);
        }
        self.reconcile_selection();
        self.audio_entries.clear();
        if let Some(directory) = &self.directory {
            for index in &self.visible {
                let entry = &directory.entries[*index];
                let audio =
                    entry.regular_file && little_clip_core::audio::is_audio_path(&entry.path);
                if audio {
                    self.audio_entries.push(entry.path.clone());
                }
            }
        }
        self.activate_media_view();
        self.rebuild_list_offsets();
    }

    fn rebuild_list_offsets(&mut self) {
        self.list_offsets.clear();
        self.list_groups.clear();
        self.list_offsets.push(0.0);
        let sort = self.list_sort().filter(|_| self.single_family.is_none());
        if let Some(directory) = &self.directory {
            for (position, index) in self.visible.iter().enumerate() {
                let entry = &directory.entries[*index];
                let family =
                    sort.and_then(|sort| sort.group_at(directory, &self.visible, position));
                if let Some(family) = family {
                    self.list_groups.push((position, family));
                }
                let heading = family.map_or(0.0, |_| ui::GROUP_HEADER_HEIGHT);
                let height = if self.search.has_offline_results() {
                    28.0
                } else if !self.search.has_result_sources() && container_preview::candidate(entry) {
                    self.containers.height(entry, self.show_hidden)
                } else if entry.system.is_some()
                    || self.previews.is_text(entry)
                    || entry.has_content_strip()
                    || entry.is_paged_document()
                    || (entry.regular_file
                        && little_clip_core::notebook::is_candidate_path(&entry.path))
                {
                    VIDEO_ROW_HEIGHT
                } else {
                    28.0
                };
                self.list_offsets.push(
                    self.list_offsets.last().copied().unwrap_or(0.0)
                        + heading
                        + height
                        + self.search.result_height(entry),
                );
            }
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        if !self.refresh_search_results() {
            self.rebuild_visible();
        }
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
        let mut entries = self.displayed_entries();
        // Lists and Miller columns navigate folder rows. Inline descendants
        // remain clickable previews, rather than extra keyboard stops.
        let folder_rows = matches!(self.view, BrowserView::List | BrowserView::Columns)
            && !self.search.has_result_sources();
        if folder_rows {
            entries.retain(|entry| entry.path.parent() == Some(self.path()));
        }
        if entries.is_empty() {
            return;
        }
        let selected = self
            .location()
            .and_then(|location| location.selected.as_ref());
        let index = entries.iter().position(|entry| {
            selected.is_some_and(|path| {
                path == &entry.path || (folder_rows && path.starts_with(&entry.path))
            })
        });
        let index = match index {
            Some(i) => i.saturating_add_signed(delta).min(entries.len() - 1),
            None if delta >= 0 => 0,
            None => entries.len() - 1,
        };
        let path = entries[index].path.clone();
        self.select_with(path, false, extend);
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
            .displayed_entries()
            .iter()
            .find(|entry| Some(&entry.path) == selected)
            .map(|entry| entry.path.clone());
        if let Some(path) = path {
            self.open_path(path);
        }
    }

    fn open_path(&mut self, path: PathBuf) {
        self.type_navigation.clear();
        if self
            .document_start
            .as_ref()
            .is_some_and(|(source, position)| {
                source == &path && *position == DocumentPosition::MinecraftMap
            })
        {
            self.archives.cancel();
            self.open_file = Some(path);
            return;
        }
        if container_preview::inventory::archive(&path)
            && self
                .displayed_entries()
                .iter()
                .any(|entry| entry.path == path && entry.regular_file)
        {
            self.open_archive(path);
            return;
        }
        self.archives.cancel();
        if self
            .displayed_entries()
            .iter()
            .any(|entry| entry.path == path && entry.directory)
        {
            if self.search_source_guard(&path).is_some() {
                self.reveal_search_result(path, true);
            } else {
                self.navigate(path, Navigation::New);
            }
        } else {
            self.open_file = Some(path);
        }
    }
}

impl FileManager {
    fn tick_watch(&mut self, jobs: &BackgroundJobs) {
        if self
            .search
            .applied
            .as_ref()
            .is_some_and(|query| self.search.uses_worker(query))
        {
            self.watch = None;
            return;
        }
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
