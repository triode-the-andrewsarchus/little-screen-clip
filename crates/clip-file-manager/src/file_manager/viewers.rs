//! One file-backed studio at a time; closing returns to the untouched browser.
use super::document_format::Format;
use crate::{
    audio_preview::AudioPreviewState,
    file_source::FileSource,
    image_preview::{DecodedImageData, ImagePreviewState},
    midi_view::MidiViewerState,
    model3d_view::Model3dViewerState,
    pdf_view::PdfEditorState,
    splat_view::SplatViewerState,
    spreadsheet_view::SpreadsheetViewerState,
};
use egui::{Context, Ui};
use little_clip_core::{AppConfig, BackgroundJobs, JobError, JobTicket, palette::AppPalette};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

mod chrome;
mod gallery;
mod rename;
mod video;
mod video_audio;

enum Loaded {
    System(Box<super::system::Document>),
    Font(super::font_strip::viewer::Viewer),
    Notebook(super::notebook::viewer::Viewer),
    Book(super::ebook::Viewer),
    Mail(super::mail::Viewer),
    Calendar(super::calendar::Viewer),
    Database(super::database::Viewer),
    Geography(super::geography::Viewer),
    Toolpath(super::gcode::Viewer),
    Multisample(Box<super::multisample::Viewer>),
    Image(DecodedImageData, FileSource),
    Audio(Option<little_clip_core::audio::AudioMetadata>, FileSource),
    Text(Box<super::text_editor::TextSession>),
    Sheet(little_clip_core::presentation::spreadsheet::SpreadsheetData),
    Midi(little_clip_core::presentation::midi::MidiData),
    Handwritten,
    OfficePdf {
        pdf_path: PathBuf,
        workspace: little_clip_core::staging::TemporaryWorkspace,
        original_path: PathBuf,
    },
    Path,
    Video(super::media::VideoMetadata),
    VideoAudio(video_audio::Prepared),
    External,
}
enum ViewerSession {
    System(super::system::Inspector),
    Font(super::font_strip::viewer::Viewer),
    Notebook(super::notebook::viewer::Viewer),
    Book(super::ebook::Viewer),
    Mail(super::mail::Viewer),
    Calendar(super::calendar::Viewer),
    Database(super::database::Viewer),
    Geography(super::geography::Viewer),
    Toolpath(super::gcode::Viewer),
    Multisample(super::multisample::Viewer),
    Image(Box<ImagePreviewState>),
    Text(super::text_editor::TextSession),
    Audio(Box<AudioPreviewState>),
    Pdf(PdfEditorState),
    Sheet(SpreadsheetViewerState),
    Midi(MidiViewerState),
    Model(Model3dViewerState),
    Scene(SplatViewerState),
    Video(video::Video),
}
struct Loading {
    response_deadline: Option<std::time::Instant>,
    ticket: JobTicket,
    path: PathBuf,
    receiver: little_clip_core::jobs::JobReceiver<Result<(Loaded, Option<FileSource>), String>>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Loading {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.ticket.cancel();
    }
}
#[derive(Default)]
pub(super) struct Viewers {
    source: Option<FileSource>,
    source_path: Option<PathBuf>,
    source_request: Option<(
        PathBuf,
        little_clip_core::jobs::JobReceiver<Result<FileSource, String>>,
    )>,
    rename: rename::Rename,
    renamed: Vec<(PathBuf, PathBuf)>,
    pub(super) saved_rating_paths: Vec<PathBuf>,
    close_requested: bool,
    office_preview: bool,
    loading: Option<Loading>,
    pub(super) source_guard: Option<super::search::SourceGuard>,
    session: Option<ViewerSession>,
    path: Option<PathBuf>,
    error: Option<String>,
    pub(super) external_request: Option<super::external::Request>,
    confirm_close: bool,
    pad_audio: bool,
    pad_trim: Option<(f32, f32)>,
    audio_return_saving: bool,
    audio_result: Option<PathBuf>,
    config: AppConfig,
    gallery: Option<gallery::Collection>,
    video_start_seconds: f64,
    document_start: Option<super::DocumentPosition>,
    system_navigation: Option<super::system::BrowserTarget>,
}
impl Viewers {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }
    pub(super) fn video_fullscreen(&self) -> bool {
        matches!(&self.session, Some(ViewerSession::Video(video)) if video.fullscreen())
    }
    pub fn active(&self) -> bool {
        self.loading.is_some() || self.session.is_some() || self.error.is_some()
    }
    pub(super) fn external_error(&mut self, error: String) {
        self.error = Some(error);
    }
    pub fn uses_audio(&self) -> bool {
        matches!(
            self.session,
            Some(
                ViewerSession::Audio(_)
                    | ViewerSession::Midi(_)
                    | ViewerSession::Video(_)
                    | ViewerSession::Multisample(_)
            )
        ) || self.loading.as_ref().is_some_and(|l| {
            matches!(
                kind(&l.path),
                Kind::Audio | Kind::Midi | Kind::Video | Kind::Document(Format::Multisample)
            )
        })
    }
    pub fn open_system(&mut self, path: PathBuf) {
        self.open_on(path, true);
    }
    pub(crate) fn open_pad_audio(&mut self, path: PathBuf, trim: (f32, f32)) {
        self.open(path);
        self.pad_audio = true;
        self.pad_trim = Some(trim);
    }
    pub(crate) fn take_audio_result(&mut self) -> Option<PathBuf> {
        self.audio_result.take()
    }

    pub fn open(&mut self, path: PathBuf) {
        self.open_on(path, false);
    }
    fn open_on(&mut self, path: PathBuf, system: bool) {
        self.open_mode(path, system, false);
    }
    fn open_mode(&mut self, path: PathBuf, system: bool, video_audio: bool) {
        if self.active() {
            return;
        }
        self.video_start_seconds = 0.0;
        self.document_start = None;
        let source = path.clone();
        let config = self.config.clone();
        let executor = if system {
            super::system::jobs()
        } else {
            BackgroundJobs::global()
        };
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = cancelled.clone();
        let guard = self.source_guard.clone();
        let (ticket, receiver) = executor.submit(Duration::from_secs(120), move || {
            super::search::SourceGuard::read_checked(guard.as_ref(), &worker, || {
                if system {
                    super::system::collect(&source, &worker)
                        .map(|document| (Loaded::System(Box::new(document)), None))
                } else {
                    let identity = FileSource::capture(&source);
                    let loaded = if video_audio {
                        Loaded::VideoAudio(video_audio::Prepared::load(&source, &worker)?)
                    } else {
                        load(&source, &config, &worker)?
                    };
                    let identity = if matches!(&loaded, Loaded::System(_)) {
                        None
                    } else {
                        let identity = identity?;
                        identity.check()?;
                        Some(identity)
                    };
                    Ok((loaded, identity))
                }
            })
        });
        self.loading = Some(Loading {
            response_deadline: system
                .then(|| std::time::Instant::now() + super::system::RESPONSE_DEADLINE),
            cancelled,
            ticket,
            path,
            receiver,
        });
    }
    pub fn open_video_at(&mut self, path: PathBuf, seconds: f64) {
        if self.active() {
            return;
        }
        self.open(path);
        self.video_start_seconds = seconds;
    }
    pub fn open_document_at(&mut self, path: PathBuf, position: super::DocumentPosition) {
        if self.active() {
            return;
        }
        if position == super::DocumentPosition::VideoAudioTrack {
            self.open_mode(path, false, true);
        } else {
            self.open(path);
        }
        self.document_start = Some(position);
    }
    fn poll(&mut self, ctx: &Context) {
        let Some(loading) = &self.loading else { return };
        let Some(result) = super::preview_job::poll(&loading.receiver, loading.response_deadline)
            .map(|result| {
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
            })
        else {
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        };
        let path = self.loading.take().unwrap().path.clone();
        self.path = Some(path.clone());
        let result = result.map(|(loaded, source)| {
            self.source = source;
            if let Loaded::VideoAudio(audio) = &loaded {
                self.path = Some(audio.source.path().to_path_buf());
                self.source = Some(audio.source.clone());
                self.source_guard = None;
            }
            self.source_path = self.path.clone();
            self.office_preview = matches!(&loaded, Loaded::OfficePdf { .. });
            loaded
        });
        match result {
            Ok(Loaded::External) => {
                self.open_external();
                if self.error.is_none() {
                    self.path = None;
                }
            }
            Ok(loaded) => {
                let mut opened = session(
                    loaded,
                    path,
                    &self.config,
                    self.video_start_seconds,
                    self.source_guard
                        .as_ref()
                        .map(super::search::SourceGuard::read_check)
                        .unwrap_or_default(),
                );
                if self.gallery.is_some()
                    && let ViewerSession::Image(state) = &mut opened
                {
                    state.keep_source_on_save_as();
                }
                self.session = Some(opened);
            }
            Err(error) => self.error = Some(error),
        }
        match (&mut self.session, self.document_start.take()) {
            (
                Some(ViewerSession::Notebook(state)),
                Some(super::DocumentPosition::NotebookCell(cell)),
            ) => state.select(cell),
            (
                Some(ViewerSession::Book(state)),
                Some(super::DocumentPosition::BookChapter(index)),
            ) => state.select(index),
            (
                Some(ViewerSession::Mail(state)),
                Some(super::DocumentPosition::MailAttachment(index)),
            ) => state.select(index),
            (
                Some(ViewerSession::Calendar(state)),
                Some(super::DocumentPosition::CalendarEvent(index)),
            ) => state.select(index),
            (
                Some(ViewerSession::Database(state)),
                Some(super::DocumentPosition::DatabaseTable(index)),
            ) => state.select(index),
            (
                Some(ViewerSession::Geography(state)),
                Some(super::DocumentPosition::GeographicFeature(index)),
            ) => state.select(index),
            (Some(ViewerSession::Pdf(state)), Some(super::DocumentPosition::PdfPage(page))) => {
                state.selected_page = Some(page.max(1))
            }
            (Some(ViewerSession::Text(state)), Some(super::DocumentPosition::TextLine(line))) => {
                state.preview.view_mode = crate::text_preview::TextViewMode::Raw;
                state.preview.center_on_source_line(line);
            }
            (
                Some(ViewerSession::Toolpath(state)),
                Some(super::DocumentPosition::ToolpathLine(line)),
            ) => state.select_line(line),
            (
                Some(ViewerSession::Multisample(state)),
                Some(super::DocumentPosition::SampleRegion(instrument, region)),
            ) => state.select(instrument, region),
            (Some(ViewerSession::Text(state)), Some(super::DocumentPosition::SubtitleCue(cue))) => {
                state.preview.select_subtitle(cue)
            }
            _ => {}
        }
        if let Some(ViewerSession::Audio(state)) = &mut self.session
            && let Some((start, end)) = self.pad_trim.take()
        {
            state.selection = Some((start.min(state.duration_sec), end.min(state.duration_sec)));
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) -> bool {
        self.poll(ui.ctx());
        self.poll_file_source(ui.ctx());
        let mut close = std::mem::take(&mut self.close_requested);
        self.gallery_controls(ui, palette);
        let blocked = self.collection_pending()
            || self.collection_modal()
            || self.rename.open
            || self.rename.busy();
        let editor_enabled = !self.confirm_close && !self.audio_return_saving && !blocked;
        if self.loading.is_some() {
            ui.spinner();
            ui.label("Opening file…");
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        ui.add_enabled_ui(editor_enabled, |ui| {
            self.render_file_session(ui, palette, &mut close)
        });
        // Dialogs paint after the disabled editor, whose focus registrations must not steal typing.
        self.rename_ui(ui, palette);
        self.poll_audio_return();
        if self.session.is_none() {
            close |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        }
        if close {
            self.request_close();
        }
        self.close_confirmation(ui, palette);
        !self.active()
    }
    fn poll_audio_return(&mut self) {
        if self.audio_return_saving
            && let Some(ViewerSession::Audio(state)) = &mut self.session
        {
            if let Some(path) = state.take_saved_files().into_iter().next() {
                self.audio_result = Some(path);
                self.clear();
            } else if !state.file_write_pending() {
                self.audio_return_saving = false;
                self.confirm_close = true;
            }
        }
    }

    fn close_confirmation(&mut self, ui: &mut Ui, palette: &AppPalette) {
        if self.confirm_close {
            egui::Window::new("Unsaved edits")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(if self.pad_audio {
                        "Save an edited copy beside the original and replace this drum pad layer?"
                    } else {
                        "Save your changes before returning to the folder."
                    });
                    if self.pad_audio
                        && ui
                            .add(
                                clip_ui::buttons::ButtonRole::Primary
                                    .button("Save copy and replace pad", palette),
                            )
                            .clicked()
                        && let Some(ViewerSession::Audio(state)) = &mut self.session
                    {
                        match state.save_edited_sibling(ui.input(|input| input.time)) {
                            Ok(()) => {
                                self.audio_return_saving = true;
                                self.confirm_close = false;
                            }
                            Err(error) => {
                                self.error = Some(error);
                            }
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                clip_ui::buttons::ButtonRole::Secondary
                                    .button("Keep editing", palette),
                            )
                            .clicked()
                        {
                            self.confirm_close = false;
                        }
                        if ui
                            .add(clip_ui::buttons::ButtonRole::Destructive.button(
                                if self.pad_audio {
                                    "Return without replacing"
                                } else {
                                    "Discard edits"
                                },
                                palette,
                            ))
                            .clicked()
                        {
                            self.clear();
                            self.gallery = None;
                        }
                    });
                });
        }
    }

    fn render_session(&mut self, ui: &mut Ui, palette: &AppPalette, close: &mut bool) {
        if let Some(viewer) = &mut self.session {
            render(viewer, ui, palette, close);
            if let ViewerSession::System(state) = viewer
                && let Some(target) = state.take_browser_target()
            {
                self.system_navigation = Some(target);
                *close = true;
            }
            match viewer {
                ViewerSession::System(state) => self.path = Some(state.document.path.clone()),
                ViewerSession::Image(state) => self.path = state.path.clone(),
                ViewerSession::Text(state) => self.path = Some(state.path().to_path_buf()),
                ViewerSession::Pdf(state) if !self.office_preview => {
                    self.path = Some(state.path.clone())
                }
                ViewerSession::Audio(state) => self.path = Some(state.path.clone()),
                ViewerSession::Sheet(state) => self.path = state.path.clone(),
                ViewerSession::Midi(state) => self.path = state.path.clone(),
                ViewerSession::Model(state) => self.path = state.path.clone(),
                _ => {}
            }
        }
    }
    pub fn take_system_navigation(&mut self) -> Option<super::system::BrowserTarget> {
        self.system_navigation.take()
    }
    pub fn request_close(&mut self) -> bool {
        if self.rename.open {
            self.rename.open = false;
            return false;
        }
        if let Some(ViewerSession::Image(state)) = &mut self.session {
            state.poll_cached_jobs();
        }
        if self.rename.busy() || self.collection_pending() || self.writes_pending() {
            self.error = Some("Wait for file save, export or copy to finish.".into());
            return false;
        }
        if self.session_dirty() || self.collection_dirty() {
            self.confirm_close = true;
            return false;
        }
        self.clear();
        self.gallery = None;
        true
    }
    fn clear(&mut self) {
        if let Some(ViewerSession::Audio(state)) = &mut self.session {
            state.stop();
        }
        if let Some(ViewerSession::Midi(state)) = &mut self.session {
            state.stop();
        }
        self.session = None;
        self.source = None;
        self.source_path = None;
        self.source_request = None;
        self.office_preview = false;
        self.source_guard = None;
        self.loading = None;
        self.path = None;
        self.error = None;
        self.confirm_close = false;
        self.pad_audio = false;
        self.pad_trim = None;
        self.audio_return_saving = false;
        self.video_start_seconds = 0.0;
    }
    fn open_external(&mut self) {
        if let Some(ViewerSession::Video(state)) = &mut self.session {
            state.stop();
        }
        let saved = match &self.session {
            Some(ViewerSession::Text(state)) => state.file_source(),
            Some(ViewerSession::Image(state)) => state.file_source(),
            _ => None,
        };
        let path = saved
            .as_ref()
            .map(|source| source.path().to_path_buf())
            .or_else(|| self.path.clone());
        if let Some(path) = path {
            self.external_request = Some(super::external::Request {
                path,
                search: self.source_guard.clone(),
                saved,
            });
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Document(Format),
    Image,
    Audio,
    Pdf,
    OfficeDoc,
    Sheet,
    Midi,
    Scene,
    Model,
    Text,
    Video,
    External,
}
fn kind(path: &Path) -> Kind {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dsl"))
        || little_clip_core::diagram::Format::from_path(path).is_some()
        || super::text_strip::is_typescript_source(path)
        || little_clip_core::subtitles::is_subtitle_path(path)
    {
        Kind::Text
    } else if let Some(format) = Format::for_path(path) {
        Kind::Document(format)
    } else if super::midi_strip::is_midi(path) {
        Kind::Midi
    } else if little_clip_core::item::is_image_path(path) {
        Kind::Image
    } else if little_clip_core::audio::is_audio_path(path) {
        Kind::Audio
    } else if little_clip_core::is_video_path(path) {
        Kind::Video
    } else if little_clip_core::pdf::is_pdf_path(path) {
        Kind::Pdf
    } else if little_clip_core::document::is_office_document(path) {
        Kind::OfficeDoc
    } else if little_clip_core::presentation::spreadsheet::is_spreadsheet_file(path) {
        Kind::Sheet
    } else if little_clip_scene::is_splat_file(path) {
        Kind::Scene
    } else if little_clip_core::presentation::model3d::is_3d_model_file(path) {
        Kind::Model
    } else if little_clip_core::model3d::materials::is_material_file(path)
        || little_clip_ops::is_text_file(path)
    {
        Kind::Text
    } else {
        Kind::External
    }
}

fn load(
    path: &Path,
    config: &AppConfig,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<Loaded, String> {
    if little_clip_core::kernel_interface::classify(path)
        .map_err(|e| e.to_string())?
        .semantic()
    {
        return super::system::collect(path, &std::sync::atomic::AtomicBool::new(false))
            .map(|document| Loaded::System(Box::new(document)));
    }
    little_clip_core::kernel_interface::require_content(path).map_err(|e| e.to_string())?;
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Only regular files can be inspected".into());
    }
    if let Some(document) = super::multisample::read_if_identified(path, cancelled) {
        return Ok(Loaded::Multisample(Box::new(
            super::multisample::Viewer::from_identified(path, document),
        )));
    }
    if little_clip_core::notebook::is_candidate_path(path)
        && little_clip_core::notebook::has_header(path, cancelled)?
    {
        little_clip_core::notebook::inspect(path, cancelled)?;
        return Ok(Loaded::Handwritten);
    }
    let kind = kind(path);
    let limit = match kind {
        Kind::Image => 64 * 1024 * 1024,
        Kind::Sheet | Kind::Midi | Kind::Document(Format::Notebook) => 32 * 1024 * 1024,
        _ => u64::MAX,
    };
    if metadata.len() > limit {
        return Err(format!(
            "File exceeds the {} MiB opening limit; use Open externally.",
            limit / (1024 * 1024)
        ));
    }
    match kind {
        Kind::Document(Format::Multisample) => super::multisample::Viewer::load(path, cancelled)
            .map(|viewer| Loaded::Multisample(Box::new(viewer))),
        Kind::Document(Format::Toolpath) => super::gcode::Viewer::load(path).map(Loaded::Toolpath),
        Kind::Document(Format::Geography) => {
            super::geography::Viewer::load(path).map(Loaded::Geography)
        }
        Kind::Document(Format::Database) => {
            super::database::Viewer::load(path).map(Loaded::Database)
        }
        Kind::Document(Format::Calendar) => {
            super::calendar::Viewer::load(path).map(Loaded::Calendar)
        }
        Kind::Document(Format::Mail) => super::mail::Viewer::load(path).map(Loaded::Mail),
        Kind::Document(Format::Book) => super::ebook::Viewer::load(path).map(Loaded::Book),
        Kind::Document(Format::Notebook) => {
            super::notebook::viewer::Viewer::load(path).map(Loaded::Notebook)
        }
        Kind::Document(Format::Font) => {
            super::font_strip::viewer::Viewer::load(path).map(Loaded::Font)
        }
        Kind::Image => {
            let source = FileSource::capture(path)?;
            let decoded = DecodedImageData::from_path_bounded(path, 64 * 1024 * 1024)?;
            if FileSource::capture(path)? != source {
                return Err("Source changed while opening; open it again.".into());
            }
            Ok(Loaded::Image(decoded, source))
        }
        Kind::Audio => {
            let source = FileSource::capture(path)?;
            let metadata = little_clip_core::audio::read_audio_metadata(path);
            source.check()?;
            Ok(Loaded::Audio(metadata, source))
        }
        Kind::Sheet => {
            little_clip_core::presentation::spreadsheet::load_spreadsheet(path).map(Loaded::Sheet)
        }
        Kind::Midi => {
            little_clip_core::presentation::midi::MidiData::parse_file(path).map(Loaded::Midi)
        }
        Kind::OfficeDoc => match little_clip_core::document::prepare_office_preview(path)? {
            little_clip_core::document::PreparedOfficePreview::Pdf {
                pdf_path,
                workspace,
            } => Ok(Loaded::OfficePdf {
                pdf_path,
                workspace,
                original_path: path.to_path_buf(),
            }),
            little_clip_core::document::PreparedOfficePreview::Markdown(md) => {
                let text_session = super::text_editor::TextSession::from_markdown(path, md, config);
                Ok(Loaded::Text(Box::new(text_session)))
            }
        },
        Kind::Text | Kind::Document(Format::Subtitle) => {
            super::text_editor::TextSession::load(path, config)
                .map(|state| Loaded::Text(Box::new(state)))
        }
        Kind::Video => video::prepare(path).map(Loaded::Video),
        Kind::External => Ok(Loaded::External),
        Kind::Pdf | Kind::Scene | Kind::Model => FileSource::capture(path).map(|_| Loaded::Path),
    }
}
fn session(
    loaded: Loaded,
    path: PathBuf,
    config: &AppConfig,
    video_start: f64,
    check: crate::file_source::ReadCheck,
) -> ViewerSession {
    match loaded {
        Loaded::Handwritten => {
            ViewerSession::Pdf(PdfEditorState::new_notebook_checked(path, check))
        }
        Loaded::Notebook(viewer) => ViewerSession::Notebook(viewer),
        Loaded::Book(viewer) => ViewerSession::Book(viewer),
        Loaded::Multisample(viewer) => ViewerSession::Multisample(*viewer),
        Loaded::Toolpath(viewer) => ViewerSession::Toolpath(viewer),
        Loaded::Geography(viewer) => ViewerSession::Geography(viewer),
        Loaded::Database(viewer) => ViewerSession::Database(viewer),
        Loaded::Calendar(viewer) => ViewerSession::Calendar(viewer),
        Loaded::Mail(viewer) => ViewerSession::Mail(viewer),
        Loaded::Font(viewer) => ViewerSession::Font(viewer),
        Loaded::System(document) => ViewerSession::System(super::system::Inspector::new(*document)),
        Loaded::Image(data, source) => ViewerSession::Image(Box::new(
            ImagePreviewState::from_decoded(
                data,
                Some(path.clone()),
                super::display_path(&path),
                config,
            )
            .with_file_source(source),
        )),
        Loaded::Video(prepared) => ViewerSession::Video(video::Video::new(
            path,
            prepared,
            video_start,
            config.file_manager.video_subtitles_enabled,
        )),
        Loaded::VideoAudio(prepared) => ViewerSession::Audio(Box::new(prepared.studio(config))),
        Loaded::Text(state) => ViewerSession::Text(*state),
        Loaded::Sheet(data) => ViewerSession::Sheet(SpreadsheetViewerState::new(data, Some(path))),
        Loaded::Midi(data) => ViewerSession::Midi(MidiViewerState::from_parsed_file(path, data)),
        Loaded::OfficePdf {
            pdf_path,
            workspace,
            original_path,
        } => {
            let title = original_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "document".to_string());
            ViewerSession::Pdf(
                PdfEditorState::new(pdf_path)
                    .with_title(title)
                    .with_office_workspace(workspace)
                    .with_protected_outputs(),
            )
        }
        Loaded::Audio(metadata, source) => ViewerSession::Audio(Box::new(
            AudioPreviewState::new_with_metadata_checked(
                path.clone(),
                super::display_path(&path),
                metadata,
                check,
            )
            .with_editor(config.audio.editor.clone())
            .with_file_source(source),
        )),
        _ if little_clip_core::pdf::is_pdf_path(&path) => {
            ViewerSession::Pdf(PdfEditorState::new_checked(path, check).with_protected_outputs())
        }
        _ if little_clip_scene::is_splat_file(&path) => ViewerSession::Scene(
            SplatViewerState::from_file_checked(path, check).with_config(config.model3d.clone()),
        ),
        _ => ViewerSession::Model(
            Model3dViewerState::from_file_checked(path, check).with_config(config.model3d.clone()),
        ),
    }
}
fn render(viewer: &mut ViewerSession, ui: &mut Ui, palette: &AppPalette, close: &mut bool) {
    let ctx = ui.ctx().clone();
    match viewer {
        ViewerSession::Notebook(state) => state.ui(ui, close, palette),
        ViewerSession::Book(state) => state.ui(ui, close, palette),
        ViewerSession::Multisample(state) => state.ui(ui, close, palette),
        ViewerSession::Toolpath(state) => state.ui(ui, close, palette),
        ViewerSession::Geography(state) => state.ui(ui, close, palette),
        ViewerSession::Database(state) => state.ui(ui, close, palette),
        ViewerSession::Calendar(state) => state.ui(ui, close, palette),
        ViewerSession::Mail(state) => state.ui(ui, close, palette),
        ViewerSession::Font(state) => state.ui(ui, close, palette),
        ViewerSession::System(state) => state.ui(ui, close, palette),
        ViewerSession::Image(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Text(state) => state.ui(ui, palette, close),
        ViewerSession::Audio(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Pdf(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Sheet(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Midi(state) => state.render_ui(ui, palette, close),
        ViewerSession::Model(state) => state.render_ui(ui, palette, close),
        ViewerSession::Scene(state) => state.render_ui(ui, palette, close),
        ViewerSession::Video(state) => state.ui(ui, close, palette),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finish_loading(viewers: &mut Viewers, ctx: &Context) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while viewers.loading.is_some() {
            viewers.poll(ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn native_diagram_extensions_select_guarded_text_studios() {
        for name in [
            "drawing.pikchr",
            "drawing.svgbob",
            "drawing.mermaid",
            "drawing.lini",
            "drawing.gv",
            "drawing.graphviz",
            "drawing.bpmn",
            "drawing.bpmn2",
            "drawing.wavejson",
            "drawing.wavedrom",
            "drawing.nomnoml",
            "drawing.noml",
            "schema.dbml",
            "diagram.goat",
            "packet.bytefield",
            "schema.er",
            "schema.erd",
            "sequence.puml",
            "class.plantuml",
            "diagram.blockdiag",
            "diagram.seqdiag",
            "diagram.actdiag",
            "diagram.nwdiag",
            "diagram.packetdiag",
            "diagram.rackdiag",
            "diagram.ditaa",
            "diagram.tikz",
            "diagram.pgf",
            "diagram.pgfplots",
            "diagram.uxf",
            "diagram.umlet",
            "diagram.symbolator",
            "diagram.symbolator.vhdl",
            "diagram.symbolator.v",
            "chart.vega",
            "chart.vegalite",
            "chart.vg.json",
            "chart.vl.json",
            "architecture.structurizr",
            "architecture.dsl",
            "drawing.drawio",
            "drawing.dio",
            "drawing.drawio.xml",
            "include.iuml",
            "activity.pu",
            "drawing.excalidraw",
            "drawing.excalidraw.json",
            "DRAWING.PIKCHR",
        ] {
            assert_eq!(kind(Path::new(name)), Kind::Text);
        }
    }

    #[test]
    fn document_hints_share_the_registry_and_ambiguous_multisamples_require_content() {
        for (name, format) in [
            ("missing.NKI", Format::Multisample),
            ("missing.IPYNB", Format::Notebook),
            ("missing.GCODE", Format::Toolpath),
            ("missing.OTF", Format::Font),
        ] {
            let path = Path::new(name);
            assert_eq!(Format::for_path(path), Some(format));
            assert_eq!(kind(path), Kind::Document(format));
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("instrument.xml");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../little-clip-core/tests/fixtures/multisample/harbor.dspreset");
        std::fs::copy(fixture, &path).unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(Format::for_path(&path), None);
        assert!(!matches!(kind(&path), Kind::Document(Format::Multisample)));
        assert!(matches!(
            load(&path, &AppConfig::default(), &cancelled).unwrap(),
            Loaded::Multisample(_)
        ));
        std::fs::write(&path, "<notes><sample>ordinary XML</sample></notes>").unwrap();
        assert!(matches!(
            load(&path, &AppConfig::default(), &cancelled).unwrap(),
            Loaded::Text(_)
        ));
    }

    #[test]
    fn true_type_fonts_open_in_the_interactive_specimen_viewer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("specimen.TTF");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../little-clip-render/assets/fonts/NotoEmoji-Regular.ttf"),
            &path,
        )
        .unwrap();
        assert!(matches!(
            load(
                &path,
                &AppConfig::default(),
                &std::sync::atomic::AtomicBool::new(false)
            )
            .unwrap(),
            Loaded::Font(_)
        ));
    }

    #[test]
    fn open_type_cff_outlines_open_in_the_interactive_specimen_viewer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("specimen.OTF");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../little-clip-render/assets/fonts/NotoSansCJK-JP-Regular.ttf"),
            &path,
        )
        .unwrap();
        assert_eq!(&std::fs::read(&path).unwrap()[..4], b"OTTO");
        assert!(matches!(
            load(
                &path,
                &AppConfig::default(),
                &std::sync::atomic::AtomicBool::new(false)
            )
            .unwrap(),
            Loaded::Font(_)
        ));
    }

    #[test]
    fn compressed_web_fonts_open_in_the_specimen_viewer() {
        for extension in ["woff", "woff2"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/fonts/latin.{extension}"));
            assert!(matches!(
                load(
                    &path,
                    &AppConfig::default(),
                    &std::sync::atomic::AtomicBool::new(false)
                )
                .unwrap(),
                Loaded::Font(_)
            ));
        }
    }

    #[test]
    fn clicked_document_position_survives_background_loading() {
        for (fixture, position, label) in [
            (
                "../little-clip-core/tests/fixtures/subtitles/sample.srt",
                super::super::DocumentPosition::SubtitleCue(1),
                "Cue 2",
            ),
            (
                "../little-clip-core/tests/fixtures/subtitles/sample.vtt",
                super::super::DocumentPosition::SubtitleCue(1),
                "Cue 2",
            ),
            (
                "../little-clip-core/tests/fixtures/subtitles/sample.ass",
                super::super::DocumentPosition::SubtitleCue(1),
                "Cue 2",
            ),
            (
                "../little-clip-core/tests/fixtures/subtitles/sample.ssa",
                super::super::DocumentPosition::SubtitleCue(1),
                "Cue 2",
            ),
            (
                "tests/fixtures/notebooks/sample.ipynb",
                super::super::DocumentPosition::NotebookCell(2),
                "Code cell 3",
            ),
            (
                "../little-clip-core/tests/fixtures/ebooks/journey.epub",
                super::super::DocumentPosition::BookChapter(1),
                "Chapter 2 / 2",
            ),
            (
                "tests/fixtures/comic.cbz",
                super::super::DocumentPosition::BookChapter(1),
                "Page 2 / 3",
            ),
            (
                "../little-clip-core/tests/fixtures/geography/harbor.gpx",
                super::super::DocumentPosition::GeographicFeature(1),
                "Ferry route",
            ),
            (
                "../little-clip-core/tests/fixtures/geography/harbor.geojson",
                super::super::DocumentPosition::GeographicFeature(3),
                "Unplaced record",
            ),
            (
                "../little-clip-core/tests/fixtures/gcode/frame.gcode",
                super::super::DocumentPosition::ToolpathLine(12),
                "Source line 12",
            ),
            (
                "../little-clip-core/tests/fixtures/multisample/harbor.sfz",
                super::super::DocumentPosition::SampleRegion(0, 1),
                "Bright keys",
            ),
            (
                "../little-clip-core/tests/fixtures/multisample/harbor.sf2",
                super::super::DocumentPosition::SampleRegion(1, 0),
                "Harbor Bass",
            ),
            (
                "tests/fixtures/database/harbor.sqlite",
                super::super::DocumentPosition::DatabaseTable(1),
                "sailings",
            ),
            (
                "../little-clip-core/tests/fixtures/calendar/harbor.ics",
                super::super::DocumentPosition::CalendarEvent(1),
                "Tide watch",
            ),
            (
                "../little-clip-core/tests/fixtures/email/harbor.eml",
                super::super::DocumentPosition::MailAttachment(1),
                "application/octet-stream",
            ),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture);
            let mut viewers = Viewers::default();
            let ctx = Context::default();
            viewers.open_document_at(path, position);
            finish_loading(&mut viewers, &ctx);
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    viewers.render_session(
                        ui,
                        &crate::ui_appearance::palette(
                            little_clip_core::config::WorkbenchAppearance::Dark,
                        ),
                        &mut false,
                    )
                });
            });
            assert!(
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == label)),
                "{fixture}: {:?}",
                viewers.error
            );
            assert!(viewers.request_close());
        }
    }

    #[test]
    fn cancelled_open_cannot_replace_a_new_session() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.txt");
        let second = dir.path().join("second.txt");
        std::fs::write(&first, "first").unwrap();
        std::fs::write(&second, "second").unwrap();
        let ctx = Context::default();
        let mut viewers = Viewers::default();
        viewers.open(first);
        assert!(viewers.request_close());
        viewers.open(second.clone());
        finish_loading(&mut viewers, &ctx);
        assert_eq!(viewers.path.as_ref(), Some(&second));
        assert!(matches!(viewers.session, Some(ViewerSession::Text(_))));
    }

    #[test]
    fn native_image_formats_keep_sources_and_use_bounded_photo_previews() {
        for (extension, mime) in [
            ("JFIF", "image/jpeg"),
            ("SVGZ", "image/svg+xml"),
            ("JXL", "image/jxl"),
            ("JP2", "image/jp2"),
            ("JPF", "image/jpx"),
            ("J2K", "image/x-jp2-codestream"),
            ("PSD", "image/vnd.adobe.photoshop"),
            ("KRA", "application/x-krita"),
            ("PCX", "image/x-pcx"),
        ] {
            let vector = extension == "SVGZ";
            if vector
                && std::process::Command::new("rsvg-convert")
                    .arg("--version")
                    .output()
                    .is_err()
            {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("photo.{extension}"));
            let fixture: Option<&[u8]> = match extension {
                "SVGZ" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/panels.svgz"
                ))),
                "JXL" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/panels.jxl"
                ))),
                "JP2" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/panels.jp2"
                ))),
                "JPF" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/panels.jpf"
                ))),
                "J2K" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/panels.j2k"
                ))),
                "PSD" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/layers.psd"
                ))),
                "KRA" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/layered.kra"
                ))),
                "PCX" => Some(include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/panels.pcx"
                ))),
                _ => None,
            };
            if let Some(bytes) = fixture {
                std::fs::write(&path, bytes).unwrap();
            } else {
                image::RgbImage::from_pixel(64, 32, image::Rgb([40, 80, 120]))
                    .save_with_format(&path, image::ImageFormat::Jpeg)
                    .unwrap();
            }
            let original = std::fs::read(&path).unwrap();
            if extension == "JFIF" {
                assert!(original.windows(5).any(|bytes| bytes == b"JFIF\0"));
            }
            let directory = super::super::filesystem::read_directory(
                dir.path(),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            let entry = directory
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .unwrap();
            assert!(entry.is_photo());
            assert_eq!(little_clip_core::item::guess_image_mime(&path), mime);
            let metadata = super::super::gallery::metadata::probe(&path, entry.revision).unwrap();
            assert_eq!((metadata.width, metadata.height), (64, 32));
            let thumbnail = super::super::gallery::thumbnail::load(
                &path,
                16,
                1,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!((thumbnail.width, thumbnail.height), (16, 8));
            let Loaded::Image(data, _) = load(
                &path,
                &AppConfig::default(),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap() else {
                panic!("Expected native image session")
            };
            let state = ImagePreviewState::from_decoded(
                data,
                Some(path.clone()),
                "Format fixture".into(),
                &Default::default(),
            );
            assert_eq!(state.is_svg, vector);
            assert_eq!(state.current_rgba.dimensions(), (64, 32));
            assert_eq!(std::fs::read(path).unwrap(), original);
        }
    }

    #[test]
    fn unsaved_image_close_requires_discard_and_preserves_source_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255]))
            .save(&path)
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        let Loaded::Image(data, source) = load(
            &path,
            &AppConfig::default(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap() else {
            panic!("image route")
        };
        let mut state = ImagePreviewState::from_decoded(
            data,
            Some(path.clone()),
            "source".into(),
            &AppConfig::default(),
        )
        .with_file_source(source);
        state.mark_pixels_changed();
        let mut viewers = Viewers {
            session: Some(ViewerSession::Image(Box::new(state))),
            path: Some(path.clone()),
            ..Viewers::default()
        };
        assert!(!viewers.request_close());
        assert!(viewers.confirm_close);
        assert!(viewers.active());
        viewers.clear();
        assert!(!viewers.active());
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn oversized_image_is_rejected_before_decode_and_unsupported_routes_externally() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("large.png");
        std::fs::File::create(&image)
            .unwrap()
            .set_len(64 * 1024 * 1024 + 1)
            .unwrap();
        assert!(
            load(
                &image,
                &AppConfig::default(),
                &std::sync::atomic::AtomicBool::new(false)
            )
            .is_err()
        );
        let unknown = dir.path().join("data.unknown");
        std::fs::write(&unknown, [0, 255, 0, 1]).unwrap();
        assert!(matches!(
            load(
                &unknown,
                &AppConfig::default(),
                &std::sync::atomic::AtomicBool::new(false)
            )
            .unwrap(),
            Loaded::External
        ));
    }

    #[test]
    fn office_document_loads_in_file_manager_viewers() {
        let dir = tempfile::tempdir().unwrap();
        let rtf_path = dir.path().join("document.rtf");
        std::fs::write(
            &rtf_path,
            b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Courier;}}\\f0\\fs24 Hello \\b File Manager!\\b0\\par Content paragraph.}",
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut viewers = Viewers::default();
        viewers.open(rtf_path.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while viewers.loading.is_some() {
            viewers.poll(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            matches!(
                viewers.session,
                Some(ViewerSession::Pdf(_)) | Some(ViewerSession::Text(_))
            ),
            "expected Pdf or Text viewer session for office document"
        );
        assert_eq!(viewers.path.as_ref(), Some(&rtf_path));
        assert!(viewers.request_close());
        assert!(!viewers.active());
    }
}
