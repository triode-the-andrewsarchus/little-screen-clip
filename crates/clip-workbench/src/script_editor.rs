//! Small script editor; drafts survive node selection and files are saved explicitly.
use crate::shell::EscapeHatchLauncher;
use crate::text_preview::{build_highlighted_layout_job, find_syntax};
use egui::text::{CCursor, CCursorRange};
use egui::{self, Id};
use little_clip_core::config::AppConfig;
use little_clip_core::palette::AppPalette;
use little_clip_ops::pipeline::recipes::{create_script_file_scaffold_in_dir, get_recipes_dir};
use little_clip_ops::pipeline::types::{NodeInstance, NodeKind};
use std::path::{Path, PathBuf};

pub struct ScriptEditor {
    pub text: String,
    saved: String,
    path: Option<PathBuf>,
    existed: bool,
    error: Option<String>,
    status: Option<String>,
    find: String,
    replace: String,
    search_open: bool,
    focus_find: bool,
    cached: Option<(String, String, egui::text::LayoutJob)>,
}

#[derive(Clone, Copy)]
enum EditorCommand {
    Save,
    ExternalEditor,
    Reload,
    Search,
    Undo,
    Redo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SearchAction {
    None,
    Next,
    Replace,
    ReplaceAll,
}

impl ScriptEditor {
    pub fn load(node: &NodeInstance) -> Self {
        let path = node
            .props
            .get("script_file")
            .map(|name| get_recipes_dir().join(name));
        let mut error = None;
        let mut existed = false;
        let text = if let Some(path) = &path {
            match std::fs::read_to_string(path) {
                Ok(text) => {
                    existed = true;
                    text
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    error = Some(format!("Could not read script: {e}"));
                    String::new()
                }
            }
        } else {
            node.props
                .get(Self::inline_key(node))
                .cloned()
                .unwrap_or_else(|| {
                    if node.kind == NodeKind::PythonScript {
                        "import sys\nsys.stdout.write(sys.stdin.read())\n".to_owned()
                    } else {
                        "cat\n".to_owned()
                    }
                })
        };
        Self {
            saved: text.clone(),
            text,
            path,
            existed,
            error,
            status: None,
            find: String::new(),
            replace: String::new(),
            search_open: false,
            focus_find: false,
            cached: None,
        }
    }

    fn inline_key(node: &NodeInstance) -> &'static str {
        if node.kind == NodeKind::PythonScript {
            "script"
        } else {
            "command"
        }
    }

    pub fn matches_source(&self, node: &NodeInstance) -> bool {
        self.path
            == node
                .props
                .get("script_file")
                .map(|name| get_recipes_dir().join(name))
            && (self.path.is_some()
                || self.dirty()
                || node
                    .props
                    .get(Self::inline_key(node))
                    .is_none_or(|text| text == &self.saved))
    }

