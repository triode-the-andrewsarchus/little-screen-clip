//! Small UTF-8 file editor with explicit, guarded source writes.
use crate::{
    file_source::FileSource,
    text_preview::TextPreviewState,
    ui_tabs::{Tab, TabBar},
};
use clip_ui::buttons::ButtonRole;
use little_clip_core::{BackgroundJobs, JobError, palette::AppPalette};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Preview,
    Edit,
    Diagram,
}
/// Owns a guarded UTF-8 source editor and its live previews.
pub struct TextSession {
    pub preview: TextPreviewState,
    close: Option<CloseRequest>,
    mode: Mode,
    editor: Option<Editor>,
    diagram: Option<(
        little_clip_core::diagram::Format,
        crate::diagram_preview::Preview,
    )>,
}
#[derive(Default)]
struct CloseRequest {
    requested: bool,
    discard: bool,
    window: bool,
}

struct SavedText {
    source: FileSource,
    path: PathBuf,
    text: String,
    preview: Option<TextPreviewState>,
}
enum Delivery {
    Saved(Box<SavedText>),
    Copied {
        text: String,
        preview: Option<Box<TextPreviewState>>,
    },
}

struct Editor {
    text: String,
    saved: String,
    source: Option<FileSource>,
    path: PathBuf,
    name: String,
    save_as: bool,
    directory: String,
    error: Option<String>,
    config: little_clip_core::AppConfig,
    pending: Option<mpsc::Receiver<Result<Result<Delivery, String>, JobError>>>,
}
impl TextSession {
    pub fn load(path: &Path, config: &little_clip_core::AppConfig) -> Result<Self, String> {
        let preview =
            TextPreviewState::from_file(path, config).ok_or("Could not read text preview")?;
        let editor = if std::fs::metadata(path).map_err(|e| e.to_string())?.len() <= 2 * 1024 * 1024
        {
            let source = FileSource::capture(path)?;
            let bytes = crate::file_source::read_file_bytes(path, 2 * 1024 * 1024, true, true)
                .map_err(|e| e.to_string())?;
            String::from_utf8(bytes).ok().map(|text| {
                let name = little_clip_core::diagram::Format::from_source_path(path, &text)
                    .and_then(|_| path.extension().and_then(|extension| extension.to_str()))
                    .map_or_else(
                        || "edited.txt".into(),
                        |extension| format!("edited.{extension}"),
                    );
                Editor {
                    saved: text.clone(),
                    text,
                    source: Some(source),
                    path: path.to_path_buf(),
                    name,
                    save_as: false,
                    directory: little_clip_core::path_address::display_path(
                        path.parent().unwrap_or(Path::new(".")),
                    ),
                    error: None,
                    config: config.clone(),
                    pending: None,
                }
            })
        } else {
            None
        };
        if let Some(editor) = &editor
            && Some(FileSource::capture(path)?) != editor.source
        {
            return Err("Text changed while opening; open it again.".into());
        }
        let source = editor
            .as_ref()
            .map_or(preview.content.as_str(), |editor| editor.text.as_str());
        let format = little_clip_core::diagram::Format::from_source_path(path, source);
        Ok(Self {
            preview,
            close: None,
            mode: if format.is_some() {
                Mode::Diagram
            } else {
                Mode::Preview
            },
            editor,
            diagram: format.map(|format| (format, Default::default())),
        })
    }
    /// Creates an unsaved source session for a conservatively recognized snippet.
    ///
    /// The exact source is retained, including its BOM and line endings.
    /// Unrecognized text returns `None` for the host's ordinary text viewer.
    pub fn from_clipboard(
        content: std::sync::Arc<String>,
        title: Option<String>,
        config: &little_clip_core::AppConfig,
    ) -> Option<Self> {
        let format = little_clip_core::diagram::Format::from_snippet(&content)?;
        let extension = format.source_extension()?;
        let preview = TextPreviewState::from_data(
            crate::text_preview::TextPreviewData::from_source(content.clone(), Some(extension)),
            title,
            config,
        );
        Some(Self {
            preview,
            close: Some(Default::default()),
            mode: Mode::Diagram,
            diagram: Some((format, Default::default())),
            editor: Some(Editor {
                text: content.as_str().to_owned(),
                saved: content.as_str().to_owned(),
                source: None,
                path: PathBuf::from(format!("clipboard.{extension}")),
                name: format!("clipboard.{extension}"),
                save_as: false,
                directory: little_clip_core::path_address::display_path(
                    &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                ),
                error: None,
                config: config.clone(),
                pending: None,
            }),
        })
    }

