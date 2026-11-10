use super::*;
use crate::ui_ext::StatusMessage;

impl TextPreviewState {
    /// Triggers opening the text in the configured external editor (e.g. VS Code).
    pub fn trigger_escape_hatch(&mut self) {
        self.trigger_escape_hatch_with(|editor, command, target| {
            EscapeHatchLauncher::spawn_with_fallback(editor, command, target)
        });
    }

    pub(super) fn trigger_escape_hatch_with(
        &mut self,
        launch: impl FnOnce(&str, &str, &Path) -> Result<String, String>,
    ) {
        let editor = &self.editor_cmd;

        let target_path = if let Some(ref p) = self.path {
            p.clone()
        } else {
            // Write temporary file with appropriate extension
            let ext = match self.language {
                "rs" => "rs",
                "py" => "py",
                "js" => "js",
                "ts" => "ts",
                "c" => "c",
                "cpp" => "cpp",
                "html" => "html",
                "json" => "json",
                "yaml" => "yaml",
                "toml" => "toml",
                "sh" => "sh",
                "md" => "md",
                _ => "txt",
            };

            let temp_name = format!(
                "little_clip_preview_{}.{}",
                uuid::Uuid::new_v4().simple(),
                ext
            );
            let temp_path = std::env::temp_dir().join(temp_name);
            if let Err(e) = std::fs::write(&temp_path, self.content.as_bytes()) {
                let now = crate::ui_ext::feedback_time();
                self.status_toast.set(
                    StatusMessage::error(format!("Failed to write temporary file: {}", e)),
                    now,
                );
                return;
            }
            temp_path
        };

        let now = crate::ui_ext::feedback_time();

        match launch(editor, editor, &target_path) {
            Ok(_) => self.status_toast.clear(),
            Err(message) => self.status_toast.set(StatusMessage::error(message), now),
        }
    }

    /// Copies the full text content to the system clipboard.
    pub fn copy_all_to_clipboard(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.content.as_str().to_owned());
        if let Err(error) = little_clip_core::copy_text_to_system_clipboard(&self.content) {
            tracing::debug!(%error, "failed to copy text preview to system clipboard");
        }

