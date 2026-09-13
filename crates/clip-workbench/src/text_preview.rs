//! Rich interactive text and code preview workbench view.
//!
//! Provides syntax-highlighted code inspection, rendered Markdown viewing, text selection and copying,
//! toggleable line-wrapping, line numbers gutter, regex find toolbar, and external editor escape hatches.

use eframe::egui::{self, Color32, FontId, Rounding, Stroke, Vec2};
use little_clip_core::config::AppConfig;
use little_clip_core::syntax::detect_language;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// View mode for markdown vs raw code inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextViewMode {
    /// Raw text / code with syntax highlighting and line numbers.
    Raw,
    /// Rendered Markdown with rich formatting.
    RenderedMarkdown,
}

/// A matched range in the text for Find navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
}

/// Interactive UI state for the Big Text Preview modal.
pub struct TextPreviewState {
    pub title: String,
    pub content: String,
    pub path: Option<PathBuf>,
    pub language: &'static str,
    pub is_markdown: bool,
    pub view_mode: TextViewMode,
    pub wrap_lines: bool,
    pub show_line_numbers: bool,
    pub editor_cmd: String,

    // Find state
    pub show_find: bool,
    pub find_query: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub matches: Vec<SearchMatch>,
    pub active_match_idx: usize,
    pub regex_error: Option<String>,
    pub focus_find_input: bool,

    // Status / Toast notification
    pub status_toast: Option<(String, f64)>, // (message, timestamp_secs)

    // Scroll state
    pub scroll_to_line: Option<usize>,
}

pub use little_clip_core::syntax::detect_markdown_text;

impl TextPreviewState {
    /// Creates a new text preview from in-memory text.
    pub fn from_text(text: String, title: Option<String>, config: &AppConfig) -> Self {
        let is_md = detect_markdown_text(&text);
        let lang = if is_md {
            "md"
        } else {
            detect_language(&text, None)
        };
        let title = title.unwrap_or_else(|| {
            if is_md {
                "Markdown Preview".to_string()
            } else if lang != "txt" {
                format!("{} Code Preview", lang.to_uppercase())
            } else {
                "Text Preview".to_string()
            }
        });
        let show_line_numbers = lang != "txt" && !is_md;
        let view_mode = if is_md {
            TextViewMode::RenderedMarkdown
        } else {
            TextViewMode::Raw
        };

        Self {
            title,
            content: text,
            path: None,
            language: lang,
            is_markdown: is_md,
            view_mode,
            wrap_lines: true,
            show_line_numbers,
            editor_cmd: config.text.resolve_editor(),
            show_find: false,
            find_query: String::new(),
            is_regex: false,
            case_sensitive: false,
            matches: Vec::new(),
            active_match_idx: 0,
            regex_error: None,
            focus_find_input: false,
            status_toast: None,
            scroll_to_line: None,
        }
    }

    /// Creates a new text preview from a file on disk.
    pub fn from_file<P: AsRef<Path>>(path: P, config: &AppConfig) -> Option<Self> {
        let path_ref = path.as_ref();
        let content = little_clip_ops::read_text_file_preview(path_ref, 2 * 1024 * 1024)?;
        let ext = path_ref.extension().and_then(|e| e.to_str());
        let lang = detect_language(&content, ext);
        // Only actual .md / .markdown files default to markdown preview
        let is_md = ext.map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")).unwrap_or(false);
        let title = path_ref
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Text file".to_string());
        let show_line_numbers = lang != "txt" && !is_md;
        let view_mode = if is_md {
            TextViewMode::RenderedMarkdown
        } else {
            TextViewMode::Raw
        };

        Some(Self {
            title,
            content,
            path: Some(path_ref.to_path_buf()),
            language: lang,
            is_markdown: is_md,
            view_mode,
            wrap_lines: true,
            show_line_numbers,
            editor_cmd: config.text.resolve_editor(),
            show_find: false,
            find_query: String::new(),
            is_regex: false,
            case_sensitive: false,
            matches: Vec::new(),
            active_match_idx: 0,
            regex_error: None,
            focus_find_input: false,
            status_toast: None,
            scroll_to_line: None,
        })
    }

