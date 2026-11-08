//! One file-backed studio at a time; closing returns to the untouched browser.
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

enum Loaded {
    System(super::system::Document),
    Font(super::font_strip::viewer::Viewer),
    Notebook(super::notebook::viewer::Viewer),
    Image(DecodedImageData, FileSource),
    Audio(Option<little_clip_core::audio::AudioMetadata>, FileSource),
    Text(super::text_editor::TextSession),
    Sheet(little_clip_core::presentation::spreadsheet::SpreadsheetData),
    Midi(little_clip_core::presentation::midi::MidiData),
    OfficePdf {
        pdf_path: PathBuf,
        workspace_dir: PathBuf,
        original_path: PathBuf,
    },
    Path,
    Video(super::media::VideoMetadata),
    External,
}
enum ViewerSession {
    System(super::system::Inspector),
    Font(super::font_strip::viewer::Viewer),
    Notebook(super::notebook::viewer::Viewer),
    Image(ImagePreviewState),
    Text(super::text_editor::TextSession),
    Audio(AudioPreviewState),
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
    receiver: mpsc::Receiver<Result<Result<(Loaded, Option<FileSource>), String>, JobError>>,
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
        mpsc::Receiver<Result<Result<FileSource, String>, JobError>>,
    )>,
    rename: rename::Rename,
    renamed: Vec<(PathBuf, PathBuf)>,
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
    pub fn active(&self) -> bool {
        self.loading.is_some() || self.session.is_some() || self.error.is_some()
    }
    pub(super) fn external_error(&mut self, error: String) {
        self.error = Some(error);
    }
    pub fn uses_audio(&self) -> bool {
        matches!(
            self.session,
            Some(ViewerSession::Audio(_) | ViewerSession::Midi(_) | ViewerSession::Video(_))
        ) || self
            .loading
            .as_ref()
            .is_some_and(|l| matches!(kind(&l.path), Kind::Audio | Kind::Midi | Kind::Video))
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
                        .map(|document| (Loaded::System(document), None))
                } else {
                    let identity = FileSource::capture(&source);
                    let loaded = load(&source, &config)?;
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
        self.open(path);
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
            self.source_path = Some(path.clone());
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
                self.session = Some(session(
                    loaded,
                    path,
                    &self.config,
                    self.video_start_seconds,
                    self.source_guard
                        .as_ref()
                        .map(super::search::SourceGuard::read_check)
                        .unwrap_or_default(),
                ))
            }
            Err(error) => self.error = Some(error),
        }
        match (&mut self.session, self.document_start.take()) {
            (
                Some(ViewerSession::Notebook(state)),
                Some(super::DocumentPosition::NotebookCell(cell)),
            ) => state.select(cell),
            (Some(ViewerSession::Pdf(state)), Some(super::DocumentPosition::PdfPage(page))) => {
                state.selected_page = Some(page.max(1))
            }
            (Some(ViewerSession::Text(state)), Some(super::DocumentPosition::TextLine(line))) => {
                state.preview.view_mode = crate::text_preview::TextViewMode::Raw;
                state.preview.center_source_line = Some(line);
            }
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
        self.gallery_controls(ui);
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
        self.rename_ui(ui);
        self.poll_audio_return();
        if self.session.is_none() {
            close |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        }
        if close {
            self.request_close();
        }
        self.close_confirmation(ui);
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

    fn close_confirmation(&mut self, ui: &mut Ui) {
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
                        && ui.button("Save copy and replace pad").clicked()
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
                        if ui.button("Keep editing").clicked() {
                            self.confirm_close = false;
                        }
                        if ui
                            .button(if self.pad_audio {
                                "Return without replacing"
                            } else {
                                "Discard edits"
                            })
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
#[derive(Clone, Copy)]
enum Kind {
    Notebook,
    Font,
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
    if super::text_strip::is_typescript_source(path)
        || little_clip_core::subtitles::is_subtitle_path(path)
    {
        Kind::Text
    } else if let Some(kind) = extension_kind(path) {
        kind
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
fn extension_kind(path: &Path) -> Option<Kind> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "ttf" | "ttc" | "otf" | "otc" | "woff" | "woff2" => Some(Kind::Font),
        "mid" | "midi" => Some(Kind::Midi),
        "ipynb" => Some(Kind::Notebook),
        _ => None,
    }
}
fn load(path: &Path, config: &AppConfig) -> Result<Loaded, String> {
    if little_clip_core::kernel_interface::classify(path)
        .map_err(|e| e.to_string())?
        .semantic()
    {
        return super::system::collect(path, &std::sync::atomic::AtomicBool::new(false))
            .map(Loaded::System);
    }
    little_clip_core::kernel_interface::require_content(path).map_err(|e| e.to_string())?;
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Only regular files can be inspected".into());
    }
    let kind = kind(path);
    let limit = match kind {
        Kind::Image => 64 * 1024 * 1024,
        Kind::Sheet | Kind::Midi | Kind::Notebook => 32 * 1024 * 1024,
        _ => u64::MAX,
    };
    if metadata.len() > limit {
        return Err(format!(
            "File exceeds the {} MiB opening limit; use Open externally.",
            limit / (1024 * 1024)
        ));
    }
    match kind {
        Kind::Notebook => super::notebook::viewer::Viewer::load(path).map(Loaded::Notebook),
        Kind::Font => super::font_strip::viewer::Viewer::load(path).map(Loaded::Font),
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
        Kind::OfficeDoc => {
            let store =
                little_clip_core::staging::ArtifactStore::new().map_err(|e| e.to_string())?;
            let workspace = store
                .create_workspace("office_preview", "pdf_render")
                .map_err(|e| e.to_string())?;
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("document");
            let pdf_path = workspace.path().join(format!("{stem}.pdf"));
            if let Ok(cached_pdf) = little_clip_core::document::ensure_rendered_pdf(path) {
                if std::fs::copy(&cached_pdf, &pdf_path).is_ok()
                    && std::fs::metadata(&pdf_path)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false)
                {
                    let workspace_dir = workspace.retain();
                    return Ok(Loaded::OfficePdf {
                        pdf_path,
                        workspace_dir,
                        original_path: path.to_path_buf(),
                    });
                }
            }
            if little_clip_core::document::render_to_pdf(path, &pdf_path).is_ok()
                && std::fs::metadata(&pdf_path)
                    .map(|m| m.len() > 0)
                    .unwrap_or(false)
            {
                let workspace_dir = workspace.retain();
                Ok(Loaded::OfficePdf {
                    pdf_path,
                    workspace_dir,
                    original_path: path.to_path_buf(),
                })
            } else {
                let md = little_clip_core::document::render_to_markdown(path)?;
                let text_session = super::text_editor::TextSession::from_markdown(path, md, config);
                Ok(Loaded::Text(text_session))
            }
        }
        Kind::Text => super::text_editor::TextSession::load(path, config).map(Loaded::Text),
        Kind::Video => video::prepare(path).map(Loaded::Video),
        Kind::External => Ok(Loaded::External),
        _ => FileSource::capture(path).map(|_| Loaded::Path),
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
        Loaded::Notebook(viewer) => ViewerSession::Notebook(viewer),
        Loaded::Font(viewer) => ViewerSession::Font(viewer),
        Loaded::System(document) => ViewerSession::System(super::system::Inspector::new(document)),
        Loaded::Image(data, source) => ViewerSession::Image(
            ImagePreviewState::from_decoded(
                data,
                Some(path.clone()),
                super::display_path(&path),
                config,
            )
            .with_file_source(source),
        ),
        Loaded::Video(prepared) => {
            ViewerSession::Video(video::Video::new(path, prepared, video_start))
        }
        Loaded::Text(state) => ViewerSession::Text(state),
        Loaded::Sheet(data) => ViewerSession::Sheet(SpreadsheetViewerState::new(data, Some(path))),
        Loaded::Midi(data) => ViewerSession::Midi(MidiViewerState::from_parsed_file(path, data)),
        Loaded::OfficePdf {
            pdf_path,
            workspace_dir,
            original_path,
        } => {
            let title = original_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "document".to_string());
            ViewerSession::Pdf(
                PdfEditorState::new(pdf_path)
                    .with_title(title)
                    .with_cleanup_dir(workspace_dir)
                    .with_protected_outputs(),
            )
        }
        Loaded::Audio(metadata, source) => ViewerSession::Audio(
            AudioPreviewState::new_with_metadata_checked(
                path.clone(),
                super::display_path(&path),
                metadata,
                check,
            )
            .with_editor(config.audio.editor.clone())
            .with_file_source(source),
        ),
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
            load(&path, &AppConfig::default()).unwrap(),
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
            load(&path, &AppConfig::default()).unwrap(),
            Loaded::Font(_)
        ));
    }

    #[test]
    fn compressed_web_fonts_open_in_the_specimen_viewer() {
        for extension in ["woff", "woff2"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/fonts/latin.{extension}"));
            assert!(matches!(
                load(&path, &AppConfig::default()).unwrap(),
                Loaded::Font(_)
            ));
        }
    }

    #[test]
    fn clicked_subtitle_cue_survives_background_loading() {
        for extension in ["srt", "vtt", "ass"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tests/fixtures/subtitles/sample.{extension}"));
            let mut viewers = Viewers::default();
            let ctx = Context::default();
            viewers.open_document_at(path, super::super::DocumentPosition::SubtitleCue(1));
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
            assert!(output.shapes.iter().any(
            |shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Cue 2")
        ));
            assert!(viewers.request_close());
        }
    }

    #[test]
    fn clicked_notebook_cell_survives_background_loading() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/notebooks/sample.ipynb");
        let mut viewers = Viewers::default();
        let ctx = Context::default();
        viewers.open_document_at(path, super::super::DocumentPosition::NotebookCell(2));
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
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Code cell 3")));
        assert!(viewers.request_close());
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
    fn native_jfif_and_svgz_keep_sources_and_use_bounded_photo_previews() {
        for (extension, mime) in [("JFIF", "image/jpeg"), ("SVGZ", "image/svg+xml")] {
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
            if vector {
                std::fs::write(
                    &path,
                    include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../little-clip-core/tests/fixtures/panels.svgz"
                    )),
                )
                .unwrap();
            } else {
                image::RgbImage::from_pixel(64, 32, image::Rgb([40, 80, 120]))
                    .save_with_format(&path, image::ImageFormat::Jpeg)
                    .unwrap();
            }
            let original = std::fs::read(&path).unwrap();
            if !vector {
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
            let Loaded::Image(data, _) = load(&path, &AppConfig::default()).unwrap() else {
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
        let Loaded::Image(data, source) = load(&path, &AppConfig::default()).unwrap() else {
            panic!("image route")
        };
        let mut state = ImagePreviewState::from_decoded(
            data,
            Some(path.clone()),
            "source".into(),
            &AppConfig::default(),
        )
        .with_file_source(source);
        state.pixel_revision += 1;
        let mut viewers = Viewers {
            session: Some(ViewerSession::Image(state)),
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
        assert!(load(&image, &AppConfig::default()).is_err());
        let unknown = dir.path().join("data.unknown");
        std::fs::write(&unknown, [0, 255, 0, 1]).unwrap();
        assert!(matches!(
            load(&unknown, &AppConfig::default()).unwrap(),
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
