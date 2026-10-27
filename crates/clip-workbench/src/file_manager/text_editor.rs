//! Small UTF-8 file editor with explicit, guarded source writes.
use crate::{
    file_source::FileSource,
    text_preview::TextPreviewState,
    ui_tabs::{Tab, TabBar},
};
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
}
pub(super) struct TextSession {
    pub preview: TextPreviewState,
    mode: Mode,
    editor: Option<Editor>,
}
struct SavedText {
    source: FileSource,
    path: PathBuf,
    text: String,
    preview: Option<TextPreviewState>,
}
struct Editor {
    text: String,
    saved: String,
    source: FileSource,
    path: PathBuf,
    name: String,
    save_as: bool,
    error: Option<String>,
    config: little_clip_core::AppConfig,
    pending: Option<mpsc::Receiver<Result<Result<SavedText, String>, JobError>>>,
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
            String::from_utf8(bytes).ok().map(|text| Editor {
                saved: text.clone(),
                text,
                source,
                path: path.to_path_buf(),
                name: "edited.txt".into(),
                save_as: false,
                error: None,
                config: config.clone(),
                pending: None,
            })
        } else {
            None
        };
        if let Some(editor) = &editor
            && FileSource::capture(path)? != editor.source
        {
            return Err("Text changed while opening; open it again.".into());
        }
        Ok(Self {
            preview,
            mode: Mode::Preview,
            editor,
        })
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
            mode: Mode::Preview,
            editor: None,
        }
    }
    pub fn file_source(&self) -> Option<&FileSource> {
        self.editor.as_ref().map(|editor| &editor.source)
    }
    pub fn path(&self) -> &Path {
        self.editor.as_ref().map_or_else(
            || self.preview.path.as_deref().unwrap_or(Path::new(".")),
            |editor| editor.path.as_path(),
        )
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
        let ctx = ui.ctx().clone();
        if let Some(editor) = &mut self.editor {
            if let Some(preview) = editor.poll(&ctx) {
                self.preview = preview;
            }
            let tabs = [
                Tab::new(Mode::Preview, "Preview"),
                Tab::new(Mode::Edit, "Edit text"),
            ];
            if let Some(mode) = TabBar::new("file_text_modes", self.mode, &tabs, palette).show(ui) {
                self.mode = mode;
            }
            if self.mode == Mode::Edit {
                editor.ui(ui);
                return;
            }
        }
        if self.dirty() {
            ui.label("Preview shows the last saved version. Edit text shows your draft.");
        }
        self.preview.render_ui(ui, &ctx, palette, close);
    }
}
impl Editor {
    fn poll(&mut self, ctx: &egui::Context) -> Option<TextPreviewState> {
        let receiver = self.pending.as_ref()?;
        let Some(result) = super::poll_job(receiver) else {
            ctx.request_repaint_after(Duration::from_millis(40));
            return None;
        };
        self.pending = None;
        match result {
            Ok(saved) => {
                self.source = saved.source;
                self.path = saved.path;
                self.saved = saved.text;
                self.error = None;
                saved.preview
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
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
            let source = source.write(&path, text.as_bytes())?;
            let preview = TextPreviewState::from_file(&path, &config);
            Ok(SavedText {
                source,
                path,
                text,
                preview,
            })
        });
        self.pending = Some(receiver);
    }
    fn ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.pending.is_none(), egui::Button::new("Save"))
                .clicked()
            {
                self.save(self.path.clone());
            }
            if ui.button("Save As…").clicked() {
                self.save_as = true;
            }
            if self.text != self.saved {
                ui.label("Unsaved changes");
            }
            if self.pending.is_some() {
                ui.spinner();
            }
        });
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::S)) {
            self.save(self.path.clone());
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut self.text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(24),
            );
        });
        if self.save_as {
            egui::Window::new("Save text beside source")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(super::display_path(
                        self.path.parent().unwrap_or(Path::new(".")),
                    ));
                    ui.label("Choose a new, unused filename.");
                    ui.text_edit_singleline(&mut self.name);
                    ui.horizontal(|ui| {
                        if ui.button("Create file").clicked() {
                            if super::edits::parse_name(&self.name).is_ok() {
                                let path = self.path.with_file_name(&self.name);
                                self.save(path);
                                self.save_as = false;
                            } else {
                                self.error = Some("Enter a single filename.".into());
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            self.save_as = false;
                        }
                    });
                });
        }
    }
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