    /// Recalculates search matches based on query, regex, and case sensitivity.
    pub fn update_search_matches(&mut self) {
        self.matches.clear();
        self.regex_error = None;

        if self.find_query.is_empty() {
            self.active_match_idx = 0;
            return;
        }

        if self.is_regex {
            let pattern = if self.case_sensitive {
                Regex::new(&self.find_query)
            } else {
                Regex::new(&format!("(?i){}", self.find_query))
            };

            match pattern {
                Ok(re) => {
                    let mut line_num = 1;
                    let mut last_idx = 0;
                    for m in re.find_iter(&self.content) {
                        let prefix = &self.content[last_idx..m.start()];
                        line_num += prefix.chars().filter(|&c| c == '\n').count();
                        last_idx = m.start();

                        self.matches.push(SearchMatch {
                            start_byte: m.start(),
                            end_byte: m.end(),
                            line: line_num,
                        });
                    }
                }
                Err(e) => {
                    self.regex_error = Some(e.to_string());
                }
            }
        } else {
            let query = if self.case_sensitive {
                self.find_query.clone()
            } else {
                self.find_query.to_lowercase()
            };

            let text_to_search = if self.case_sensitive {
                self.content.clone()
            } else {
                self.content.to_lowercase()
            };

            let mut line_num = 1;
            let mut last_idx = 0;
            let mut offset = 0;

            while let Some(found) = text_to_search[offset..].find(&query) {
                let start_byte = offset + found;
                let end_byte = start_byte + query.len();

                let prefix = &self.content[last_idx..start_byte];
                line_num += prefix.chars().filter(|&c| c == '\n').count();
                last_idx = start_byte;

                self.matches.push(SearchMatch {
                    start_byte,
                    end_byte,
                    line: line_num,
                });

                offset = end_byte;
                if offset >= text_to_search.len() {
                    break;
                }
            }
        }

        if !self.matches.is_empty() {
            if self.active_match_idx >= self.matches.len() {
                self.active_match_idx = 0;
            }
        } else {
            self.active_match_idx = 0;
        }
    }

