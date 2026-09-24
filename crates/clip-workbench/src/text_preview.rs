//! Rich interactive text and code preview workbench view.
//!
//! Provides syntax-highlighted code inspection, rendered Markdown viewing, text selection and copying,
//! toggleable line-wrapping, line numbers gutter, regex find toolbar, and external editor escape hatches.

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, FontId, Rounding, Stroke, Vec2};
use little_clip_core::config::AppConfig;
use little_clip_core::syntax::detect_language;
use little_clip_core::theme::AppPalette;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Maps an RGB syntax highlighting token color from syntect into the active Hundred Rabbits theme harmonies.
fn map_syntect_color_to_theme(
    style_fg: syntect::highlighting::Color,
    palette: &AppPalette,
) -> Color32 {
    let r = style_fg.r as f32 / 255.0;
    let g = style_fg.g as f32 / 255.0;
    let b = style_fg.b as f32 / 255.0;

    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    // If low saturation (monochrome / gray)
    if delta < 0.08 {
        if max < 0.65 {
            // Comment or muted punctuation
            return hex_to_c32(&palette.theme.f_low);
        } else {
            // Standard high contrast text
            return hex_to_c32(&palette.theme.f_high);
        }
    }

    // Compute hue (0..360)
    let mut h = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    if h < 0.0 {
        h += 360.0;
    }

    // Map by hue angle to theme harmonies
    if h < 45.0 {
        // Red / Orange (constants, literals)
        hex_to_c32(&palette.diff_mod)
    } else if h < 90.0 {
        // Amber / Yellow (strings)
        hex_to_c32(&palette.harmonies.triadic_2)
    } else if h < 165.0 {
        // Green (strings, regex)
        hex_to_c32(&palette.diff_add)
    } else if h < 210.0 {
        // Cyan (types, structures)
        hex_to_c32(&palette.harmonies.split_comp_1)
    } else if h < 275.0 {
        // Blue (functions, identifiers)
        hex_to_c32(&palette.harmonies.triadic_1)
    } else {
        // Purple / Magenta / Pink (keywords, control flow)
        hex_to_c32(&palette.harmonies.base)
    }
}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

const MINIMAP_COLUMNS: usize = 96;
const MINIMAP_WIDTH: f32 = 112.0;

/// A syntax-colored, character-free slice of one source line for the minimap.
#[derive(Debug, Clone)]
struct MinimapSegment {
    start_column: usize,
    end_column: usize,
    color: syntect::highlighting::Color,
}

#[derive(Debug, Clone, Default)]
struct MinimapLine {
    segments: Vec<MinimapSegment>,
}

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
    pub status_toast: crate::ui_ext::StatusToast,

    // Scroll state
    pub scroll_to_line: Option<usize>,
    /// Exact pixel offset requested by minimap navigation.
    scroll_to_offset: Option<f32>,
    /// Byte offsets for each line. This is built once when the preview opens so
    /// large files do not allocate/scan all lines every repaint.
    line_starts: Vec<usize>,
    pub minimap_fraction: f32,
    minimap_view_fraction: f32,
    minimap_content_height: f32,
    minimap_viewport_height: f32,
    minimap_lines: Option<Vec<MinimapLine>>,
    minimap_overflows: bool,
}

fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

pub use little_clip_core::syntax::detect_markdown_text;

