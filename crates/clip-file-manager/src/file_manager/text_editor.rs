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
pub(super) struct TextSession {
    pub preview: TextPreviewState,
    mode: Mode,
    editor: Option<Editor>,
    diagram: Option<(
        little_clip_core::diagram::Format,
        clip_studios::diagram_preview::Preview,
    )>,
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
                name: little_clip_core::diagram::Format::from_path(path)
                    .and_then(|_| path.extension().and_then(|extension| extension.to_str()))
                    .map_or_else(
                        || "edited.txt".into(),
                        |extension| format!("edited.{extension}"),
                    ),
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
        let source = editor
            .as_ref()
            .map_or(preview.content.as_str(), |editor| editor.text.as_str());
        let format = little_clip_core::diagram::Format::from_source_path(path, source);
        Ok(Self {
            preview,
            mode: if format.is_some() {
                Mode::Diagram
            } else {
                Mode::Preview
            },
            editor,
            diagram: format.map(|format| (format, Default::default())),
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
        self.editor.as_ref().map(|editor| editor.source.clone())
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
            editor.source = source;
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
            clip_studios::diagram_preview::Preview,
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
                self.save(self.path.clone());
            }
            if ui
                .add(ButtonRole::Secondary.button("Save As…", palette))
                .clicked()
            {
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
                        if ui
                            .add(ButtonRole::Primary.button("Create file", palette))
                            .clicked()
                        {
                            if super::edits::parse_name(&self.name).is_ok() {
                                let path = self.path.with_file_name(&self.name);
                                self.save(path);
                                self.save_as = false;
                            } else {
                                self.error = Some("Enter a single filename.".into());
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
    fn diagram_files_keep_live_drafts_and_existing_guarded_save_rules() {
        let directory = tempfile::tempdir().unwrap();
        for (extension, original, edited) in [
            ("mermaid", "flowchart LR\nA-->B\n", "flowchart LR\nA-->C\n"),
            ("mmd", "flowchart LR\nA-->B\n", "flowchart LR\nA-->C\n"),
            ("lini", "|box| \"Source\"\n", "|box| \"Edited\"\n"),
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