    /// Advances to the next search match.
    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.active_match_idx = (self.active_match_idx + 1) % self.matches.len();
            self.scroll_to_line = Some(self.matches[self.active_match_idx].line);
        }
    }

    /// Moves to the previous search match.
    pub fn prev_match(&mut self) {
        if !self.matches.is_empty() {
            if self.active_match_idx == 0 {
                self.active_match_idx = self.matches.len() - 1;
            } else {
                self.active_match_idx -= 1;
            }
            self.scroll_to_line = Some(self.matches[self.active_match_idx].line);
        }
    }

    /// Triggers opening the text in the configured external editor (e.g. VS Code).
    pub fn trigger_escape_hatch(&mut self) {
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

            let temp_name = format!("little_clip_preview_{}.{}", uuid::Uuid::new_v4().simple(), ext);
            let temp_path = std::env::temp_dir().join(temp_name);
            if let Err(e) = std::fs::write(&temp_path, &self.content) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                self.status_toast = Some((format!("Failed to write temporary file: {}", e), now));
                return;
            }
            temp_path
        };

        match Command::new(editor).arg(&target_path).spawn() {
            Ok(_) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                self.status_toast = Some((format!("Opened in {}", editor), now));
            }
            Err(e) => {
                // Try xdg-open fallback
                if Command::new("xdg-open").arg(&target_path).spawn().is_ok() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    self.status_toast = Some(("Opened in system default editor".to_string(), now));
                } else {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    self.status_toast = Some((format!("Failed to launch {}: {}", editor, e), now));
                }
            }
        }
    }

    /// Copies the full text content to the system clipboard.
    pub fn copy_all_to_clipboard(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.content.clone());
        let _ = Command::new("wl-copy").arg(&self.content).output();
        let _ = Command::new("xclip").args(["-selection", "clipboard"]).arg(&self.content).output();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
        self.status_toast = Some(("Copied full text to clipboard! 📋".to_string(), now));
    }

    /// Renders the complete preview modal UI.
    /// Renders the complete preview modal UI.
    pub fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        self.handle_shortcuts(ctx, close);

        egui::Frame::none()
            .fill(ui.visuals().window_fill())
            .inner_margin(egui::Margin::same(16.0))
            .show(ui, |ui| {
                self.render_header(ui, ctx, close);
                self.render_toast(ui, now);

                if self.show_find {
                    self.render_search_toolbar(ui);
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                match self.view_mode {
                    TextViewMode::RenderedMarkdown => {
                        self.render_markdown_view(ui);
                    }
                    TextViewMode::Raw => {
                        self.render_raw_code_view(ui);
                    }
                }
            });
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context, close: &mut bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.show_find {
                self.show_find = false;
            } else {
                *close = true;
            }
        }

        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
            self.show_find = true;
            self.focus_find_input = true;
        }
    }

    fn render_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        let total_lines = self.content.lines().count().max(1);
        let total_chars = self.content.chars().count();

        ui.horizontal(|ui| {
            let icon = if self.is_markdown {
                "📖"
            } else if self.language != "txt" {
                "📝"
            } else {
                "📄"
            };

            ui.label(egui::RichText::new(icon).size(22.0));
            ui.label(
                egui::RichText::new(&self.title)
                    .strong()
                    .size(17.0)
                    .color(ui.visuals().text_color()),
            );

            ui.add_space(8.0);
            let lang_label = self.language.to_uppercase();
            egui::Frame::none()
                .fill(ui.visuals().widgets.noninteractive.bg_fill)
                .rounding(Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(lang_label)
                            .small()
                            .strong()
                            .color(ui.visuals().hyperlink_color),
                    );
                });

            ui.add_space(4.0);
            let info_text = format!("{} lines  •  {} chars", total_lines, total_chars);
            ui.label(
                egui::RichText::new(info_text)
                    .small()
                    .color(Color32::from_rgb(148, 163, 184)),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.render_header_actions(ui, ctx, close);
            });
        });
    }

    fn render_header_command_buttons(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        let close_btn = egui::Button::new(
            egui::RichText::new("✕ Close (Esc)")
                .color(Color32::WHITE)
                .size(13.0),
        )
        .fill(Color32::from_rgb(220, 38, 38))
        .rounding(Rounding::same(4.0));
        if ui.add(close_btn).clicked() {
            *close = true;
        }

        let editor_label = format!("📝 Open in {}", self.editor_cmd);
        let hatch_btn = egui::Button::new(
            egui::RichText::new(editor_label)
                .color(Color32::WHITE)
                .size(13.0),
        )
        .fill(Color32::from_rgb(37, 99, 235))
        .rounding(Rounding::same(4.0));
        if ui.add(hatch_btn).on_hover_text("Open in configured external code editor").clicked() {
            self.trigger_escape_hatch();
        }

        let copy_btn = egui::Button::new(
            egui::RichText::new("📋 Copy All")
                .color(Color32::WHITE)
                .size(13.0),
        )
        .fill(Color32::from_rgb(51, 65, 85))
        .rounding(Rounding::same(4.0));
        if ui.add(copy_btn).clicked() {
            self.copy_all_to_clipboard(ctx);
        }

        let find_btn = egui::Button::new(
            egui::RichText::new("🔍 Find (Ctrl+F)")
                .color(if self.show_find { Color32::from_rgb(250, 204, 21) } else { Color32::WHITE })
                .size(13.0),
        )
        .fill(if self.show_find { Color32::from_rgb(71, 85, 105) } else { Color32::from_rgb(51, 65, 85) })
        .rounding(Rounding::same(4.0));
        if ui.add(find_btn).clicked() {
            self.show_find = !self.show_find;
            if self.show_find {
                self.focus_find_input = true;
            }
        }
    }

    fn render_header_toggle_buttons(&mut self, ui: &mut egui::Ui) {
        let wrap_label = if self.wrap_lines { "↩ Wrap: ON" } else { "➡ Wrap: OFF" };
        let wrap_btn = egui::Button::new(
            egui::RichText::new(wrap_label)
                .color(Color32::WHITE)
                .size(13.0),
        )
        .fill(if self.wrap_lines { Color32::from_rgb(16, 149, 193) } else { Color32::from_rgb(51, 65, 85) })
        .rounding(Rounding::same(4.0));
        if ui.add(wrap_btn).on_hover_text("Toggle line-wrapping on/off").clicked() {
            self.wrap_lines = !self.wrap_lines;
        }

        if self.view_mode == TextViewMode::Raw {
            let num_label = if self.show_line_numbers { "# Lines: ON" } else { "# Lines: OFF" };
            let num_btn = egui::Button::new(
                egui::RichText::new(num_label)
                    .color(Color32::WHITE)
                    .size(13.0),
            )
            .fill(if self.show_line_numbers { Color32::from_rgb(100, 116, 139) } else { Color32::from_rgb(51, 65, 85) })
            .rounding(Rounding::same(4.0));
            if ui.add(num_btn).on_hover_text("Toggle line numbers gutter").clicked() {
                self.show_line_numbers = !self.show_line_numbers;
            }
        }

        if self.is_markdown {
            let mode_label = match self.view_mode {
                TextViewMode::RenderedMarkdown => "📖 Rendered MD",
                TextViewMode::Raw => "💻 Raw / Code",
            };
            let mode_btn = egui::Button::new(
                egui::RichText::new(mode_label)
                    .color(Color32::WHITE)
                    .size(13.0),
            )
            .fill(Color32::from_rgb(13, 148, 136))
            .rounding(Rounding::same(4.0));
            if ui.add(mode_btn).on_hover_text("Switch between Rendered Markdown and Raw Code view").clicked() {
                self.view_mode = match self.view_mode {
                    TextViewMode::RenderedMarkdown => TextViewMode::Raw,
                    TextViewMode::Raw => TextViewMode::RenderedMarkdown,
                };
            }
        }
    }

    fn render_header_actions(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, close: &mut bool) {
        self.render_header_command_buttons(ui, ctx, close);
        self.render_header_toggle_buttons(ui);
    }

    fn render_toast(&self, ui: &mut egui::Ui, now: f64) {
        if let Some((msg, timestamp)) = &self.status_toast {
            if now - timestamp < 4.0 {
                ui.add_space(4.0);
                egui::Frame::none()
                    .fill(Color32::from_rgb(15, 23, 42))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(16, 185, 129)))
                    .rounding(Rounding::same(4.0))
                    .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(msg)
                                .size(11.0)
                                .color(Color32::from_rgb(52, 211, 153)),
                        );
                    });
            }
        }
    }

    fn render_search_status_label(&self, ui: &mut egui::Ui) {
        if self.find_query.is_empty() {
            return;
        }
        if let Some(ref err) = self.regex_error {
            ui.label(
                egui::RichText::new(format!("⚠ Regex error: {}", err))
                    .small()
                    .color(Color32::from_rgb(239, 68, 68)),
            );
        } else if self.matches.is_empty() {
            ui.label(
                egui::RichText::new("No matches")
                    .small()
                    .color(Color32::from_rgb(203, 213, 225)),
            );
        } else {
            let count_text = format!(
                "{}/{} matches",
                self.active_match_idx + 1,
                self.matches.len()
            );
            ui.label(
                egui::RichText::new(count_text)
                    .small()
                    .strong()
                    .color(Color32::from_rgb(250, 204, 21)),
            );
        }
    }

    fn render_search_options(&mut self, ui: &mut egui::Ui) {
        let case_btn = egui::Button::new(
            egui::RichText::new("Aa")
                .size(13.0)
                .strong()
                .color(if self.case_sensitive { Color32::from_rgb(56, 189, 248) } else { Color32::from_rgb(148, 163, 184) }),
        )
        .fill(if self.case_sensitive { Color32::from_rgb(15, 23, 42) } else { Color32::TRANSPARENT });
        if ui.add(case_btn).on_hover_text("Match Case (Case Sensitive)").clicked() {
            self.case_sensitive = !self.case_sensitive;
            self.update_search_matches();
        }

        let regex_btn = egui::Button::new(
            egui::RichText::new(".*")
                .size(13.0)
                .strong()
                .color(if self.is_regex { Color32::from_rgb(56, 189, 248) } else { Color32::from_rgb(148, 163, 184) }),
        )
        .fill(if self.is_regex { Color32::from_rgb(15, 23, 42) } else { Color32::TRANSPARENT });
        if ui.add(regex_btn).on_hover_text("Use Regular Expression").clicked() {
            self.is_regex = !self.is_regex;
            self.update_search_matches();
        }
    }

    fn render_search_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        egui::Frame::none()
            .fill(Color32::from_rgb(30, 41, 59))
            .stroke(Stroke::new(1.0, Color32::from_rgb(71, 85, 105)))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("🔍").size(15.0));

                    let search_edit = egui::TextEdit::singleline(&mut self.find_query)
                        .hint_text("Find in text (Enter for next, Shift+Enter for prev)...")
                        .desired_width(280.0);

                    let response = ui.add(search_edit);
                    if self.focus_find_input {
                        response.request_focus();
                        self.focus_find_input = false;
                    }

                    if response.changed() {
                        self.update_search_matches();
                    }

                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if ui.input(|i| i.modifiers.shift) {
                            self.prev_match();
                        } else {
                            self.next_match();
                        }
                    }

                    self.render_search_status_label(ui);

                    let prev_btn = egui::Button::new("▲ Prev").small();
                    if ui.add(prev_btn).clicked() {
                        self.prev_match();
                    }

                    let next_btn = egui::Button::new("▼ Next").small();
                    if ui.add(next_btn).clicked() {
                        self.next_match();
                    }

                    ui.add_space(8.0);
                    self.render_search_options(ui);
                });
            });
    }

    /// Renders styled markdown view.
    fn render_markdown_view(&mut self, ui: &mut egui::Ui) {
        let parsed = little_clip_render::markdown::parse_markdown(&self.content);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 6.0);

                let mut in_code_block = false;
                let mut code_block_buf = String::new();
                let mut code_block_lang = String::new();

                for line in parsed {
                    render_markdown_element(
                        ui,
                        line,
                        &mut in_code_block,
                        &mut code_block_buf,
                        &mut code_block_lang,
                    );
                }

                flush_code_block(ui, &mut in_code_block, &mut code_block_buf, &mut code_block_lang);
            });
    }

    /// Renders raw / code view with syntax highlighting, line numbers gutter, and search match highlighting.
    fn render_raw_code_view(&mut self, ui: &mut egui::Ui) {
        let syntax = find_syntax(self.language);
        let theme = &THEME_SET.themes["base16-eighties.dark"];

        let show_gutter = self.show_line_numbers;
        let wrap = self.wrap_lines;
        let query = if self.find_query.is_empty() { None } else { Some(self.find_query.as_str()) };
        let is_regex = self.is_regex;
        let case_sensitive = self.case_sensitive;
        let active_match = self.matches.get(self.active_match_idx).copied();

        let mut scroll_area = egui::ScrollArea::both()
            .auto_shrink([false, false]);

        if let Some(target_line) = self.scroll_to_line.take() {
            let approx_y = (target_line.saturating_sub(5)) as f32 * 18.0;
            scroll_area = scroll_area.vertical_scroll_offset(approx_y);
        }

        // Unified editor surface frame
        egui::Frame::none()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().window_stroke())
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                scroll_area.show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        // Line numbers gutter inside editor surface
                        if show_gutter {
                            let total_lines = self.content.lines().count().max(1);
                            let digits = total_lines.to_string().len().max(2);
                            let mut line_numbers_text = String::with_capacity(total_lines * (digits + 2));
                            for i in 1..=total_lines {
                                line_numbers_text.push_str(&format!("{:>width$}\n", i, width = digits));
                            }

                            ui.label(
                                egui::RichText::new(line_numbers_text)
                                    .font(FontId::monospace(12.5))
                                    .color(ui.visuals().weak_text_color()),
                            );

                            ui.add_space(8.0);
                            let h = (total_lines as f32 * 17.5).max(ui.available_height());
                            let (div_rect, _) = ui.allocate_exact_size(Vec2::new(1.0, h), egui::Sense::hover());
                            ui.painter().vline(div_rect.center().x, div_rect.y_range(), ui.visuals().widgets.noninteractive.bg_stroke);
                            ui.add_space(8.0);
                        }

                        let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
                            let mut layout_job = build_highlighted_layout_job(
                                text,
                                syntax,
                                theme,
                                query,
                                is_regex,
                                case_sensitive,
                                active_match,
                            );
                            if wrap {
                                layout_job.wrap.max_width = wrap_width;
                            } else {
                                layout_job.wrap.max_width = f32::INFINITY;
                            }
                            ui.fonts(|f| f.layout_job(layout_job))
                        };

                        let text_edit = egui::TextEdit::multiline(&mut self.content)
                            .font(FontId::monospace(12.5))
                            .layouter(&mut layouter)
                            .desired_width(f32::INFINITY)
                            .lock_focus(false)
                            .interactive(true)
                            .frame(false)
                            .margin(egui::Margin::ZERO);

                        ui.add(text_edit);
                    });
                });
            });
    }
}