    /// Accept header/path updates made by the recipe saver without discarding search state.
    pub fn refresh_after_recipe_save(&mut self, node: &NodeInstance) {
        if self.path.is_some() && !self.dirty() {
            let refreshed = Self::load(node);
            self.text = refreshed.text;
            self.saved = refreshed.saved;
            self.path = refreshed.path;
            self.existed = refreshed.existed;
            self.error = refreshed.error;
            self.cached = None;
        }
    }

    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    pub fn save(&mut self, node: &mut NodeInstance) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if let Some(path) = &self.path {
            let disk = std::fs::read_to_string(path);
            let unchanged = match &disk {
                Ok(text) => self.existed && text == &self.saved,
                Err(e) => !self.existed && e.kind() == std::io::ErrorKind::NotFound,
            };
            if !unchanged {
                return Err("Script changed on disk. Reload before saving.".to_owned());
            }
            std::fs::write(path, &self.text).map_err(|e| format!("Could not save script: {e}"))?;
            self.existed = true;
        } else {
            node.props
                .insert(Self::inline_key(node).to_owned(), self.text.clone());
        }
        self.saved.clone_from(&self.text);
        Ok(())
    }

    fn open_external_with(
        &mut self,
        node: &mut NodeInstance,
        recipes_dir: &Path,
        editor: &str,
        launch: impl FnOnce(&str, &Path) -> Result<String, String>,
    ) -> Result<String, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.path.is_none() {
            let filename = create_script_file_scaffold_in_dir(
                &node.title,
                node.kind == NodeKind::PythonScript,
                recipes_dir,
            )
            .map_err(|e| format!("Could not create script file: {e}"))?;
            let path = recipes_dir.join(&filename);
            std::fs::write(&path, &self.text).map_err(|e| format!("Could not save script: {e}"))?;
            node.props.insert("script_file".to_owned(), filename);
            node.props.remove(Self::inline_key(node));
            self.path = Some(path);
            self.existed = true;
            self.saved.clone_from(&self.text);
        } else if self.dirty() || !self.existed {
            self.save(node)?;
        }
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| "Script has no file path".to_owned())?;
        launch(editor, path)
            .map(|message| format!("{message}. Use Reload to bring external edits back."))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, node: &mut NodeInstance, palette: &AppPalette) {
        let id = Id::new(("script-code", node.id));
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        ui.heading(if node.kind == NodeKind::PythonScript {
            "Python Script"
        } else {
            "Bash Script"
        });
        ui.label(
            self.path
                .as_ref()
                .map_or_else(|| "Inline script".to_owned(), |p| p.display().to_string()),
        );
        let mut commands = self.render_toolbar(ui, &state);
        commands.extend(self.shortcuts(ui, id));
        let mut state_changed = false;
        for command in commands {
            state_changed |= self.apply_command(command, node, &mut state);
        }
        let search = self.render_search(ui, id);
        if search != SearchAction::None {
            self.apply_search(search, &mut state, ui.input(|i| i.time));
            ui.memory_mut(|m| m.request_focus(id));
            state_changed = true;
        }
        if state_changed {
            state.store(ui.ctx(), id);
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::hex_to_c32(&palette.diff_del), error);
        }
        if let Some(status) = &self.status {
            ui.label(status);
        }
        ui.add(
            egui::Label::new(
                "Ctrl+S Save · Ctrl+F Find · Ctrl+Z Undo · Ctrl+Shift+Z / Ctrl+Y Redo",
            )
            .wrap(true),
        );
        self.render_code(
            ui,
            node,
            palette,
            matches!(search, SearchAction::Next | SearchAction::Replace),
        );
    }

    fn shortcuts(&mut self, ui: &mut egui::Ui, id: Id) -> Vec<EditorCommand> {
        let code_focused = ui.memory(|m| m.has_focus(id));
        if !ui.memory(|m| {
            [id, id.with("find"), id.with("replace")]
                .into_iter()
                .any(|field| m.has_focus(field))
        }) {
            return Vec::new();
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            self.search_open = true;
            self.focus_find = true;
        }
        let mut commands = Vec::new();
        // egui 0.27 matches Ctrl+Shift+Z as Ctrl+Z before its redo branch.
        if code_focused
            && ui.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                ) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y)
            })
        {
            commands.push(EditorCommand::Redo);
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
            commands.push(EditorCommand::Save);
        }
        commands
    }

    fn render_toolbar(
        &self,
        ui: &mut egui::Ui,
        state: &egui::text_edit::TextEditState,
    ) -> Vec<EditorCommand> {
        let undoer = state.undoer();
        let before = (
            state.cursor.char_range().unwrap_or_default(),
            self.text.clone(),
        );
        let mut commands = Vec::new();
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(if self.dirty() { "Save *" } else { "Save" })
                .clicked()
            {
                commands.push(EditorCommand::Save);
            }
            if ui
                .button("Open in External Editor")
                .on_hover_text(
                    "Save this script and open it in your preferred editor (for example, VS Code)",
                )
                .clicked()
            {
                commands.push(EditorCommand::ExternalEditor);
            }
            if ui
                .button("Reload")
                .on_hover_text("Discard this draft and reload the script")
                .clicked()
            {
                commands.push(EditorCommand::Reload);
            }
            if ui.button("Find / Replace").clicked() {
                commands.push(EditorCommand::Search);
            }
            if ui
                .add_enabled(undoer.has_undo(&before), egui::Button::new("Undo"))
                .clicked()
            {
                commands.push(EditorCommand::Undo);
            }
            if ui
                .add_enabled(undoer.has_redo(&before), egui::Button::new("Redo"))
                .clicked()
            {
                commands.push(EditorCommand::Redo);
            }
        });
        commands
    }

    fn apply_command(
        &mut self,
        command: EditorCommand,
        node: &mut NodeInstance,
        state: &mut egui::text_edit::TextEditState,
    ) -> bool {
        match command {
            EditorCommand::Save => {
                self.status = Some(self.save(node).err().unwrap_or_else(|| "Saved".to_owned()));
                false
            }
            EditorCommand::ExternalEditor => {
                let editor = AppConfig::load().workbench.resolve_editor();
                self.status = Some(
                    self.open_external_with(node, &get_recipes_dir(), &editor, |editor, path| {
                        EscapeHatchLauncher::spawn_with_fallback(editor, editor, path)
                    })
                    .unwrap_or_else(|error| error),
                );
                false
            }
            EditorCommand::Reload => {
                *self = Self::load(node);
                *state = Default::default();
                true
            }
            EditorCommand::Search => {
                self.search_open = !self.search_open;
                self.focus_find = self.search_open;
                false
            }
            EditorCommand::Undo | EditorCommand::Redo => {
                let before = (
                    state.cursor.char_range().unwrap_or_default(),
                    self.text.clone(),
                );
                let mut undoer = state.undoer();
                let result = match command {
                    EditorCommand::Undo => undoer.undo(&before),
                    _ => undoer.redo(&before),
                }
                .cloned();
                if let Some((cursor, text)) = result {
                    self.text = text;
                    state.cursor.set_char_range(Some(cursor));
                }
                state.set_undoer(undoer);
                true
            }
        }
    }

    fn render_search(&mut self, ui: &mut egui::Ui, id: Id) -> SearchAction {
        if !self.search_open {
            return SearchAction::None;
        }
        let mut action = SearchAction::None;
        ui.horizontal(|ui| {
            ui.label("Find");
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.find)
                    .id(id.with("find"))
                    .desired_width((ui.available_width() - 75.0).max(60.0)),
            );
            if self.focus_find {
                field.request_focus();
                self.focus_find = false;
            }
            if ui
                .add_enabled(!self.find.is_empty(), egui::Button::new("Next"))
                .clicked()
                || (field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                action = SearchAction::Next;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Replace");
            ui.add(
                egui::TextEdit::singleline(&mut self.replace)
                    .id(id.with("replace"))
                    .desired_width(ui.available_width().max(60.0)),
            );
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!self.find.is_empty(), egui::Button::new("Replace"))
                .clicked()
            {
                action = SearchAction::Replace;
            }
            if ui
                .add_enabled(!self.find.is_empty(), egui::Button::new("Replace All"))
                .clicked()
            {
                action = SearchAction::ReplaceAll;
            }
            ui.add(
                egui::Label::new(format!(
                    "{} matches · case sensitive",
                    if self.find.is_empty() {
                        0
                    } else {
                        self.text.matches(&self.find).count()
                    }
                ))
                .wrap(false),
            );
        });
        action
    }

    fn apply_search(
        &mut self,
        action: SearchAction,
        state: &mut egui::text_edit::TextEditState,
        time: f64,
    ) {
        let mut selection = state.cursor.char_range().unwrap_or_default();
        let mut undoer = state.undoer();
        undoer.add_undo(&(selection, self.text.clone()));
        if action == SearchAction::ReplaceAll && !self.find.is_empty() {
            self.text = self.text.replace(&self.find, &self.replace);
        } else if let Some((start, end)) =
            self.next_match(selection, action == SearchAction::Replace)
        {
            let end = if action == SearchAction::Replace {
                self.text.replace_range(start..end, &self.replace);
                start + self.replace.len()
            } else {
                end
            };
            selection = CCursorRange::two(
                CCursor::new(self.text[..start].chars().count()),
                CCursor::new(self.text[..end].chars().count()),
            );
        }
        undoer.feed_state(time, &(selection, self.text.clone()));
        undoer.add_undo(&(selection, self.text.clone()));
        state.cursor.set_char_range(Some(selection));
        state.set_undoer(undoer);
    }

    fn render_code(
        &mut self,
        ui: &mut egui::Ui,
        node: &NodeInstance,
        palette: &AppPalette,
        reveal_selection: bool,
    ) {
        let id = Id::new(("script-code", node.id));
        let lang = if node.kind == NodeKind::PythonScript {
            "py"
        } else {
            "sh"
        };
        let color_key = palette.colors.text_primary.clone();
        if self
            .cached
            .as_ref()
            .is_none_or(|(text, colors, _)| text != &self.text || colors != &color_key)
        {
            self.cached = Some((
                self.text.clone(),
                color_key,
                build_highlighted_layout_job(&self.text, find_syntax(lang), palette, &[]),
            ));
        }
        let cached = &self.cached.as_ref().unwrap().2;
        let mut layouter = |ui: &egui::Ui, text: &str, _: f32| {
            let mut job = if text == cached.text {
                cached.clone()
            } else {
                build_highlighted_layout_job(text, find_syntax(lang), palette, &[])
            };
            job.wrap.max_width = f32::INFINITY;
            ui.fonts(|f| f.layout_job(job))
        };
        egui::ScrollArea::both()
            .id_source(("script-scroll", node.id))
            .show(ui, |ui| {
                let output = egui::TextEdit::multiline(&mut self.text)
                    .id(id)
                    .code_editor()
                    .desired_width(ui.available_width())
                    .desired_rows(20)
                    .layouter(&mut layouter)
                    .show(ui);
                if reveal_selection && let Some(cursor) = output.cursor_range {
                    ui.scroll_to_rect(
                        output
                            .galley
                            .pos_from_cursor(&cursor.primary)
                            .translate(output.galley_pos.to_vec2()),
                        None,
                    );
                }
            });
    }

    fn next_match(
        &self,
        selection: CCursorRange,
        include_selection: bool,
    ) -> Option<(usize, usize)> {
        if self.find.is_empty() {
            return None;
        }
        let chars = selection.sorted();
        let index = if include_selection {
            chars[0].index
        } else {
            chars[1].index
        };
        let offset = self
            .text
            .char_indices()
            .nth(index)
            .map_or(self.text.len(), |(byte, _)| byte);
        self.text[offset..]
            .find(&self.find)
            .map(|i| offset + i)
            .or_else(|| self.text[..offset].find(&self.find))
            .map(|start| (start, start + self.find.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_ops::pipeline::catalog::{instantiate_template, search_templates};

    fn script_node(kind: NodeKind) -> NodeInstance {
        instantiate_template(
            search_templates("", None)
                .into_iter()
                .find(|t| t.kind == kind)
                .unwrap(),
            [0.0, 0.0],
        )
    }

    #[test]
    fn external_editor_receives_saved_python_and_bash_files() {
        let dir = tempfile::tempdir().unwrap();
        for kind in [NodeKind::PythonScript, NodeKind::BashCommand] {
            let mut node = script_node(kind);
            let mut editor = ScriptEditor::load(&node);
            editor.text = "# current draft\n".to_owned();
            let result = editor
                .open_external_with(
                    &mut node,
                    dir.path(),
                    "preferred-editor",
                    |command, path| {
                        assert_eq!(command, "preferred-editor");
                        assert_eq!(std::fs::read_to_string(path).unwrap(), "# current draft\n");
                        Ok("Opened".to_owned())
                    },
                )
                .unwrap();
            assert!(result.contains("Reload"));
            assert_eq!(
                editor.path.as_ref().unwrap().extension().unwrap(),
                if node.kind == NodeKind::PythonScript {
                    "py"
                } else {
                    "sh"
                }
            );
            assert!(node.props.contains_key("script_file"));
            assert!(!node.props.contains_key(ScriptEditor::inline_key(&node)));
            assert!(!editor.dirty());
            editor.text.push_str("# more edits\n");
            editor
                .open_external_with(&mut node, dir.path(), "preferred-editor", |_, path| {
                    assert!(
                        std::fs::read_to_string(path)
                            .unwrap()
                            .ends_with("# more edits\n")
                    );
                    Ok("Opened".to_owned())
                })
                .unwrap();
        }
    }

    #[test]
    fn external_editor_does_not_launch_when_saving_conflicts_with_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.py");
        std::fs::write(&path, "original").unwrap();
        let mut node = script_node(NodeKind::PythonScript);
        node.props
            .insert("script_file".to_owned(), path.display().to_string());
        let mut editor = ScriptEditor::load(&node);
        editor.text = "draft".to_owned();
        std::fs::write(&path, "external edit").unwrap();
        assert!(
            editor
                .open_external_with(&mut node, dir.path(), "preferred-editor", |_, _| panic!(
                    "must not launch after a failed save"
                ))
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "external edit");
    }

    #[test]
    fn search_uses_utf8_offsets_and_wraps_from_end() {
        let mut editor = ScriptEditor::load(&script_node(NodeKind::PythonScript));
        editor.text = "é猫 hit\n猫 hit".to_owned();
        editor.find = "猫".to_owned();
        assert_eq!(
            editor.next_match(CCursorRange::default(), false),
            Some((2, 5))
        );
        assert_eq!(
            editor.next_match(CCursorRange::one(CCursor::new(2)), false),
            Some((10, 13))
        );
        assert_eq!(
            editor.next_match(CCursorRange::one(CCursor::new(100)), false),
            Some((2, 5))
        );
        editor.find.clear();
        assert_eq!(editor.next_match(CCursorRange::default(), true), None);
    }

    #[test]
    fn saves_inline_python_and_bash_using_executor_properties() {
        for (kind, key) in [
            (NodeKind::PythonScript, "script"),
            (NodeKind::BashCommand, "command"),
        ] {
            let mut node = script_node(kind);
            let mut editor = ScriptEditor::load(&node);
            editor.text = "changed".to_owned();
            editor.save(&mut node).unwrap();
            assert_eq!(node.props.get(key).unwrap(), "changed");
            assert!(!editor.dirty());
        }
    }

    #[test]
    fn file_save_preserves_draft_when_disk_changes_and_reload_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.py");
        std::fs::write(&path, "original").unwrap();
        let mut node = script_node(NodeKind::PythonScript);
        node.props
            .insert("script_file".to_owned(), path.display().to_string());
        let mut editor = ScriptEditor::load(&node);
        editor.text = "draft".to_owned();
        std::fs::write(&path, "external").unwrap();
        assert!(editor.save(&mut node).is_err());
        assert_eq!(editor.text, "draft");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        editor = ScriptEditor::load(&node);
        editor.text = "saved".to_owned();
        editor.save(&mut node).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved");
        std::fs::write(&path, "# Recipe: Changed\nsaved").unwrap();
        editor.refresh_after_recipe_save(&node);
        assert_eq!(editor.text, "# Recipe: Changed\nsaved");
        editor.text.push_str("\n# next edit");
        editor.save(&mut node).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), editor.text);
    }

    #[test]
    fn code_editor_typing_undo_redo_and_search_replace_share_history() {
        let mut node = script_node(NodeKind::PythonScript);
        let mut editor = ScriptEditor::load(&node);
        editor.text = "value = 1\n".to_owned();
        editor.saved = editor.text.clone();
        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let id = Id::new(("script-code", node.id));
        let mut time = 0.0;
        let mut frame = |editor: &mut ScriptEditor, events| {
            time += 0.1;
            ctx.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| editor.ui(ui, &mut node, &palette));
                },
            )
        };
        frame(&mut editor, vec![]);
        ctx.memory_mut(|m| m.request_focus(id));
        frame(&mut editor, vec![egui::Event::Text("#".to_owned())]);
        let typed = editor.text.clone();
        assert_ne!(typed, editor.saved);
        for _ in 0..15 {
            frame(&mut editor, vec![]);
        }
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        frame(
            &mut editor,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(editor.text, editor.saved);
        frame(
            &mut editor,
            vec![key(
                egui::Key::Z,
                egui::Modifiers {
                    shift: true,
                    ..egui::Modifiers::COMMAND
                },
            )],
        );
        assert_eq!(editor.text, typed);
        frame(
            &mut editor,
            vec![key(egui::Key::F, egui::Modifiers::COMMAND)],
        );
        assert!(editor.search_open);
        editor.find = "value".to_owned();
        editor.replace = "résultat".to_owned();
        let output = frame(&mut editor, vec![]);
        let button_pos = output
            .shapes
            .iter()
            .find_map(|clipped| {
                if let egui::epaint::Shape::Text(text) = &clipped.shape
                    && text.galley.job.text == "Replace All"
                {
                    return Some(text.galley.rect.translate(text.pos.to_vec2()).center());
                }
                None
            })
            .expect("replace-all control");
        let click = |pressed| egui::Event::PointerButton {
            pos: button_pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut editor, vec![egui::Event::PointerMoved(button_pos)]);
        frame(&mut editor, vec![click(true)]);
        frame(&mut editor, vec![click(false)]);
        assert_eq!(editor.text, typed.replace("value", "résultat"));
        frame(
            &mut editor,
            vec![key(egui::Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(editor.text, typed);
        frame(
            &mut editor,
            vec![key(egui::Key::Y, egui::Modifiers::COMMAND)],
        );
        assert_eq!(editor.text, typed.replace("value", "résultat"));
    }
}