impl TextPreviewState {
    /// Creates a new text preview from in-memory text.
    pub fn from_text(text: String, title: Option<String>, config: &AppConfig) -> Self {
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_string();
        let line_starts = line_start_offsets(&text);
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
            status_toast: crate::ui_ext::StatusToast::new(),
            scroll_to_line: None,
            scroll_to_offset: None,
            line_starts,
            minimap_fraction: 0.0,
            minimap_view_fraction: 1.0,
            minimap_content_height: 1.0,
            minimap_viewport_height: 1.0,
            minimap_lines: None,
            minimap_overflows: false,
        }
    }

    /// Creates a new text preview from a file on disk.
    pub fn from_file<P: AsRef<Path>>(path: P, config: &AppConfig) -> Option<Self> {
        let path_ref = path.as_ref();
        let content = little_clip_ops::read_text_file_preview(path_ref, 2 * 1024 * 1024)?;
        let line_starts = line_start_offsets(&content);
        let ext = path_ref.extension().and_then(|e| e.to_str());
        let lang = detect_language(&content, ext);
        // Only actual .md / .markdown files default to markdown preview
        let is_md = ext
            .map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
            .unwrap_or(false);
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
            status_toast: crate::ui_ext::StatusToast::new(),
            scroll_to_line: None,
            scroll_to_offset: None,
            line_starts,
            minimap_fraction: 0.0,
            minimap_view_fraction: 1.0,
            minimap_content_height: 1.0,
            minimap_viewport_height: 1.0,
            minimap_lines: None,
            minimap_overflows: false,
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

            let temp_name = format!(
                "little_clip_preview_{}.{}",
                uuid::Uuid::new_v4().simple(),
                ext
            );
            let temp_path = std::env::temp_dir().join(temp_name);
            if let Err(e) = std::fs::write(&temp_path, &self.content) {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                self.status_toast
                    .set(format!("Failed to write temporary file: {}", e), now);
                return;
            }
            temp_path
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        EscapeHatchLauncher::launch(editor, editor, &target_path, &mut self.status_toast, now);
    }

    /// Copies the full text content to the system clipboard.
    pub fn copy_all_to_clipboard(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.content.clone());
        if let Err(error) = little_clip_core::copy_text_to_system_clipboard(&self.content) {
            tracing::debug!(%error, "failed to copy text preview to system clipboard");
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.status_toast.set("Copied full text to clipboard!", now);
    }

    /// Renders the complete preview modal UI.
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close: &mut bool,
    ) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        self.handle_shortcuts(ctx, close);

        let total_lines = self.content.lines().count().max(1);
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
            .fill(hex_to_c32(&palette.theme.background))
            .inner_margin(egui::Margin::same(12.0))
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
                    self.render_search_toolbar(ui, palette);
                }

                ui.add_space(4.0);

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
                        TextViewMode::Raw => {
                            self.render_raw_code_view(ui, palette);
                        }
                    },
                );

                ui.add_space(4.0);

                // Zone 5: Footer
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
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

    fn render_header_command_buttons(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        let copy_btn = crate::ui_ext::secondary_button_icon(
            Some(Icon::Copy),
            "Copy All",
            hex_to_c32(&palette.theme.b_med),
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

    fn render_header_toggle_buttons(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
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
            let (mode_label, mode_icon) = match self.view_mode {
                TextViewMode::RenderedMarkdown => ("Rendered MD", Icon::Book),
                TextViewMode::Raw => ("Raw / Code", Icon::Code),
            };
            let mode_btn = crate::ui_ext::mode_tab_button_icon(
                Some(mode_icon),
                mode_label,
                self.view_mode == TextViewMode::RenderedMarkdown,
                palette,
            );
            if ui
                .add(mode_btn)
                .on_hover_text("Switch between Rendered Markdown and Raw Code view")
                .clicked()
            {
                self.view_mode = match self.view_mode {
                    TextViewMode::RenderedMarkdown => TextViewMode::Raw,
                    TextViewMode::Raw => TextViewMode::RenderedMarkdown,
                };
            }
        }
    }

    fn render_header_actions(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
    ) {
        self.render_header_command_buttons(ui, ctx, palette);
        self.render_header_toggle_buttons(ui, palette);
    }

    fn render_search_status_label(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.find_query.is_empty() {
            return;
        }
        if let Some(ref err) = self.regex_error {
            ui.horizontal(|ui| {
                Icon::Alert.ui(ui, 13.0, hex_to_c32(&palette.diff_del));
                ui.label(
                    egui::RichText::new(format!("Regex error: {}", err))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(hex_to_c32(&palette.diff_del)),
                );
            });
        } else if self.matches.is_empty() {
            ui.label(
                egui::RichText::new("No matches")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.theme.f_low)),
            );
        } else {
            let count_text = format!(
                "{}/{} matches",
                self.active_match_idx + 1,
                self.matches.len()
            );
            ui.label(
                egui::RichText::new(count_text)
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(hex_to_c32(&palette.harmonies.complementary)),
            );
        }
    }

    fn render_search_options(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let case_btn = crate::mode_tab_button("Aa", self.case_sensitive, palette);
        if ui
            .add(case_btn)
            .on_hover_text("Match Case (Case Sensitive)")
            .clicked()
        {
            self.case_sensitive = !self.case_sensitive;
            self.update_search_matches();
        }

        let regex_btn = crate::mode_tab_button(".*", self.is_regex, palette);
        if ui
            .add(regex_btn)
            .on_hover_text("Use Regular Expression")
            .clicked()
        {
            self.is_regex = !self.is_regex;
            self.update_search_matches();
        }
    }

    fn render_search_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.add_space(8.0);
        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    Icon::Search.ui(ui, 15.0, hex_to_c32(&palette.theme.f_med));

                    let search_edit = egui::TextEdit::singleline(&mut self.find_query)
                        .hint_text("Find in text (Enter for next, Shift+Enter for prev)...");

                    let response =
                        ui.add_sized([280.0, crate::ui_ext::CONTROL_HEIGHT], search_edit);
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

                    self.render_search_status_label(ui, palette);

                    let prev_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::ArrowUp),
                        "Prev",
                        hex_to_c32(&palette.theme.b_med),
                        palette,
                    );
                    if ui.add(prev_btn).clicked() {
                        self.prev_match();
                    }

                    let next_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::ArrowDown),
                        "Next",
                        hex_to_c32(&palette.theme.b_med),
                        palette,
                    );
                    if ui.add(next_btn).clicked() {
                        self.next_match();
                    }

                    ui.add_space(8.0);
                    self.render_search_options(ui, palette);
                });
            });
    }

    /// Renders styled markdown view.
    fn render_markdown_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
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
                        palette,
                    );
                }

                flush_code_block(
                    ui,
                    &mut in_code_block,
                    &mut code_block_buf,
                    &mut code_block_lang,
                    palette,
                );
            });
    }

    /// Renders raw / code view with syntax highlighting, line numbers gutter, and search match highlighting.
    fn render_raw_code_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let syntax = find_syntax(self.language);

        let show_gutter = self.show_line_numbers;
        let query = if self.find_query.is_empty() {
            None
        } else {
            Some(self.find_query.as_str())
        };
        let is_regex = self.is_regex;
        let case_sensitive = self.case_sensitive;
        let active_match = self.matches.get(self.active_match_idx).copied();

        let mut scroll_area = egui::ScrollArea::vertical()
            .id_source("text_code_virtual_scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);

        if let Some(offset) = self.scroll_to_offset.take() {
            scroll_area = scroll_area.vertical_scroll_offset(offset);
        } else if let Some(target_line) = self.scroll_to_line.take() {
            let approx_y = (target_line.saturating_sub(5)) as f32 * 18.0;
            scroll_area = scroll_area.vertical_scroll_offset(approx_y);
        }

        // Unified editor surface frame
        egui::Frame::none()
            .fill(hex_to_c32(&palette.theme.b_low))
            .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
            .rounding(Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                let total_lines = self.line_starts.len().max(1);
                let digits = total_lines.to_string().len().max(2);
                let line_height = 18.0;
                // A horizontal layout shrink-wraps children by default.  That left the
                // virtual ScrollArea with a one-row-high clip rect on some egui backends.
                // Reserve the viewport explicitly before asking it to virtualize rows.
                let surface_size = ui.available_size();
                // The scroll area reports its actual content height below. Use that
                // measurement from the previous frame to avoid showing a minimap for
                // files that fit, while still allowing wrapping to affect overflow.
                let show_minimap = self.minimap_overflows;
                let minimap_width = if show_minimap { MINIMAP_WIDTH } else { 0.0 };
                let editor_width = (surface_size.x - minimap_width - 6.0).max(64.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(editor_width, surface_size.y.max(1.0)),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let output =
                                scroll_area.show_rows(ui, line_height, total_lines, |ui, rows| {
                                    for line_index in rows {
                                        let start = self.line_starts[line_index];
                                        let end = self
                                            .line_starts
                                            .get(line_index + 1)
                                            .copied()
                                            .unwrap_or(self.content.len());
                                        let line = self.content[start..end].trim_end_matches('\n');
                                        ui.horizontal(|ui| {
                                            if show_gutter {
                                                ui.add_sized(
                                                    [((digits + 1) * 8) as f32, line_height],
                                                    egui::Label::new(
                                                        egui::RichText::new(format!(
                                                            "{:>width$}",
                                                            line_index + 1,
                                                            width = digits
                                                        ))
                                                        .font(FontId::monospace(12.5))
                                                        .color(hex_to_c32(&palette.theme.f_low)),
                                                    ),
                                                );
                                                ui.separator();
                                            }
                                            let mut job = build_highlighted_layout_job(
                                                line,
                                                syntax,
                                                palette,
                                                query,
                                                is_regex,
                                                case_sensitive,
                                                active_match,
                                            );
                                            job.wrap.max_width = if self.wrap_lines {
                                                ui.available_width()
                                            } else {
                                                f32::INFINITY
                                            };
                                            // `add_sized(available_width)` inside a horizontal
                                            // row makes egui align the galley to the far edge of
                                            // that oversized allocation.  Let the label occupy its
                                            // natural, left-anchored layout space instead.
                                            ui.add(egui::Label::new(job).wrap(self.wrap_lines));
                                        });
                                    }
                                });
                            self.minimap_fraction = (output.state.offset.y
                                / (output.content_size.y - output.inner_rect.height()).max(1.0))
                            .clamp(0.0, 1.0);
                            self.minimap_view_fraction = (output.inner_rect.height()
                                / output.content_size.y.max(1.0))
                            .clamp(0.0, 1.0);
                            self.minimap_content_height = output.content_size.y;
                            self.minimap_viewport_height = output.inner_rect.height();
                            self.minimap_overflows =
                                output.content_size.y > output.inner_rect.height();
                            if self.minimap_overflows && self.minimap_lines.is_none() {
                                self.minimap_lines =
                                    Some(build_minimap_lines(&self.content, syntax));
                            }
                        },
                    );
                    if show_minimap {
                        if let Some(fraction) = render_code_minimap(
                            ui,
                            self.minimap_lines.as_deref().unwrap_or_default(),
                            surface_size.y.max(1.0),
                            self.minimap_fraction,
                            self.minimap_view_fraction,
                            palette,
                        ) {
                            let max_offset = (self.minimap_content_height
                                - self.minimap_viewport_height)
                                .max(0.0);
                            self.scroll_to_offset = Some(
                                (fraction * self.minimap_content_height
                                    - self.minimap_viewport_height * 0.5)
                                    .clamp(0.0, max_offset),
                            );
                        }
                    }
                });
            });
    }
}