fn flush_code_block(ui: &mut egui::Ui, in_code: &mut bool, buf: &mut String, lang: &mut String) {
    if *in_code {
        render_styled_code_block(ui, buf, lang);
        buf.clear();
        lang.clear();
        *in_code = false;
    }
}

fn render_markdown_element(
    ui: &mut egui::Ui,
    line: little_clip_render::markdown::MarkdownLine,
    in_code_block: &mut bool,
    code_block_buf: &mut String,
    code_block_lang: &mut String,
) {
    match line {
        little_clip_render::markdown::MarkdownLine::CodeFence { language } => {
            if *in_code_block {
                flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            } else {
                *in_code_block = true;
                *code_block_lang = language;
            }
        }
        little_clip_render::markdown::MarkdownLine::CodeLine { line } => {
            if *in_code_block {
                code_block_buf.push_str(&line);
                code_block_buf.push('\n');
            }
        }
        little_clip_render::markdown::MarkdownLine::Heading { level, text: _, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let (size, color) = match level {
                    1 => (20.0, Color32::from_rgb(248, 250, 252)),
                    2 => (17.0, Color32::from_rgb(226, 232, 240)),
                    _ => (15.0, Color32::from_rgb(203, 213, 225)),
                };
                for span in spans {
                    render_markdown_span(ui, &span, size, color);
                }
            });
            ui.add_space(4.0);
        }
        little_clip_render::markdown::MarkdownLine::Bullet { indent, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent as f32 * 16.0);
                ui.label(egui::RichText::new("•").size(14.0).color(Color32::from_rgb(56, 189, 248)));
                ui.add_space(4.0);
                for span in spans {
                    render_markdown_span(ui, &span, 14.0, Color32::from_rgb(241, 245, 249));
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Numbered { number, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("{}.", number)).size(14.0).strong().color(Color32::from_rgb(56, 189, 248)));
                ui.add_space(4.0);
                for span in spans {
                    render_markdown_span(ui, &span, 14.0, Color32::from_rgb(241, 245, 249));
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Quote { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            egui::Frame::none()
                .fill(Color32::from_rgb(30, 41, 59))
                .stroke(Stroke::new(3.0, Color32::from_rgb(56, 189, 248)))
                .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                .rounding(Rounding::same(3.0))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for span in spans {
                            render_markdown_span(ui, &span, 13.5, Color32::from_rgb(203, 213, 225));
                        }
                    });
                });
        }
        little_clip_render::markdown::MarkdownLine::Rule => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
        }
        little_clip_render::markdown::MarkdownLine::Paragraph { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang);
            ui.horizontal_wrapped(|ui| {
                for span in spans {
                    render_markdown_span(ui, &span, 14.0, Color32::from_rgb(241, 245, 249));
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Empty => {
            if *in_code_block {
                code_block_buf.push('\n');
            } else {
                ui.add_space(6.0);
            }
        }
    }
}

/// Helper to find syntax definition for language string.
fn find_syntax(lang: &str) -> &'static SyntaxReference {
    let ps = &*SYNTAX_SET;
    ps.find_syntax_by_token(lang)
        .or_else(|| ps.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ps.find_syntax_plain_text())
}