    /// Requests leaving without discarding a draft or accepted write.
    ///
    /// A `false` result requires retaining this owner. Its UI offers save,
    /// discard and cancel after any accepted write finishes.
    pub fn request_close(&mut self) -> bool {
        if self.busy() {
            return false;
        }
        if !self.dirty() || self.close.as_ref().is_some_and(|close| close.discard) {
            return true;
        }
        self.close.get_or_insert_default().requested = true;
        false
    }

    pub fn from_markdown(
        path: &Path,
        markdown: String,
        config: &little_clip_core::AppConfig,
    ) -> Self {
        let title = path.file_name().map(|n| n.to_string_lossy().to_string());
        let mut preview = TextPreviewState::from_text(markdown, title, config);
        preview.path = Some(path.to_path_buf());
        preview.view_mode = crate::text_preview::TextViewMode::RenderedMarkdown;
        Self {
            preview,
            close: None,
            mode: Mode::Preview,
            editor: None,
            diagram: None,
        }
    }
    pub fn path(&self) -> &Path {
        self.editor.as_ref().map_or_else(
            || self.preview.path.as_deref().unwrap_or(Path::new(".")),
            |editor| editor.path.as_path(),
        )
    }
    pub fn file_source(&self) -> Option<FileSource> {
        self.editor
            .as_ref()
            .and_then(|editor| editor.source.clone())
    }
    pub fn relocate_file(&mut self, path: PathBuf, source: FileSource) {
        self.preview.title = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.preview.path = Some(path.clone());
        if let Some(editor) = &mut self.editor {
            editor.path = path;
            editor.source = Some(source);
        }
    }
    pub fn dirty(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.text != editor.saved)
    }
    pub fn busy(&self) -> bool {
        self.editor
            .as_ref()
            .is_some_and(|editor| editor.pending.is_some())
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        let was_busy = self.busy();
        if self.close.is_some()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            if let Some(editor) = &mut self.editor
                && editor.save_as
            {
                editor.save_as = false;
            } else if self.close.as_ref().is_some_and(|request| request.requested) {
                self.close = Some(Default::default());
            } else {
                *close = true;
            }
        }
        self.render_editor(ui, palette, close);
        if self.close.is_some() {
            // A close requested during delivery must not hide its failure, even
            // when the original clipboard source itself had no unsaved changes.
            if was_busy
                && !self.busy()
                && self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.error.is_some())
            {
                self.close = Some(Default::default());
                *close = false;
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else {
                self.service_close(ui, palette, close);
            }
        }
    }

    fn service_close(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        let window = ui.ctx().input(|input| input.viewport().close_requested());
        if let Some(request) = &mut self.close {
            request.window |= window;
            request.requested |= window || *close;
        }
        if self.busy() {
            *close = false;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx().request_repaint_after(Duration::from_millis(30));
            return;
        }
        if !self.close.as_ref().is_some_and(|request| request.requested) {
            return;
        }
        if self.request_close() {
            *close = true;
            if self.close.as_ref().is_some_and(|request| request.window) {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        *close = false;
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if !self.editor.as_ref().is_some_and(|editor| editor.save_as) {
            self.discard_dialog(ui.ctx(), palette);
        }
    }

    fn discard_dialog(&mut self, ctx: &egui::Context, palette: &AppPalette) {
        egui::Window::new("Keep source changes?")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Save or copy your edited source before leaving, or discard the changes.");
                ui.horizontal(|ui| {
                    if ui
                        .add(ButtonRole::Primary.button("Save…", palette))
                        .clicked()
                        && let Some(editor) = &mut self.editor
                    {
                        editor.request_save();
                    }
                    if ui
                        .add(ButtonRole::Destructive.button("Discard", palette))
                        .clicked()
                        && let Some(request) = &mut self.close
                    {
                        request.discard = true;
                    }
                    if ui
                        .add(ButtonRole::Secondary.button("Keep editing", palette))
                        .clicked()
                    {
                        self.close = Some(Default::default());
                    }
                });
            });
    }

    fn render_editor(&mut self, ui: &mut egui::Ui, palette: &AppPalette, close: &mut bool) {
        let ctx = ui.ctx().clone();
        if let Some(editor) = &mut self.editor {
            if let Some(preview) = editor.poll(&ctx) {
                self.preview = preview;
            }
            let mut tabs = vec![
                Tab::new(Mode::Preview, "Preview"),
                Tab::new(Mode::Edit, "Edit text"),
            ];
            if self.diagram.is_some() {
                tabs.insert(0, Tab::new(Mode::Diagram, "Diagram"));
            }
            if let Some(mode) = TabBar::new("file_text_modes", self.mode, &tabs, palette).show(ui) {
                self.mode = mode;
            }
            if self.mode != Mode::Diagram
                && let Some((_, preview)) = &mut self.diagram
            {
                preview.suspend();
            }
            if matches!(self.mode, Mode::Edit | Mode::Diagram) {
                self.preview.cancel_diagram_previews();
                let mut shell = crate::shell::StudioShell::new(
                    editor
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    crate::icons::Icon::Text,
                    palette,
                    close,
                );
                shell.render_header(ui, |_| {});
                let syntax_format = self.diagram.as_ref().map(|(format, _)| *format);
                let diagram = if self.mode == Mode::Diagram {
                    self.diagram.as_mut()
                } else {
                    None
                };
                editor.ui(ui, palette, syntax_format, diagram);
                return;
            }
        }
        if self.dirty() {
            ui.label(
                if self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.source.is_some())
                {
                    "Preview shows the last saved version. Edit text shows your draft."
                } else {
                    "Preview shows the last copied version. Edit text shows your draft."
                },
            );
        }
        self.preview.render_ui(ui, &ctx, palette, close);
    }
}
impl Editor {
    fn poll(&mut self, ctx: &egui::Context) -> Option<TextPreviewState> {
        let receiver = self.pending.as_ref()?;
        let Some(result) = little_clip_core::jobs::task::poll(receiver, None).map(|result| {
            result
                .map_err(|error| error.to_string())
                .and_then(|result| result)
        }) else {
            ctx.request_repaint_after(Duration::from_millis(40));
            return None;
        };
        self.pending = None;
        match result {
            Ok(Delivery::Saved(saved)) => {
                self.source = Some(saved.source);
                self.path = saved.path;
                self.saved = saved.text;
                self.error = None;
                saved.preview
            }
            Ok(Delivery::Copied { text, preview }) => {
                if self.source.is_none() {
                    self.saved = text;
                }
                self.error = None;
                preview.map(|preview| *preview)
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
    fn copy_source(&mut self, copy: impl FnOnce(&str) -> Result<(), String> + Send + 'static) {
        if self.pending.is_some() {
            return;
        }
        let text = self.text.clone();
        let clipboard = self.source.is_none();
        let path = self.path.clone();
        let config = self.config.clone();
        let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            copy(&text)?;
            let preview = clipboard.then(|| {
                Box::new(TextPreviewState::from_data(
                    crate::text_preview::TextPreviewData::from_source(
                        std::sync::Arc::new(text.clone()),
                        path.extension().and_then(|extension| extension.to_str()),
                    ),
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned()),
                    &config,
                ))
            });
            Ok(Delivery::Copied { text, preview })
        });
        self.pending = Some(receiver);
    }