fn flush_code_block(
    ui: &mut egui::Ui,
    in_code: &mut bool,
    buf: &mut String,
    lang: &mut String,
    palette: &AppPalette,
) {
    if *in_code {
        render_styled_code_block(ui, buf, lang, palette);
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
    palette: &AppPalette,
) {
    match line {
        little_clip_render::markdown::MarkdownLine::CodeFence { language } => {
            if *in_code_block {
                flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
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
        little_clip_render::markdown::MarkdownLine::Heading {
            level,
            text: _,
            spans,
        } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let (size, color) = match level {
                    1 => (20.0, hex_to_c32(&palette.theme.f_high)),
                    2 => (17.0, hex_to_c32(&palette.theme.f_high)),
                    _ => (15.0, hex_to_c32(&palette.theme.f_med)),
                };
                for span in spans {
                    render_markdown_span(ui, &span, size, color, palette);
                }
            });
            ui.add_space(4.0);
        }
        little_clip_render::markdown::MarkdownLine::Bullet { indent, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent as f32 * 16.0);
                ui.label(
                    egui::RichText::new("•")
                        .size(14.0)
                        .color(hex_to_c32(&palette.harmonies.base)),
                );
                ui.add_space(4.0);
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        14.0,
                        hex_to_c32(&palette.theme.f_high),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Numbered { number, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}.", number))
                        .size(14.0)
                        .strong()
                        .color(hex_to_c32(&palette.harmonies.base)),
                );
                ui.add_space(4.0);
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        14.0,
                        hex_to_c32(&palette.theme.f_high),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Quote { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            egui::Frame::none()
                .fill(hex_to_c32(&palette.theme.b_low))
                .stroke(Stroke::new(3.0_f32, hex_to_c32(&palette.harmonies.base)))
                .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                .rounding(Rounding::same(3.0))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for span in spans {
                            render_markdown_span(
                                ui,
                                &span,
                                13.5,
                                hex_to_c32(&palette.theme.f_med),
                                palette,
                            );
                        }
                    });
                });
        }
        little_clip_render::markdown::MarkdownLine::Rule => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
        }
        little_clip_render::markdown::MarkdownLine::Paragraph { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        14.0,
                        hex_to_c32(&palette.theme.f_high),
                        palette,
                    );
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