fn compute_highlight_matches(
    text: &str,
    search_query: Option<&str>,
    is_regex: bool,
    case_sensitive: bool,
    active_match: Option<SearchMatch>,
) -> Vec<(usize, usize, bool)> {
    let mut match_ranges = Vec::new();
    let query = match search_query {
        Some(q) if !q.is_empty() => q,
        _ => return match_ranges,
    };

    if is_regex {
        let pattern = if case_sensitive {
            Regex::new(query)
        } else {
            Regex::new(&format!("(?i){}", query))
        };
        if let Ok(re) = pattern {
            for m in re.find_iter(text) {
                let is_active = active_match.map(|am| am.start_byte == m.start()).unwrap_or(false);
                match_ranges.push((m.start(), m.end(), is_active));
            }
        }
    } else {
        let q = if case_sensitive { query.to_string() } else { query.to_lowercase() };
        let target = if case_sensitive { text.to_string() } else { text.to_lowercase() };
        let mut offset = 0;
        while let Some(found) = target[offset..].find(&q) {
            let start = offset + found;
            let end = start + q.len();
            let is_active = active_match.map(|am| am.start_byte == start).unwrap_or(false);
            match_ranges.push((start, end, is_active));
            offset = end;
            if offset >= target.len() {
                break;
            }
        }
    }

    match_ranges
}

