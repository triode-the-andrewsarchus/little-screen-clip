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

mod gallery;
mod video;

enum Loaded {
    Image(DecodedImageData, FileSource),
    Text(super::text_editor::TextSession),
    Sheet(little_clip_core::presentation::spreadsheet::SpreadsheetData),
    Midi(little_clip_core::presentation::midi::MidiData),
    Path(FileSource),
    Video(super::media::VideoMetadata),
    External,
}
enum ViewerSession {
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
    ticket: JobTicket,
    path: PathBuf,
    receiver: mpsc::Receiver<Result<Result<Loaded, String>, JobError>>,
}
impl Drop for Loading {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}
#[derive(Default)]
pub(super) struct Viewers {
    loading: Option<Loading>,
    session: Option<ViewerSession>,
    path: Option<PathBuf>,
    error: Option<String>,
    confirm_close: bool,
    config: AppConfig,
    gallery: Option<gallery::Collection>,
    video_start_seconds: f64,
    text_start_line: Option<usize>,
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
    pub fn uses_audio(&self) -> bool {
        matches!(
            self.session,
            Some(ViewerSession::Audio(_) | ViewerSession::Midi(_) | ViewerSession::Video(_))
        ) || self
            .loading
            .as_ref()
            .is_some_and(|l| matches!(kind(&l.path), Kind::Audio | Kind::Midi | Kind::Video))
    }
    pub fn open(&mut self, path: PathBuf) {
        if self.active() {
            return;
        }
        self.video_start_seconds = 0.0;
        self.text_start_line = None;
        let source = path.clone();
        let config = self.config.clone();
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::from_secs(120), move || load(&source, &config));
        self.loading = Some(Loading {
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
    pub fn open_text_at(&mut self, path: PathBuf, line: usize) {
        if self.active() {
            return;
        }
        self.open(path);
        self.text_start_line = Some(line);
    }
    fn poll(&mut self, ctx: &Context) {
        let Some(loading) = &self.loading else { return };
        let Some(result) = super::poll_job(&loading.receiver) else {
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        };
        let path = self.loading.take().unwrap().path.clone();
        self.path = Some(path.clone());
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
                ))
            }
            Err(error) => self.error = Some(error),
        }
        if let Some(ViewerSession::Text(state)) = &mut self.session
            && let Some(line) = self.text_start_line.take()
        {
            state.preview.view_mode = crate::text_preview::TextViewMode::Raw;
            state.preview.center_source_line = Some(line);
        }
    }
    pub fn ui(&mut self, ui: &mut Ui, palette: &AppPalette) -> bool {
        self.poll(ui.ctx());
        self.gallery_controls(ui);
        ui.set_enabled(!self.confirm_close);
        let mut close = false;
        ui.horizontal(|ui| {
            close = ui.button("Back to folder").clicked();
            if self.path.is_some() && ui.button("Open externally").clicked() {
                self.open_external();
            }
            let path = self
                .path
                .as_ref()
                .or_else(|| self.loading.as_ref().map(|job| &job.path))
                .map_or_else(String::new, |path| super::display_path(path));
            ui.add(egui::Label::new(&path).truncate(true))
                .on_hover_text(path);
        });
        ui.separator();
        if self.loading.is_some() {
            ui.spinner();
            ui.label("Opening file…");
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if let Some(viewer) = &mut self.session {
            render(viewer, ui, palette, &mut close);
            match viewer {
                ViewerSession::Image(state) => self.path = state.path.clone(),
                ViewerSession::Text(state) => self.path = Some(state.path().to_path_buf()),
                ViewerSession::Pdf(state) => self.path = Some(state.path.clone()),
                _ => {}
            }
        }
        if self.session.is_none() {
            close |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        }
        if close {
            self.request_close();
        }
        if self.confirm_close {
            egui::Window::new("Unsaved edits")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label("Save your changes before returning to the folder.");
                    ui.horizontal(|ui| {
                        if ui.button("Keep editing").clicked() {
                            self.confirm_close = false;
                        }
                        if ui.button("Discard edits").clicked() {
                            self.clear();
                            self.gallery = None;
                        }
                    });
                });
        }
        !self.active()
    }
    pub fn request_close(&mut self) -> bool {
        self.pause_slideshow();
        if let Some(ViewerSession::Pdf(state)) = &self.session
            && state.file_write_pending()
        {
            self.error = Some("Wait for PDF editing to finish.".into());
            return false;
        }
        if let Some(ViewerSession::Audio(state)) = &self.session {
            if state.file_write_pending() {
                self.error = Some("Wait for audio export to finish.".into());
                return false;
            }
            if state.has_session_edits() {
                self.confirm_close = true;
                return false;
            }
        }
        if let Some(ViewerSession::Text(state)) = &self.session {
            if state.busy() {
                self.error = Some("Wait for text save to finish.".into());
                return false;
            }
            if state.dirty() {
                self.confirm_close = true;
                return false;
            }
        }
        if let Some(ViewerSession::Image(state)) = &mut self.session {
            if state.poll_cached_jobs() {
                self.error = Some("Wait for image save/copy to finish before closing.".into());
                return false;
            }
            if state.has_unsaved_changes() {
                self.confirm_close = true;
                return false;
            }
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
        self.loading = None;
        self.path = None;
        self.error = None;
        self.confirm_close = false;
        self.video_start_seconds = 0.0;
    }
    fn open_external(&mut self) {
        if let Some(ViewerSession::Video(state)) = &mut self.session {
            state.stop();
        }
        if let Some(path) = &self.path {
            if let Err(error) = external(path.clone()) {
                self.error = Some(error);
            }
        }
    }
}
#[derive(Clone, Copy)]
enum Kind {
    Image,
    Audio,
    Pdf,
    Sheet,
    Midi,
    Scene,
    Model,
    Text,
    Video,
    External,
}
fn kind(path: &Path) -> Kind {
    if super::text_strip::is_typescript_source(path) {
        Kind::Text
    } else if little_clip_core::item::is_image_path(path) {
        Kind::Image
    } else if little_clip_core::audio::is_audio_path(path) {
        Kind::Audio
    } else if little_clip_core::is_video_path(path) {
        Kind::Video
    } else if little_clip_core::pdf::is_pdf_path(path) {
        Kind::Pdf
    } else if little_clip_core::presentation::spreadsheet::is_spreadsheet_file(path) {
        Kind::Sheet
    } else if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
    {
        Kind::Midi
    } else if little_clip_scene::is_splat_file(path) {
        Kind::Scene
    } else if little_clip_core::presentation::model3d::is_3d_model_file(path) {
        Kind::Model
    } else if little_clip_ops::is_text_file(path) {
        Kind::Text
    } else {
        Kind::External
    }
}
fn load(path: &Path, config: &AppConfig) -> Result<Loaded, String> {
    let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Only regular files can be inspected".into());
    }
    let kind = kind(path);
    let limit = match kind {
        Kind::Image => 64 * 1024 * 1024,
        Kind::Sheet | Kind::Midi => 32 * 1024 * 1024,
        _ => u64::MAX,
    };
    if metadata.len() > limit {
        return Err(format!(
            "File exceeds the {} MiB opening limit; use Open externally.",
            limit / (1024 * 1024)
        ));
    }
    match kind {
        Kind::Image => {
            let source = FileSource::capture(path)?;
            let decoded = DecodedImageData::from_path_bounded(path, 64 * 1024 * 1024)?;
            if FileSource::capture(path)? != source {
                return Err("Source changed while opening; open it again.".into());
            }
            Ok(Loaded::Image(decoded, source))
        }
        Kind::Sheet => {
            little_clip_core::presentation::spreadsheet::load_spreadsheet(path).map(Loaded::Sheet)
        }
        Kind::Midi => {
            little_clip_core::presentation::midi::MidiData::parse_file(path).map(Loaded::Midi)
        }
        Kind::Text => super::text_editor::TextSession::load(path, config).map(Loaded::Text),
        Kind::Video => video::prepare(path).map(Loaded::Video),
        Kind::External => Ok(Loaded::External),
        _ => FileSource::capture(path).map(Loaded::Path),
    }
}
fn session(loaded: Loaded, path: PathBuf, config: &AppConfig, video_start: f64) -> ViewerSession {
    match loaded {
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
        Loaded::Path(source) if little_clip_core::audio::is_audio_path(&path) => {
            ViewerSession::Audio(
                AudioPreviewState::new(path.clone(), super::display_path(&path))
                    .with_editor(config.audio.editor.clone())
                    .with_file_source(source),
            )
        }
        _ if little_clip_core::pdf::is_pdf_path(&path) => {
            ViewerSession::Pdf(PdfEditorState::new(path).with_protected_outputs())
        }
        _ if little_clip_scene::is_splat_file(&path) => ViewerSession::Scene(
            SplatViewerState::from_file(path).with_config(config.model3d.clone()),
        ),
        _ => ViewerSession::Model(
            Model3dViewerState::from_file(path).with_config(config.model3d.clone()),
        ),
    }
}
fn render(viewer: &mut ViewerSession, ui: &mut Ui, palette: &AppPalette, close: &mut bool) {
    let ctx = ui.ctx().clone();
    match viewer {
        ViewerSession::Image(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Text(state) => state.ui(ui, palette, close),
        ViewerSession::Audio(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Pdf(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Sheet(state) => state.render_ui(ui, &ctx, palette, close),
        ViewerSession::Midi(state) => state.render_ui(ui, palette, close),
        ViewerSession::Model(state) => state.render_ui(ui, palette, close),
        ViewerSession::Scene(state) => state.render_ui(ui, palette, close),
        ViewerSession::Video(state) => state.ui(ui, close),
    }
}
pub(super) fn external(path: PathBuf) -> Result<(), String> {
    std::thread::Builder::new()
        .name("file-external-launch".into())
        .spawn(move || {
            if let Ok(mut child) = std::process::Command::new("xdg-open").arg(path).spawn() {
                let _ = child.wait();
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while viewers.loading.is_some() {
            viewers.poll(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(viewers.path.as_ref(), Some(&second));
        assert!(matches!(viewers.session, Some(ViewerSession::Text(_))));
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
}