/// Precomputes compact syntax spans once, avoiding text layout and per-frame parsing.
fn build_minimap_lines(text: &str, syntax: &SyntaxReference) -> Vec<MinimapLine> {
    let syn_theme = &THEME_SET.themes["base16-eighties.dark"];
    let mut highlighter = HighlightLines::new(syntax, syn_theme);

    text.split('\n')
        .map(|line| {
            let mut column = 0;
            let mut segments = Vec::new();
            for (style, token) in highlighter
                .highlight_line(line, &SYNTAX_SET)
                .unwrap_or_default()
            {
                let token_width = token.chars().count();
                let leading_space = token.len() - token.trim_start().len();
                let trailing_space = token.len() - token.trim_end().len();
                let visible_start = column + token[..leading_space].chars().count();
                let visible_end =
                    column + token_width.saturating_sub(token[..trailing_space].chars().count());
                if visible_start < visible_end && visible_start < MINIMAP_COLUMNS {
                    segments.push(MinimapSegment {
                        start_column: visible_start,
                        end_column: visible_end.min(MINIMAP_COLUMNS),
                        color: style.foreground,
                    });
                }
                column += token_width;
            }
            MinimapLine { segments }
        })
        .collect()
}

/// Draws a VS Code-style overview from syntax-colored rectangles, not glyphs.
fn render_code_minimap(
    ui: &mut egui::Ui,
    lines: &[MinimapLine],
    height: f32,
    scroll_fraction: f32,
    view_fraction: f32,
    palette: &AppPalette,
) -> Option<f32> {
    let (map_rect, response) = ui.allocate_exact_size(
        Vec2::new(MINIMAP_WIDTH, height),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter();
    painter.rect_filled(
        map_rect,
        Rounding::same(3.0),
        hex_to_c32(&palette.theme.b_med),
    );

    let content_rect = map_rect.shrink2(Vec2::new(4.0, 3.0));
    let line_count = lines.len().max(1);
    // Do not emit thousands of tiny shapes for large sources. Each sampled row
    // still represents its source position, retaining the document's color rhythm.
    let stride =
        ((line_count as f32 / content_rect.height().ceil().max(1.0)).ceil() as usize).max(1);
    for line_index in (0..lines.len()).step_by(stride) {
        let row_top =
            content_rect.top() + content_rect.height() * line_index as f32 / line_count as f32;
        let row_bottom = (content_rect.top()
            + content_rect.height() * (line_index + stride).min(line_count) as f32
                / line_count as f32)
            .max(row_top + 1.0)
            .min(content_rect.bottom());
        for segment in &lines[line_index].segments {
            let x_start = content_rect.left()
                + content_rect.width() * segment.start_column as f32 / MINIMAP_COLUMNS as f32;
            let x_end = content_rect.left()
                + content_rect.width() * segment.end_column as f32 / MINIMAP_COLUMNS as f32;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x_start, row_top),
                    egui::pos2(x_end, row_bottom),
                ),
                Rounding::ZERO,
                map_syntect_color_to_theme(segment.color, palette),
            );
        }
    }

    let view_height = (content_rect.height() * view_fraction)
        .clamp(12.0_f32.min(content_rect.height()), content_rect.height());
    let view_top = content_rect.top()
        + (content_rect.height() - view_height) * scroll_fraction.clamp(0.0, 1.0);
    let view_rect = egui::Rect::from_min_size(
        egui::pos2(content_rect.left(), view_top),
        Vec2::new(content_rect.width(), view_height),
    );
    painter.rect_filled(
        view_rect,
        Rounding::ZERO,
        hex_to_c32_alpha(&palette.harmonies.base, 45),
    );
    painter.rect_stroke(
        view_rect,
        Rounding::ZERO,
        Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)),
    );

    response
        .interact_pointer_pos()
        .filter(|_| response.clicked() || response.dragged())
        .map(|pointer| {
            ((pointer.y - content_rect.top()) / content_rect.height().max(1.0)).clamp(0.0, 1.0)
        })
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
                let is_active = active_match
                    .map(|am| am.start_byte == m.start())
                    .unwrap_or(false);
                match_ranges.push((m.start(), m.end(), is_active));
            }
        }
    } else {
        let q = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        let target = if case_sensitive {
            text.to_string()
        } else {
            text.to_lowercase()
        };
        let mut offset = 0;
        while let Some(found) = target[offset..].find(&q) {
            let start = offset + found;
            let end = start + q.len();
            let is_active = active_match
                .map(|am| am.start_byte == start)
                .unwrap_or(false);
            match_ranges.push((start, end, is_active));
            offset = end;
            if offset >= target.len() {
                break;
            }
        }
    }

    match_ranges
}

