//! Small UTF-8 file editor with explicit, guarded source writes.
use crate::{
    file_source::FileSource,
    text_preview::TextPreviewState,
    ui_tabs::{Tab, TabBar},
};
use clip_ui::buttons::ButtonRole;
use little_clip_core::{BackgroundJobs, JobError, diagram::Format, palette::AppPalette};
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
use crate::diagram_preview::GraphicalUpdate;

/// Owns a guarded UTF-8 source editor and its live previews.
pub struct TextSession {
    pub preview: TextPreviewState,
    close: Option<CloseRequest>,
    mode: Mode,
    editor: Option<Editor>,
    diagram: Option<(
        little_clip_core::diagram::Format,
        Box<crate::diagram_preview::Preview>,
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

enum AfterGesture {
    Save,
    SaveTo(PathBuf),
    Undo,
    Redo,
}

struct Editor {
    id: egui::Id,
    text: String,
    timing: Option<Box<crate::text_editor_wave::WaveEditor>>,
    drawing: Option<Box<crate::text_editor_drawing::DrawingEditor>>,
    harness: Option<Box<crate::text_editor_wireviz::WireEditor>>,
    after_gesture: Vec<AfterGesture>,
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
                    .and_then(|format| {
                        if format == Format::Excalidraw {
                            Some("excalidraw")
                        } else {
                            path.extension().and_then(|extension| extension.to_str())
                        }
                    })
                    .map_or_else(
                        || "edited.txt".into(),
                        |extension| format!("edited.{extension}"),
                    );
                Editor::new(text, Some(source), path.to_path_buf(), name, config)
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
            editor: Some(Editor::new(
                content.as_str().to_owned(),
                None,
                PathBuf::from(format!("clipboard.{extension}")),
                format!("clipboard.{extension}"),
                config,
            )),
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
            .is_some_and(|editor| editor.pending.is_some() || editor.graphic_busy())
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
                && self.editor.as_ref().is_some_and(|editor| {
                    editor.error.is_some()
                        || editor
                            .drawing
                            .as_ref()
                            .is_some_and(|drawing| drawing.failed())
                        || editor
                            .harness
                            .as_ref()
                            .is_some_and(|harness| harness.failed())
                })
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
            if self.mode != Mode::Diagram {
                editor.suspend_graphics();
                if let Some((_, preview)) = &mut self.diagram {
                    preview.suspend();
                }
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
    fn new(
        text: String,
        source: Option<FileSource>,
        path: PathBuf,
        name: String,
        config: &little_clip_core::AppConfig,
    ) -> Self {
        let directory = if source.is_some() {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        Self {
            id: egui::Id::new(uuid::Uuid::new_v4()),
            timing: None,
            drawing: None,
            harness: None,
            after_gesture: Vec::new(),
            saved: text.clone(),
            text,
            source,
            path,
            name,
            save_as: false,
            directory: little_clip_core::path_address::display_path(&directory),
            error: None,
            config: config.clone(),
            pending: None,
        }
    }

    fn apply_graphical_edit(&mut self, ctx: &egui::Context, id: egui::Id, text: String) {
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let cursor = state.cursor.char_range().unwrap_or_default();
        let mut history = state.undoer();
        history.add_undo(&(cursor, self.text.clone()));
        self.text = text;
        let cursor = egui::text::CCursorRange::one(egui::text::CCursor::new(
            cursor.primary.index.min(self.text.chars().count()),
        ));
        history.add_undo(&(cursor, self.text.clone()));
        state.cursor.set_char_range(Some(cursor));
        state.set_undoer(history);
        state.store(ctx, id);
        ctx.request_repaint();
    }
    fn history_step(&mut self, ctx: &egui::Context, id: egui::Id, redo: bool) {
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        let current = (
            state.cursor.char_range().unwrap_or_default(),
            self.text.clone(),
        );
        let mut history = state.undoer();
        let next = if redo {
            history.redo(&current)
        } else {
            history.undo(&current)
        }
        .cloned();
        if let Some((cursor, text)) = next {
            self.text = text;
            state.cursor.set_char_range(Some(cursor));
            ctx.request_repaint();
        }
        state.set_undoer(history);
        state.store(ctx, id);
    }
    fn history_ui(&mut self, ui: &mut egui::Ui, id: egui::Id, palette: &AppPalette) {
        let state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let current = (
            state.cursor.char_range().unwrap_or_default(),
            self.text.clone(),
        );
        let history = state.undoer();
        let canvas_focused = self
            .drawing
            .as_ref()
            .is_some_and(|drawing| drawing.has_focus(ui.ctx()));
        // egui's wants_keyboard_input includes buttons and canvas widgets.
        // Only an actual text editor (or the naming dialog) owns text undo.
        let text_focused = ui
            .ctx()
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
        let (mut undo, mut redo) = if self.save_as || text_focused && !canvas_focused {
            (false, false)
        } else {
            ui.input_mut(|input| {
                // Match the more specific redo chord first; logical modifier
                // matching permits extra Shift on an ordinary undo shortcut.
                let redo = input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                )) || input.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::Y,
                ));
                let undo = !redo
                    && input.consume_shortcut(&egui::KeyboardShortcut::new(
                        egui::Modifiers::COMMAND,
                        egui::Key::Z,
                    ));
                (undo, redo)
            })
        };
        let busy = self.graphic_busy();
        ui.horizontal(|ui| {
            undo |= ui
                .add_enabled(
                    !busy && history.has_undo(&current),
                    ButtonRole::Secondary.button("Undo", palette),
                )
                .clicked();
            redo |= ui
                .add_enabled(
                    !busy && history.has_redo(&current),
                    ButtonRole::Secondary.button("Redo", palette),
                )
                .clicked();
        });
        if busy {
            if undo {
                self.after_gesture.push(AfterGesture::Undo);
            }
            if redo {
                self.after_gesture.push(AfterGesture::Redo);
            }
        } else {
            if undo {
                self.history_step(ui.ctx(), id, false);
            }
            if redo {
                self.history_step(ui.ctx(), id, true);
            }
        }
    }
    fn poll(&mut self, ctx: &egui::Context) -> Option<TextPreviewState> {
        let updates = [
            self.drawing
                .as_mut()
                .and_then(|drawing| drawing.poll(ctx, &self.text)),
            self.harness
                .as_mut()
                .and_then(|harness| harness.poll(ctx, &self.text)),
        ];
        for update in updates.into_iter().flatten() {
            match update {
                GraphicalUpdate::Changed(text) => self.apply_graphical_edit(ctx, self.id, text),
                GraphicalUpdate::Replaced | GraphicalUpdate::Failed => self.after_gesture.clear(),
            }
        }
        if !self.graphic_busy() {
            for action in std::mem::take(&mut self.after_gesture) {
                match action {
                    AfterGesture::Save => self.request_save(),
                    AfterGesture::SaveTo(path) => self.save(path),
                    AfterGesture::Undo => self.history_step(ctx, self.id, false),
                    AfterGesture::Redo => self.history_step(ctx, self.id, true),
                }
            }
        }
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

    fn suspend_graphics(&mut self) {
        if let Some(timing) = &mut self.timing {
            timing.suspend();
        }
        if let Some(drawing) = &mut self.drawing {
            drawing.suspend();
        }
        if let Some(harness) = &mut self.harness {
            harness.suspend();
        }
    }

    fn graphic_busy(&self) -> bool {
        self.drawing.as_ref().is_some_and(|drawing| drawing.busy())
            || self.harness.as_ref().is_some_and(|harness| harness.busy())
    }

    fn request_save(&mut self) {
        if self.graphic_busy() {
            self.after_gesture.push(AfterGesture::Save);
            return;
        }
        if self.source.is_some() {
            self.save(self.path.clone());
        } else {
            self.save_as = true;
        }
    }

    fn save(&mut self, path: PathBuf) {
        if self.graphic_busy() {
            self.after_gesture.push(AfterGesture::SaveTo(path));
            return;
        }
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
        id: egui::Id,
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
                    .id(id)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(24);
                if highlight {
                    edit = edit.layouter(&mut layouter);
                }
                ui.add(edit);
            });
    }

    fn diagram_ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        format: little_clip_core::diagram::Format,
        preview: &mut crate::diagram_preview::Preview,
        source_id: egui::Id,
    ) {
        if format == Format::Excalidraw {
            self.drawing
                .get_or_insert_default()
                .show(ui, &self.text, preview, palette);
            return;
        }
        if format == Format::Wireviz {
            let show_source = self.harness.get_or_insert_default().toolbar(ui, palette);
            if show_source {
                ui.columns(2, |columns| {
                    self.source_ui(&mut columns[0], palette, Some(format), source_id);
                    self.harness.as_mut().unwrap().show(
                        &mut columns[1],
                        &self.text,
                        preview,
                        palette,
                    );
                });
            } else {
                self.harness
                    .as_mut()
                    .unwrap()
                    .show(ui, &self.text, preview, palette);
            }
            return;
        }
        ui.columns(2, |columns| {
            self.source_ui(&mut columns[0], palette, Some(format), source_id);
            egui::ScrollArea::vertical()
                .id_source("diagram_live_preview")
                .show(&mut columns[1], |ui| {
                    if format == little_clip_core::diagram::Format::Wavedrom {
                        if let Some(text) = self
                            .timing
                            .get_or_insert_default()
                            .show(ui, &self.text, palette)
                        {
                            self.apply_graphical_edit(ui.ctx(), source_id, text);
                        }
                        ui.separator();
                    }
                    preview.show(ui, format, &self.text);
                });
        });
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        syntax_format: Option<little_clip_core::diagram::Format>,
        diagram: Option<&mut (
            little_clip_core::diagram::Format,
            Box<crate::diagram_preview::Preview>,
        )>,
    ) {
        let source_id = self.id;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.pending.is_none() && !self.graphic_busy(),
                    ButtonRole::Primary.button("Save", palette),
                )
                .clicked()
            {
                self.request_save();
            }
            if ui
                .add_enabled(
                    !self.graphic_busy(),
                    ButtonRole::Secondary.button("Save As…", palette),
                )
                .clicked()
            {
                self.save_as = true;
            }
            if ui
                .add_enabled(
                    self.pending.is_none() && !self.graphic_busy(),
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
            if syntax_format == Some(Format::Excalidraw) {
                ui.separator();
                self.history_ui(ui, source_id, palette);
            }
        });
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.request_save();
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if let Some(error) = self.drawing.as_ref().and_then(|drawing| drawing.error()) {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Some((format, preview)) = diagram {
            self.diagram_ui(ui, palette, *format, preview, source_id);
        } else {
            self.source_ui(ui, palette, syntax_format, source_id);
        }
        if matches!(syntax_format, Some(Format::Wavedrom | Format::Wireviz)) {
            self.history_ui(ui, source_id, palette);
        }
        self.save_as_ui(ui, palette);
    }

    fn save_as_ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
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

    fn drawing_gesture() -> little_clip_core::diagram::drawing::Change {
        use little_clip_core::diagram::drawing::{Bounds, Change, Kind, Style};
        Change::Create {
            id: "new-box".into(),
            kind: Kind::Rectangle,
            bounds: Bounds {
                x: 20.0,
                y: 30.0,
                width: 80.0,
                height: 50.0,
                angle: 0.0,
            },
            points: Vec::new(),
            style: Style::default(),
        }
    }

    #[test]
    fn graphical_edits_preserve_the_order_of_quick_save_and_undo_commands() {
        let directory = tempfile::tempdir().unwrap();
        for (original, extension, harness) in [
            (
                "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[]}\n",
                "excalidraw",
                false,
            ),
            (
                include_str!("../../../docs/examples/diagrams/harness.wireviz"),
                "wireviz",
                true,
            ),
        ] {
            for save_first in [true, false] {
                let path = directory
                    .path()
                    .join(format!("canvas-{harness}-{save_first}.{extension}"));
                std::fs::write(&path, original).unwrap();
                let mut session = TextSession::load(&path, &Default::default()).unwrap();
                let release = if harness {
                    let (harness, release) = crate::text_editor_wireviz::delayed_edit(
                        original,
                        little_clip_core::diagram::wireviz::edits::Edit::PinLabel {
                            connector: 0,
                            pin: 0,
                            label: "GROUND".into(),
                        },
                    );
                    session.editor.as_mut().unwrap().harness = Some(harness);
                    release
                } else {
                    let (drawing, release) =
                        crate::text_editor_drawing::delayed_edit(original, drawing_gesture());
                    session.editor.as_mut().unwrap().drawing = Some(drawing);
                    release
                };
                assert!(session.busy());
                assert!(!session.request_close());
                let editor = session.editor.as_mut().unwrap();
                if save_first {
                    editor.request_save();
                }
                editor.after_gesture.push(AfterGesture::Undo);
                if !save_first {
                    editor.request_save();
                }
                assert!(
                    editor.pending.is_none(),
                    "save must wait for the accepted gesture"
                );
                release.send(()).unwrap();
                let ctx = egui::Context::default();
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while editor.graphic_busy() || editor.pending.is_some() {
                    editor.poll(&ctx);
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(editor.error.is_none(), "{:?}", editor.error);
                assert!(!editor.drawing.as_ref().is_some_and(|e| e.failed()));
                assert!(!editor.harness.as_ref().is_some_and(|e| e.failed()));
                assert_eq!(
                    editor.text, original,
                    "undo restores the exact original bytes"
                );
                let saved = std::fs::read_to_string(&path).unwrap();
                if harness {
                    let saved = little_clip_core::diagram::wireviz::parse(&saved).unwrap();
                    assert_eq!(
                        saved.connectors[0].pins[0].label.as_deref(),
                        Some(if save_first { "GROUND" } else { "RETURN" })
                    );
                } else {
                    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
                    assert_eq!(
                        saved["elements"].as_array().unwrap().len(),
                        usize::from(save_first)
                    );
                }
                assert_eq!(editor.text != editor.saved, save_first);
            }
        }
    }

    #[test]
    fn failed_or_superseded_graphical_edits_never_save_an_older_source() {
        let directory = tempfile::tempdir().unwrap();
        for (original, extension, is_harness) in [
            (
                "{\"type\":\"excalidraw\",\"version\":2,\"elements\":[]}",
                "excalidraw",
                false,
            ),
            (
                include_str!("../../../docs/examples/diagrams/harness.wireviz"),
                "wireviz",
                true,
            ),
        ] {
            for superseded in [false, true] {
                let path = directory
                    .path()
                    .join(format!("canvas-{is_harness}-{superseded}.{extension}"));
                std::fs::write(&path, original).unwrap();
                let mut session = TextSession::load(&path, &Default::default()).unwrap();
                let editor = session.editor.as_mut().unwrap();
                let release = if is_harness {
                    let (harness, release) = crate::text_editor_wireviz::delayed_edit(
                        original,
                        little_clip_core::diagram::wireviz::edits::Edit::PinLabel {
                            connector: if superseded { 0 } else { 999 },
                            pin: 0,
                            label: "GROUND".into(),
                        },
                    );
                    editor.harness = Some(harness);
                    release
                } else {
                    let change = if superseded {
                        drawing_gesture()
                    } else {
                        little_clip_core::diagram::drawing::Change::Move {
                            ids: vec!["missing".into()],
                            by: [10.0, 0.0],
                        }
                    };
                    let (drawing, release) =
                        crate::text_editor_drawing::delayed_edit(original, change);
                    editor.drawing = Some(drawing);
                    release
                };
                editor.request_save();
                if superseded {
                    editor.text = "manual source changes".into();
                }
                release.send(()).unwrap();
                let ctx = egui::Context::default();
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while editor.graphic_busy() {
                    editor.poll(&ctx);
                    assert!(std::time::Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
                assert!(editor.pending.is_none());
                assert!(editor.after_gesture.is_empty());
                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
                if superseded {
                    assert_eq!(editor.text, "manual source changes");
                } else {
                    assert!(
                        editor.drawing.as_ref().is_some_and(|e| e.failed())
                            || editor.harness.as_ref().is_some_and(|e| e.failed())
                    );
                }
            }
        }
    }

    fn wait_for_save(editor: &mut Editor, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while editor.pending.is_some() {
            editor.poll(ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn graphical_gestures_and_intervening_typing_share_source_undo_history() {
        let ctx = egui::Context::default();
        let mut session = clipboard_graph("original");
        let editor = session.editor.as_mut().unwrap();
        let id = editor.id;
        let other = clipboard_graph("unrelated file");
        assert_ne!(id, other.editor.as_ref().unwrap().id);
        editor.apply_graphical_edit(&ctx, id, "painted".into());
        editor.text = "painted and typed".into();
        editor.apply_graphical_edit(&ctx, id, "second painting".into());
        let state = egui::TextEdit::load_state(&ctx, id).unwrap();
        let cursor = state.cursor.char_range().unwrap();
        let mut history = state.undoer();
        let current = (cursor, editor.text.clone());
        let typed = history.undo(&current).unwrap().clone();
        assert_eq!(typed.1, "painted and typed");
        let painted = history.undo(&typed).unwrap().clone();
        assert_eq!(painted.1, "painted");
        let original = history.undo(&painted).unwrap().clone();
        assert_eq!(original.1, "original");
        assert_eq!(history.redo(&original).unwrap().1, "painted");
    }

    #[test]
    fn drawing_canvas_focus_routes_undo_to_shared_history_and_text_fields_keep_their_own_keys() {
        let ctx = egui::Context::default();
        let mut session = clipboard_graph("original");
        let editor = session.editor.as_mut().unwrap();
        editor.apply_graphical_edit(&ctx, editor.id, "painted".into());
        let canvas = egui::Id::new("focused drawing fixture");
        editor.drawing = Some(crate::text_editor_drawing::focused_fixture(canvas));
        egui::text_edit::TextEditState::default().store(&ctx, egui::Id::new("a text field"));
        for (focus, expected) in [
            (egui::Id::new("a text field"), "painted"),
            (canvas, "original"),
        ] {
            let modifiers = egui::Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            };
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.interact(
                            ui.available_rect_before_wrap(),
                            focus,
                            egui::Sense::click_and_drag(),
                        );
                        ctx.memory_mut(|memory| memory.request_focus(focus));
                        assert!(ctx.wants_keyboard_input());
                        editor.history_ui(ui, editor.id, &Default::default());
                    });
                },
            );
            assert_eq!(editor.text, expected);
        }
    }

    #[test]
    fn canvas_shortcuts_undo_and_redo_without_a_focused_text_field() {
        let ctx = egui::Context::default();
        let mut session = clipboard_graph("original");
        let editor = session.editor.as_mut().unwrap();
        editor.apply_graphical_edit(&ctx, editor.id, "painted".into());
        for (redo, expected) in [(false, "original"), (true, "painted")] {
            let modifiers = egui::Modifiers {
                ctrl: true,
                command: true,
                shift: redo,
                ..Default::default()
            };
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key: egui::Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        editor.history_ui(ui, editor.id, &Default::default())
                    });
                },
            );
            assert_eq!(editor.text, expected);
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
    fn harness_clipboard_editor_keeps_source_and_uses_yaml_highlighting() {
        let source = std::sync::Arc::new(
            include_str!("../../../docs/examples/diagrams/harness.wireviz").to_owned(),
        );
        let session =
            TextSession::from_clipboard(source.clone(), None, &Default::default()).unwrap();
        assert!(session.mode == Mode::Diagram);
        let editor = session.editor.as_ref().unwrap();
        assert_eq!(editor.name, "clipboard.wireviz");
        assert_eq!(&editor.text, source.as_ref());
        let syntax = crate::text_preview::SYNTAX_SET
            .find_syntax_by_name("WireViz")
            .unwrap();
        let job = crate::text_preview::build_highlighted_layout_job(
            "shield: true\nnotes: 'example'\n# comment\n",
            syntax,
            &Default::default(),
            &[],
        );
        let colors: std::collections::HashSet<_> = job
            .sections
            .iter()
            .map(|section| section.format.color)
            .collect();
        assert!(
            colors.len() > 1,
            "harness source must use YAML highlighting"
        );
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
                "excalidraw",
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                r#"{"type":"excalidraw","version":2,"elements":[],"appState":{}}"#,
            ),
            (
                "excalidraw.json",
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                r#"{"type":"excalidraw","version":2,"elements":[],"appState":{}}"#,
            ),
            (
                "json",
                r#"{"type":"excalidraw","version":2,"elements":[]}"#,
                r#"{"type":"excalidraw","version":2,"elements":[],"appState":{}}"#,
            ),
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
                if Format::from_source_path(&path, original) == Some(Format::Excalidraw) {
                    "edited.excalidraw".into()
                } else {
                    format!("edited.{extension}")
                }
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