/// Builds an egui LayoutJob with Syntect syntax highlighting and search match background highlights.
fn build_highlighted_layout_job(
    text: &str,
    syntax: &SyntaxReference,
    theme: &Theme,
    search_query: Option<&str>,
    is_regex: bool,
    case_sensitive: bool,
    active_match: Option<SearchMatch>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut highlighter = HighlightLines::new(syntax, theme);

    let match_ranges = compute_highlight_matches(text, search_query, is_regex, case_sensitive, active_match);

    let mut byte_offset = 0;

    for line in text.split_inclusive('\n') {
        let ranges = highlighter
            .highlight_line(line, &SYNTAX_SET)
            .unwrap_or_default();

        for (style, token_str) in ranges {
            let token_start = byte_offset;
            let token_end = token_start + token_str.len();

            let fg = Color32::from_rgb(style.foreground.r, style.foreground.g, style.foreground.b);

            // Check if this token intersects with any search matches
            let mut has_match = false;
            let mut is_active = false;
            for &(m_start, m_end, active) in &match_ranges {
                if token_start < m_end && token_end > m_start {
                    has_match = true;
                    if active {
                        is_active = true;
                    }
                }
            }

            let bg = if is_active {
                Color32::from_rgb(202, 138, 4) // Amber active match highlight
            } else if has_match {
                Color32::from_rgb(113, 63, 18) // Muted amber match highlight
            } else {
                Color32::TRANSPARENT
            };

            let text_color = if is_active {
                Color32::WHITE
            } else {
                fg
            };

            job.append(
                token_str,
                0.0,
                egui::TextFormat {
                    font_id: FontId::monospace(12.5),
                    color: text_color,
                    background: bg,
                    ..Default::default()
                },
            );

            byte_offset += token_str.len();
        }
    }

    job
}