    fn request_save(&mut self) {
        if self.source.is_some() {
            self.save(self.path.clone());
        } else {
            self.save_as = true;
        }
    }

    fn save(&mut self, path: PathBuf) {
        if self.pending.is_some() {
            return;
        }
        if self.text.len() > 2 * 1024 * 1024 {
            self.error = Some("Text exceeds the 2 MiB editor limit.".into());
            return;
        }
        let text = self.text.clone();
        let source = self.source.clone();
        let config = self.config.clone();
        let (_, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
            let source = if let Some(source) = source {
                source.write(&path, text.as_bytes())?
            } else {
                let directory = little_clip_core::kernel_interface::pin_content_directory(
                    path.parent().unwrap_or(Path::new(".")),
                )
                .map_err(|error| error.to_string())?;
                let installed = little_clip_core::file_source::create_file_in(
                    &path,
                    text.as_bytes(),
                    &directory,
                )?;
                FileSource::capture_file(&installed)?
            };
            let path = source.path().to_path_buf();
            let preview = TextPreviewState::from_file(&path, &config);
            Ok(Delivery::Saved(Box::new(SavedText {
                source,
                path,
                text,
                preview,
            })))
        });
        self.pending = Some(receiver);
    }
    fn source_ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        format: Option<little_clip_core::diagram::Format>,
    ) {
        let syntax = format
            .and_then(|format| crate::text_preview::SYNTAX_SET.find_syntax_by_name(format.name()))
            .unwrap_or_else(|| crate::text_preview::SYNTAX_SET.find_syntax_plain_text());
        let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
            let mut job =
                crate::text_preview::build_highlighted_layout_job(text, syntax, palette, &[]);
            job.wrap.max_width = wrap_width;
            ui.fonts(|fonts| fonts.layout_job(job))
        };
        let highlight = format.is_some() && self.text.len() <= 256 * 1024;
        egui::ScrollArea::both()
            .id_source("diagram_source_editor")
            .show(ui, |ui| {
                let mut edit = egui::TextEdit::multiline(&mut self.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(24);
                if highlight {
                    edit = edit.layouter(&mut layouter);
                }
                ui.add(edit);
            });
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        syntax_format: Option<little_clip_core::diagram::Format>,
        diagram: Option<&mut (
            little_clip_core::diagram::Format,
            crate::diagram_preview::Preview,
        )>,
    ) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    ButtonRole::Primary.button("Save", palette),
                )
                .clicked()
            {
                self.request_save();
            }
            if ui
                .add(ButtonRole::Secondary.button("Save As…", palette))
                .clicked()
            {
                self.save_as = true;
            }
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    ButtonRole::Secondary.button("Copy source", palette),
                )
                .clicked()
            {
                self.copy_source(|text| {
                    little_clip_core::copy_text_to_system_clipboard(text)
                        .map_err(|error| error.to_string())
                });
            }
            if self.text != self.saved {
                ui.label("Unsaved changes");
            }
            if self.pending.is_some() {
                ui.spinner();
            }
        });
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.request_save();
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if let Some((format, preview)) = diagram {
            ui.columns(2, |columns| {
                self.source_ui(&mut columns[0], palette, syntax_format);
                egui::ScrollArea::vertical()
                    .id_source("diagram_live_preview")
                    .show(&mut columns[1], |ui| {
                        preview.show(ui, *format, &self.text);
                    });
            });
        } else {
            self.source_ui(ui, palette, syntax_format);
        }
        if self.save_as {
            egui::Window::new("Save source to a new file")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label("Existing destination directory");
                    ui.text_edit_singleline(&mut self.directory);
                    ui.label("Choose a new, unused filename.");
                    ui.text_edit_singleline(&mut self.name);
                    ui.horizontal(|ui| {
                        if ui
                            .add(ButtonRole::Primary.button("Create file", palette))
                            .clicked()
                        {
                            match little_clip_core::path_address::decode_path(&self.directory)
                                .and_then(|directory| {
                                    little_clip_core::path_address::decode_file_name(&self.name)
                                        .map(|name| directory.join(name))
                                }) {
                                Ok(path) => {
                                    self.save(path);
                                    self.save_as = false;
                                }
                                Err(error) => self.error = Some(error),
                            }
                        }
                        if ui
                            .add(ButtonRole::Secondary.button("Cancel", palette))
                            .clicked()
                        {
                            self.save_as = false;
                        }
                    });
                });
        }
    }
}