        self.status_toast.clear();
    }

    /// Renders the complete preview modal UI.
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close: &mut bool,
    ) {
        let now = crate::ui_ext::feedback_time();

        self.handle_shortcuts(ctx, close);
        self.service_search_jobs(ctx, BackgroundJobs::global(), Instant::now());

        let trailing = usize::from(self.content.ends_with(['\n', '\r']));
        let total_lines = self
            .line_first_fragment
            .len()
            .saturating_sub(trailing)
            .max(1);
        let total_chars = self.content.chars().count();
        let icon = if self.is_markdown {
            Icon::Book
        } else if self.language != "txt" {
            Icon::Code
        } else {
            Icon::Text
        };
        let lang_label = self.language.to_uppercase();
        let title = self.title.clone();

        let editor_hatch = EscapeHatch::new(
            format!("Open in {}", self.editor_cmd),
            self.editor_cmd.clone(),
            format!("Open in {}", self.editor_cmd),
        )
        .with_icon(Icon::Pencil);

        let mut shell = StudioShell::new(title, icon, palette, close)
            .with_badge(lang_label, Some(hex_to_c32(&palette.harmonies.base)))
            .with_badge(format!("{} lines", total_lines), None)
            .with_badge(format!("{} chars", total_chars), None)
            .with_escape_hatch(editor_hatch)
            .with_footer_hints("Ctrl+F: Find • Esc: Close/Dismiss")
            .with_footer_readout(format!("{} lines • {} chars", total_lines, total_chars));

        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                // Header (Zone 1)
                let clicked_hatch = shell.render_header(ui, |ui| {
                    self.render_header_actions(ui, ctx, palette);
                });

                if clicked_hatch.is_some() {
                    self.trigger_escape_hatch();
                }

                if shell.is_closed() {
                    return;
                }

                // Zone 2b: Find Toolbar (if open)
                if self.show_find {
                    if self.view_mode == TextViewMode::SubtitleTimeline {
                        self.show_subtitle_source();
                    }
                    self.render_search_toolbar(ui, palette);
                }

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 3: Main text viewport
                let footer_h = 28.0;
                let avail = ui.available_size();
                let viewport_h = (avail.y - footer_h - 6.0).max(100.0);

                ui.allocate_ui_with_layout(
                    Vec2::new(avail.x, viewport_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| match self.view_mode {
                        TextViewMode::RenderedMarkdown => {
                            self.render_markdown_view(ui, palette);
                        }
                        TextViewMode::SubtitleTimeline => {
                            if let Some(viewer) = &mut self.subtitle_view {
                                viewer.show(ui, palette);
                            }
                        }
                        TextViewMode::Raw => {
                            if let Some(error) = &self.subtitle_error {
                                ui.label(format!("Cue timeline unavailable: {error}"));
                            }
                            self.render_raw_code_view(ui, palette);
                        }
                    },
                );

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 5: Footer
                let toast_tuple = self.status_toast.active(now).cloned();
                shell.render_footer(ui, &toast_tuple, now);
            });
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context, close: &mut bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.show_find {
                self.show_find = false;
            } else {
                *close = true;
            }
        }

        if !ctx.wants_keyboard_input()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
        {
            self.show_find = true;
            self.focus_find_input = true;
        }
    }

    pub(super) fn render_header_command_buttons(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let copy_btn = clip_ui::buttons::ButtonRole::Secondary.button_with_icon(
            Some(Icon::Copy),
            "Copy All",
            palette,
        );
        if ui.add(copy_btn).clicked() {
            self.copy_all_to_clipboard(ctx);
        }

        let find_btn = crate::ui_ext::mode_tab_button_icon(
            Some(Icon::Search),
            "Find (Ctrl+F)",
            self.show_find,
            palette,
        );
        if ui.add(find_btn).clicked() {
            self.show_find = !self.show_find;
            if self.show_find {
                self.focus_find_input = true;
            }
        }
    }

    pub(super) fn render_header_toggle_buttons(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.subtitle_view.is_some() {
            let tabs = [
                crate::ui_tabs::Tab::new(TextViewMode::SubtitleTimeline, "Cue timeline"),
                crate::ui_tabs::Tab::new(TextViewMode::Raw, "Raw / Code"),
            ];
            if let Some(mode) =
                crate::ui_tabs::TabBar::new("subtitle_views", self.view_mode, &tabs, palette)
                    .show(ui)
            {
                if mode == TextViewMode::Raw {
                    self.show_subtitle_source();
                } else {
                    self.view_mode = mode;
                }
            }
            if self.view_mode == TextViewMode::SubtitleTimeline {
                return;
            }
        }

        let wrap_label = if self.wrap_lines {
            "Wrap: ON"
        } else {
            "Wrap: OFF"
        };
        let wrap_btn = crate::ui_ext::mode_tab_button_icon(
            Some(Icon::Wrap),
            wrap_label,
            self.wrap_lines,
            palette,
        );
        if ui
            .add(wrap_btn)
            .on_hover_text("Toggle line-wrapping on/off")
            .clicked()
        {
            self.wrap_lines = !self.wrap_lines;
        }

        if self.view_mode == TextViewMode::Raw {
            let num_label = if self.show_line_numbers {
                "Lines: ON"
            } else {
                "Lines: OFF"
            };
            let num_btn = crate::ui_ext::mode_tab_button_icon(
                Some(Icon::Code),
                num_label,
                self.show_line_numbers,
                palette,
            );
            if ui
                .add(num_btn)
                .on_hover_text("Toggle line numbers gutter")
                .clicked()
            {
                self.show_line_numbers = !self.show_line_numbers;
            }
        }

        if self.is_markdown {
            let tabs = [
                crate::ui_tabs::Tab::new(TextViewMode::RenderedMarkdown, "Rendered MD")
                    .icon(Icon::Book),
                crate::ui_tabs::Tab::new(TextViewMode::Raw, "Raw / Code").icon(Icon::Code),
            ];
            if let Some(mode) =
                crate::ui_tabs::TabBar::new("markdown_views", self.view_mode, &tabs, palette)
                    .show(ui)
            {
                self.view_mode = mode;
            }
        }
    }

    pub(super) fn render_header_actions(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.render_header_command_buttons(ui, ctx, palette);
        self.render_header_toggle_buttons(ui, palette);
    }
}