/// Renders a single markdown inline span into egui UI.
fn render_markdown_span(
    ui: &mut egui::Ui,
    span: &little_clip_render::markdown::MarkdownSpan,
    size: f32,
    color: Color32,
) {
    match span {
        little_clip_render::markdown::MarkdownSpan::Text(s) => {
            ui.label(egui::RichText::new(s).size(size).color(color));
        }
        little_clip_render::markdown::MarkdownSpan::Bold(s) => {
            ui.label(egui::RichText::new(s).size(size).strong().color(color));
        }
        little_clip_render::markdown::MarkdownSpan::Italic(s) => {
            ui.label(egui::RichText::new(s).size(size).italics().color(color));
        }
        little_clip_render::markdown::MarkdownSpan::BoldItalic(s) => {
            ui.label(egui::RichText::new(s).size(size).strong().italics().color(color));
        }
        little_clip_render::markdown::MarkdownSpan::Code(s) => {
            egui::Frame::none()
                .fill(Color32::from_rgb(30, 41, 59))
                .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                .rounding(Rounding::same(3.0))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(s)
                            .font(FontId::monospace(size * 0.9))
                            .color(Color32::from_rgb(244, 114, 182)), // Pink
                    );
                });
        }
        little_clip_render::markdown::MarkdownSpan::Link { label, url } => {
            ui.hyperlink_to(
                egui::RichText::new(label)
                    .size(size)
                    .color(Color32::from_rgb(56, 189, 248))
                    .underline(),
                url,
            );
        }
    }
}