/// Builds an egui LayoutJob with theme syntax highlighting and search match background highlights.
fn build_highlighted_layout_job(
    text: &str,
    syntax: &SyntaxReference,
    palette: &AppPalette,
    search_query: Option<&str>,
    is_regex: bool,
    case_sensitive: bool,
    active_match: Option<SearchMatch>,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let syn_theme = &THEME_SET.themes["base16-eighties.dark"];
    let mut highlighter = HighlightLines::new(syntax, syn_theme);

    let match_ranges =
        compute_highlight_matches(text, search_query, is_regex, case_sensitive, active_match);

    let mut byte_offset = 0;

    for line in text.split_inclusive('\n') {
        let ranges = highlighter
            .highlight_line(line, &SYNTAX_SET)
            .unwrap_or_default();

        for (style, token_str) in ranges {
            let token_start = byte_offset;
            let token_end = token_start + token_str.len();

            let fg = map_syntect_color_to_theme(style.foreground, palette);

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
                hex_to_c32(&palette.harmonies.complementary)
            } else if has_match {
                hex_to_c32_alpha(&palette.diff_mod, 100)
            } else {
                Color32::TRANSPARENT
            };

            let text_color = if is_active {
                hex_to_c32(&palette.theme.background)
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
    palette: &AppPalette,
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
            ui.label(
                egui::RichText::new(s)
                    .size(size)
                    .strong()
                    .italics()
                    .color(color),
            );
        }
        little_clip_render::markdown::MarkdownSpan::Code(s) => {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.theme.b_med))
                .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                .rounding(Rounding::same(3.0))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(s)
                            .font(FontId::monospace(size * 0.9))
                            .color(hex_to_c32(&palette.harmonies.triadic_2)),
                    );
                });
        }
        little_clip_render::markdown::MarkdownSpan::Link { label, url } => {
            ui.hyperlink_to(
                egui::RichText::new(label)
                    .size(size)
                    .color(hex_to_c32(&palette.harmonies.base))
                    .underline(),
                url,
            );
        }
    }
}