/// Supplies an owned copy gate for host lifecycle tests without using a clipboard.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn hold_clipboard_copy_for_test(session: &mut TextSession) -> mpsc::Sender<()> {
    let (release, wait) = mpsc::channel();
    session
        .editor
        .as_mut()
        .expect("source fixture is editable")
        .copy_source(move |_| {
            wait.recv_timeout(Duration::from_secs(10))
                .map_err(|error| error.to_string())
        });
    release
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_save(editor: &mut Editor, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while editor.pending.is_some() {
            editor.poll(ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn clipboard_graph(draft: &str) -> TextSession {
        let mut session = TextSession::from_clipboard(
            std::sync::Arc::new("digraph { a -> b }".into()),
            None,
            &Default::default(),
        )
        .unwrap();
        session.editor.as_mut().unwrap().text = draft.into();
        session
    }

    fn editor_frame(
        context: &egui::Context,
        session: &mut TextSession,
        close: &mut bool,
        input: egui::RawInput,
    ) -> egui::FullOutput {
        context.run(input, |context| {
            egui::CentralPanel::default()
                .show(context, |ui| session.ui(ui, &Default::default(), close));
        })
    }

    #[test]
    fn clipboard_diagrams_retain_exact_source_and_first_save_requires_a_destination() {
        let source = std::sync::Arc::new("\u{feff}digraph { a -> b }\r\n".to_owned());
        let mut session = TextSession::from_clipboard(
            source.clone(),
            Some("Clipboard graph".into()),
            &Default::default(),
        )
        .unwrap();
        assert!(session.preview.path.is_none());
        assert!(session.file_source().is_none());
        assert_eq!(session.preview.content.as_str(), source.as_str());
        assert!(session.mode == Mode::Diagram);
        let editor = session.editor.as_mut().unwrap();
        assert_eq!(&editor.text, source.as_ref());
        assert_eq!(editor.name, "clipboard.gv");
        editor.text = "digraph { a -> c }".into();
        editor.request_save();
        assert!(editor.save_as);
        assert!(editor.pending.is_none());
        assert!(session.dirty());
        assert!(!session.request_close());
        assert!(session.close.as_ref().unwrap().requested);
        assert!(
            TextSession::from_clipboard(
                std::sync::Arc::new("ordinary text".into()),
                None,
                &Default::default()
            )
            .is_none()
        );
    }

    #[test]
    fn clipboard_first_save_is_atomic_no_overwrite_and_then_tracks_file_identity() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("graph.gv");
        let mut session = clipboard_graph("digraph { a -> b }");
        let editor = session.editor.as_mut().unwrap();
        editor.text = "digraph { a -> c }".into();
        std::fs::write(&destination, "existing source").unwrap();
        editor.save(destination.clone());
        wait_for_save(editor, &egui::Context::default());
        assert!(editor.error.is_some());
        assert!(editor.source.is_none());
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "existing source"
        );
        let unused = directory.path().join("new.gv");
        editor.save(unused.clone());
        assert!(session.busy());
        assert!(!session.request_close());
        let editor = session.editor.as_mut().unwrap();
        wait_for_save(editor, &egui::Context::default());
        assert!(editor.error.is_none(), "{:?}", editor.error);
        assert!(editor.source.is_some());
        assert_eq!(editor.path, unused);
        assert_eq!(
            std::fs::read_to_string(&unused).unwrap(),
            "digraph { a -> c }"
        );
        assert!(!session.dirty());
        assert!(session.request_close());
    }

    #[test]
    fn clipboard_copy_delivers_owned_snapshots_and_retains_newer_edits_until_close() {
        let mut session = clipboard_graph("digraph { a -> b }");
        let captured = "digraph { a -> c }";
        session.editor.as_mut().unwrap().text = captured.into();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        session.editor.as_mut().unwrap().copy_source(move |text| {
            started_tx.send(text.to_owned()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        });
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            captured
        );
        session
            .editor
            .as_mut()
            .unwrap()
            .text
            .push_str(" // newer draft");
        assert!(session.busy());
        assert!(!session.request_close());
        let context = egui::Context::default();
        let mut close = false;
        let mut input = egui::RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
        let output = editor_frame(&context, &mut session, &mut close, input);
        assert!(!close);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let preview = loop {
            if let Some(preview) = session.editor.as_mut().unwrap().poll(&context) {
                break preview;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        };
        assert_eq!(preview.content.as_str(), captured);
        assert!(
            session
                .editor
                .as_ref()
                .unwrap()
                .text
                .ends_with("// newer draft")
        );
        assert!(session.dirty());
        assert!(!session.request_close());
        session.close.as_mut().unwrap().discard = true;
        let output = editor_frame(
            &context,
            &mut session,
            &mut close,
            egui::RawInput::default(),
        );
        assert!(close);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Close))
        );
    }

    #[test]
    fn failed_delivery_keeps_an_unchanged_source_open_to_report_the_error() {
        let mut session = clipboard_graph("digraph { a -> b }");
        session
            .editor
            .as_mut()
            .unwrap()
            .copy_source(|_| Err("clipboard unavailable".into()));
        session.close = Some(CloseRequest {
            requested: true,
            window: true,
            discard: false,
        });
        assert!(!session.dirty());
        let context = egui::Context::default();
        let mut close = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let output = editor_frame(
                &context,
                &mut session,
                &mut close,
                egui::RawInput::default(),
            );
            assert!(!close);
            if !session.busy() {
                assert!(
                    output.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .iter()
                        .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            session.editor.as_ref().unwrap().error.as_deref(),
            Some("clipboard unavailable")
        );
        assert!(!session.close.as_ref().unwrap().requested);
        // A fresh explicit close is still allowed after the failure is shown.
        assert!(session.request_close());
    }

    #[test]
    fn copy_failure_keeps_clipboard_drafts_and_copying_files_does_not_mark_them_saved() {
        let mut session = clipboard_graph("digraph { a -> b } // changed");
        let context = egui::Context::default();
        let editor = session.editor.as_mut().unwrap();
        editor.copy_source(|_| Err("clipboard unavailable".into()));
        wait_for_save(editor, &context);
        assert_eq!(editor.error.as_deref(), Some("clipboard unavailable"));
        assert!(session.dirty());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.gv");
        std::fs::write(&path, "digraph { a -> b }").unwrap();
        let mut session = TextSession::load(&path, &Default::default()).unwrap();
        let editor = session.editor.as_mut().unwrap();
        editor.text.push_str(" // changed");
        editor.copy_source(|_| Ok(()));
        wait_for_save(editor, &context);
        assert!(editor.error.is_none());
        assert!(session.dirty());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "digraph { a -> b }"
        );
    }

    #[test]
    fn clipboard_escape_cancels_nested_dialogs_and_protects_unsaved_drafts() {
        let mut session = clipboard_graph("digraph { a -> b } // changed");
        let context = egui::Context::default();
        let mut close = false;
        let frame = |session: &mut TextSession, close: &mut bool| {
            editor_frame(
                &context,
                session,
                close,
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
            )
        };
        let output = frame(&mut session, &mut close);
        assert!(!close);
        assert!(session.dirty());
        assert!(session.close.as_ref().unwrap().requested);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::CancelClose))
        );
        frame(&mut session, &mut close);
        assert!(!session.close.as_ref().unwrap().requested);
        session.editor.as_mut().unwrap().save_as = true;
        frame(&mut session, &mut close);
        assert!(!session.editor.as_ref().unwrap().save_as);
        assert!(!close);
    }

    #[test]
    fn diagram_files_keep_live_drafts_and_existing_guarded_save_rules() {
        let directory = tempfile::tempdir().unwrap();
        for (extension, original, edited) in [
            ("mermaid", "flowchart LR\nA-->B\n", "flowchart LR\nA-->C\n"),
            ("mmd", "flowchart LR\nA-->B\n", "flowchart LR\nA-->C\n"),
            ("lini", "|box| \"Source\"\n", "|box| \"Edited\"\n"),
            ("gv", "digraph { a -> b }", "digraph { a -> c }"),
            ("dot", "digraph { a -> b }", "digraph { a -> c }"),
            ("graphviz", "digraph { a -> b }", "digraph { a -> c }"),
            ("d2", "a -> b: Original", "a -> b: Edited"),
            (
                "json5",
                "{signal:[{name:'x',wave:'01'}]}",
                "{signal:[{name:'x',wave:'10'}]}",
            ),
            (
                "wavejson",
                "{signal:[{name:'x',wave:'01'}]}",
                "{signal:[{name:'x',wave:'10'}]}",
            ),
            (
                "wavedrom",
                "{signal:[{name:'x',wave:'01'}]}",
                "{signal:[{name:'x',wave:'10'}]}",
            ),
            (
                "bpmn",
                "<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\" targetNamespace=\"urn:test\"><process id=\"p\" name=\"Original\"/></definitions>",
                "<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\" targetNamespace=\"urn:test\"><process id=\"p\" name=\"Edited\"/></definitions>",
            ),
            (
                "bpmn2",
                "<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\" targetNamespace=\"urn:test\"><process id=\"p\" name=\"Original\"/></definitions>",
                "<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\" targetNamespace=\"urn:test\"><process id=\"p\" name=\"Edited\"/></definitions>",
            ),
            (
                "xml",
                "<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\" targetNamespace=\"urn:test\"><process id=\"p\" name=\"Original\"/></definitions>",
                "<definitions xmlns=\"http://www.omg.org/spec/BPMN/20100524/MODEL\" targetNamespace=\"urn:test\"><process id=\"p\" name=\"Edited\"/></definitions>",
            ),
            (
                "pikchr",
                "box \"Original\"\n",
                "box \"Edited\"; arrow; circle\n",
            ),
            (
                "svgbob",
                "+----+\n|Old |\n+----+\n",
                "+----+\n|New |\n+----+\n",
            ),
        ] {
            let path = directory.path().join(format!("diagram.{extension}"));
            std::fs::write(&path, original).unwrap();
            let mut session = TextSession::load(&path, &Default::default()).unwrap();
            assert!(session.mode == Mode::Diagram);
            assert_eq!(
                session.editor.as_ref().unwrap().name,
                format!("edited.{extension}")
            );
            assert!(session.diagram.is_some());
            session.editor.as_mut().unwrap().text = edited.into();
            assert!(session.dirty());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            let editor = session.editor.as_mut().unwrap();
            editor.save(path.clone());
            wait_for_save(editor, &egui::Context::default());
            assert!(editor.error.is_none(), "{:?}", editor.error);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
            assert!(!session.dirty());
        }
    }

    #[test]
    fn subtitle_save_rebuilds_the_cue_timeline_and_preserves_the_draft() {
        for (extension, source, replacement) in [
            (
                "srt",
                "1\n00:00:01,000 --> 00:00:02,000\nOriginal\n",
                "1\n00:00:03,000 --> 00:00:05,000\nEdited\n",
            ),
            (
                "vtt",
                "WEBVTT\n\nNOTE preserve me\n\n00:01.000 --> 00:02.000\nOriginal\n",
                "WEBVTT\n\nNOTE preserve me\n\nSTYLE\n::cue { color: red; }\n\nedited\n00:03.000 --> 00:05.000 align:start\nEdited\n",
            ),
            (
                "ass",
                "[Script Info]\nScriptType: v4.00+\n[Events]\nFormat: Start,End,Text\nDialogue: 0:00:01.00,0:00:02.00,Original\n",
                "[Script Info]\nScriptType: v4.00+\n[Events]\nFormat: Start,End,Text\nComment: 0:00:00.00,0:00:30.00,Keep metadata\nDialogue: 0:00:03.00,0:00:05.00,{\\i1}Edited\n",
            ),
            (
                "ssa",
                "[Script Info]\nScriptType: v4.00\n[Events]\nDialogue: Marked=0,0:00:01.00,0:00:02.00,Default,Guide,0,0,0,,Original\n",
                "[Script Info]\nScriptType: v4.00\n[Events]\nComment: Marked=0,0:00:00.00,0:00:30.00,Default,Guide,0,0,0,,Keep metadata\nDialogue: Marked=0,0:00:03.00,0:00:05.00,Default,Guide,0,0,0,,{\\b1}Edited\n",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("captions.{extension}"));
            std::fs::write(&path, source).unwrap();
            let mut session = TextSession::load(&path, &Default::default()).unwrap();
            let editor = session.editor.as_mut().unwrap();
            editor.text = replacement.into();
            editor.save(path.clone());
            let ctx = egui::Context::default();
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let refreshed = loop {
                if let Some(refreshed) = editor.poll(&ctx) {
                    break refreshed;
                }
                assert!(editor.pending.is_some(), "{:?}", editor.error);
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(
                refreshed.view_mode,
                crate::text_preview::TextViewMode::SubtitleTimeline
            );
            assert_eq!(refreshed.content.as_str(), replacement);
            assert_eq!(std::fs::read_to_string(path).unwrap(), replacement);
            assert_eq!(editor.text, editor.saved);
        }
    }

    #[test]
    fn non_utf8_and_large_text_are_inspection_only() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("invalid.txt", [vec![b'A'; 10_000], vec![0xff]].concat()),
            ("large.txt", vec![b'A'; 2 * 1024 * 1024 + 1]),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let session = TextSession::load(&path, &Default::default()).unwrap();
            assert!(session.editor.is_none());
            assert_eq!(session.path(), path);
            assert!(!session.dirty());
        }
    }

    #[test]
    fn save_as_handles_unicode_names_and_preserves_existing_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("café 東京.txt");
        std::fs::write(&path, "original").unwrap();
        let mut session = TextSession::load(&path, &Default::default()).unwrap();
        let editor = session.editor.as_mut().unwrap();
        editor.text = "edited 🖥️\n".into();
        let sibling = dir.path().join("new café.txt");
        let ctx = egui::Context::default();
        for _ in 0..2 {
            editor.save(sibling.clone());
            wait_for_save(editor, &ctx);
            assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "edited 🖥️\n");
        }
        assert_eq!(editor.path, sibling);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
        let existing = dir.path().join("collision.txt");
        std::fs::write(&existing, "preserved").unwrap();
        editor.save(existing.clone());
        wait_for_save(editor, &ctx);
        assert!(editor.error.is_some());
        assert_eq!(std::fs::read_to_string(existing).unwrap(), "preserved");
    }
}