/// Renders a styled code block for markdown view.
fn render_styled_code_block(ui: &mut egui::Ui, code: &str, lang: &str) {
    let syntax = find_syntax(lang);
    let theme = &THEME_SET.themes["base16-eighties.dark"];

    egui::Frame::none()
        .fill(Color32::from_rgb(15, 23, 42))
        .stroke(Stroke::new(1.0, Color32::from_rgb(51, 65, 85)))
        .inner_margin(egui::Margin::same(10.0))
        .rounding(Rounding::same(4.0))
        .show(ui, |ui| {
            if !lang.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lang.to_uppercase())
                            .small()
                            .color(Color32::from_rgb(148, 163, 184)),
                    );
                });
                ui.add_space(4.0);
            }

            let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let mut layout_job = build_highlighted_layout_job(
                    text,
                    syntax,
                    theme,
                    None,
                    false,
                    false,
                    None,
                );
                layout_job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(layout_job))
            };

            let mut code_clone = code.to_string();
            let text_edit = egui::TextEdit::multiline(&mut code_clone)
                .font(FontId::monospace(12.0))
                .layouter(&mut layouter)
                .desired_width(f32::INFINITY)
                .lock_focus(false)
                .interactive(true);

            ui.add(text_edit);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_preview_creation_and_search() {
        let config = AppConfig::default();
        let content = "fn main() {\n    let answer = 42;\n    println!(\"{}\", answer);\n}".to_string();
        let mut state = TextPreviewState::from_text(content, None, &config);

        assert_eq!(state.language, "rs");
        assert!(state.show_line_numbers);
        assert_eq!(state.view_mode, TextViewMode::Raw);

        // Test Find query
        state.find_query = "answer".to_string();
        state.update_search_matches();
        assert_eq!(state.matches.len(), 2);
        assert_eq!(state.active_match_idx, 0);

        state.next_match();
        assert_eq!(state.active_match_idx, 1);

        state.next_match();
        assert_eq!(state.active_match_idx, 0);

        state.prev_match();
        assert_eq!(state.active_match_idx, 1);
    }

    #[test]
    fn test_text_preview_regex_search() {
        let config = AppConfig::default();
        let content = "const VALUE_A = 100;\nconst VALUE_B = 200;\nconst OTHER = 300;".to_string();
        let mut state = TextPreviewState::from_text(content, None, &config);

        state.find_query = r"VALUE_[A-Z]".to_string();
        state.is_regex = true;
        state.update_search_matches();

        assert!(state.regex_error.is_none());
        assert_eq!(state.matches.len(), 2);

        // Invalid regex
        state.find_query = r"[invalid".to_string();
        state.update_search_matches();
        assert!(state.regex_error.is_some());
        assert_eq!(state.matches.len(), 0);
    }

    #[test]
    fn test_markdown_preview_mode() {
        let config = AppConfig::default();
        let content = "# Title\n\n- item 1\n- item 2\n\n```rust\nfn test() {}\n```".to_string();
        let state = TextPreviewState::from_text(content, None, &config);

        assert!(state.is_markdown);
        assert_eq!(state.view_mode, TextViewMode::RenderedMarkdown);
    }

    #[test]
    fn test_c_code_not_markdown() {
        let config = AppConfig::default();
        let content = "#include <stdio.h>\n#include <math.h>\n\nint main(void) {\n    printf(\"pi\");\n    return 0;\n}".to_string();
        let state = TextPreviewState::from_text(content, None, &config);

        assert_eq!(state.language, "c");
        assert!(!state.is_markdown);
        assert_eq!(state.view_mode, TextViewMode::Raw);
        assert!(state.show_line_numbers);
    }

    #[test]
    fn test_python_code_not_markdown() {
        let config = AppConfig::default();
        let content = "# A python script\nimport math\n\ndef calculate(n):\n    return math.pi * n".to_string();
        let state = TextPreviewState::from_text(content, None, &config);

        assert_eq!(state.language, "py");
        assert!(!state.is_markdown);
        assert_eq!(state.view_mode, TextViewMode::Raw);
        assert!(state.show_line_numbers);
    }

    #[test]
    fn test_gutter_text_alignment() {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    let gutter_resp = ui.label(egui::RichText::new("1\n2\n3").font(FontId::monospace(12.5)));
                    let mut text = "#include <stdio.h>\nint main() {}\n".to_string();
                    let syntax = find_syntax("c");
                    let theme = &THEME_SET.themes["base16-eighties.dark"];
                    let mut layouter = |ui: &egui::Ui, text: &str, _wrap_width: f32| {
                        let layout_job = build_highlighted_layout_job(
                            text,
                            syntax,
                            theme,
                            None,
                            false,
                            false,
                            None,
                        );
                        ui.fonts(|f| f.layout_job(layout_job))
                    };
                    let edit = egui::TextEdit::multiline(&mut text)
                        .font(FontId::monospace(12.5))
                        .layouter(&mut layouter)
                        .frame(false)
                        .margin(egui::Margin::ZERO);
                    let edit_resp = ui.add(edit);

                    assert_eq!(gutter_resp.rect.min.y, edit_resp.rect.min.y);
                });
            });
        });

        // Verify that the gutter text shape and the editor text shape share the exact same top Y position and row heights
        let mut text_shapes = Vec::new();
        for s in &output.shapes {
            if let egui::epaint::Shape::Text(t) = &s.shape {
                text_shapes.push(t);
            }
        }
        assert_eq!(text_shapes.len(), 2, "Expected gutter text and code text shapes");
        let gutter_text = text_shapes[0];
        let code_text = text_shapes[1];

        assert_eq!(gutter_text.pos.y, code_text.pos.y, "Gutter and code must align on identical Y coordinate");
        assert_eq!(gutter_text.galley.rows.len(), code_text.galley.rows.len(), "Same number of rows");
        for (g_row, c_row) in gutter_text.galley.rows.iter().zip(code_text.galley.rows.iter()) {
            assert_eq!(g_row.rect.min.y, c_row.rect.min.y, "Row min Y must align");
            assert_eq!(g_row.rect.height(), c_row.rect.height(), "Row heights must be identical");
        }
    }
}