/// Renders a styled code block for markdown view.
fn render_styled_code_block(ui: &mut egui::Ui, code: &str, lang: &str, palette: &AppPalette) {
    let syntax = find_syntax(lang);

    egui::Frame::none()
        .fill(hex_to_c32(&palette.theme.b_low))
        .stroke(Stroke::new(1.0_f32, hex_to_c32(&palette.theme.b_med)))
        .inner_margin(egui::Margin::same(10.0))
        .rounding(Rounding::same(4.0))
        .show(ui, |ui| {
            if !lang.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lang.to_uppercase())
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.theme.f_low)),
                    );
                });
                ui.add_space(4.0);
            }

            let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let mut layout_job =
                    build_highlighted_layout_job(text, syntax, palette, None, false, false, None);
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
        let content =
            "fn main() {\n    let answer = 42;\n    println!(\"{}\", answer);\n}".to_string();
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
        let content = "# A python script\nimport math\n\ndef calculate(n):\n    return math.pi * n"
            .to_string();
        let state = TextPreviewState::from_text(content, None, &config);

        assert_eq!(state.language, "py");
        assert!(!state.is_markdown);
        assert_eq!(state.view_mode, TextViewMode::Raw);
        assert!(state.show_line_numbers);
    }

    #[test]
    fn test_minimap_cache_uses_syntax_colored_blocks() {
        let lines =
            build_minimap_lines("fn main() {\n    let answer = 42;\n}\n", find_syntax("rs"));

        assert_eq!(lines.len(), 4, "one minimap row per source line");
        assert!(lines.iter().take(3).all(|line| !line.segments.is_empty()));
        assert!(lines.iter().flat_map(|line| &line.segments).all(|segment| {
            segment.start_column < segment.end_column && segment.end_column <= MINIMAP_COLUMNS
        }));
    }

    #[test]
    fn test_gutter_text_alignment() {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    let gutter_resp =
                        ui.label(egui::RichText::new("1\n2\n3").font(FontId::monospace(12.5)));
                    let mut text = "#include <stdio.h>\nint main() {}\n".to_string();
                    let syntax = find_syntax("c");
                    let palette = AppPalette::default();
                    let mut layouter = |ui: &egui::Ui, text: &str, _wrap_width: f32| {
                        let layout_job = build_highlighted_layout_job(
                            text, syntax, &palette, None, false, false, None,
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
        assert_eq!(
            text_shapes.len(),
            2,
            "Expected gutter text and code text shapes"
        );
        let gutter_text = text_shapes[0];
        let code_text = text_shapes[1];

        assert_eq!(
            gutter_text.pos.y, code_text.pos.y,
            "Gutter and code must align on identical Y coordinate"
        );
        assert_eq!(
            gutter_text.galley.rows.len(),
            code_text.galley.rows.len(),
            "Same number of rows"
        );
        for (g_row, c_row) in gutter_text
            .galley
            .rows
            .iter()
            .zip(code_text.galley.rows.iter())
        {
            assert_eq!(g_row.rect.min.y, c_row.rect.min.y, "Row min Y must align");
            assert_eq!(
                g_row.rect.height(),
                c_row.rect.height(),
                "Row heights must be identical"
            );
        }
    }
}
