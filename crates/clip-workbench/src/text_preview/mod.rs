//! Rich interactive text and code preview workbench view.
//!
//! Provides syntax-highlighted code inspection, rendered Markdown viewing, text selection and copying,
//! toggleable line-wrapping, line numbers gutter, regex find toolbar, and external editor escape hatches.

mod markdown;
mod minimap;
mod raw_code;
mod search;
mod studio;

use minimap::*;
pub use raw_code::build_highlighted_layout_job;
use search::*;

use crate::icons::Icon;
use crate::shell::{EscapeHatch, EscapeHatchLauncher, StudioShell};
use crate::ui_ext::{BODY_FONT_SIZE, HEADING_FONT_SIZE, MONO_FONT_SIZE, SUBHEADING_FONT_SIZE};
use crate::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, FontId, Rounding, Stroke, Vec2};
use little_clip_core::config::AppConfig;
use little_clip_core::palette::AppPalette;
use little_clip_core::syntax::detect_language;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use regex::RegexBuilder;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, mpsc::Receiver};
use std::time::{Duration, Instant};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::{SyntaxReference, SyntaxSet};

/// Maps syntax highlighting hues into the fixed application palette harmonies.
fn map_syntect_color_to_theme(
    style_fg: syntect::highlighting::Color,
    palette: &AppPalette,
) -> Color32 {
    let surface = hex_to_c32(&palette.colors.surface_recessed);
    let readable =
        |preferred| crate::ui_ext::contrast_text_c32_on(surface, surface, Some(preferred), palette);
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
            return readable(hex_to_c32(&palette.colors.text_muted));
        } else {
            // Standard high contrast text
            return readable(hex_to_c32(&palette.colors.text_primary));
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

    // Map by hue angle to the shared color harmonies.
    if h < 45.0 {
        // Red / Orange (constants, literals)
        readable(hex_to_c32(&palette.diff_mod))
    } else if h < 90.0 {
        // Amber / Yellow (strings)
        readable(hex_to_c32(&palette.harmonies.triadic_2))
    } else if h < 165.0 {
        // Green (strings, regex)
        readable(hex_to_c32(&palette.diff_add))
    } else if h < 210.0 {
        // Cyan (types, structures)
        readable(hex_to_c32(&palette.harmonies.split_comp_1))
    } else if h < 275.0 {
        // Blue (functions, identifiers)
        readable(hex_to_c32(&palette.harmonies.triadic_1))
    } else {
        // Purple / Magenta / Pink (keywords, control flow)
        readable(hex_to_c32(&palette.harmonies.base))
    }
}

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

const MINIMAP_COLUMNS: usize = 96;
const MINIMAP_WIDTH: f32 = 112.0;
/// Maximum source bytes laid out in one editor fragment. Oversized logical
/// lines are presented as continuation rows so no single egui layout job can
/// shape megabytes of text synchronously.
const MAX_CODE_FRAGMENT_BYTES: usize = 4 * 1024;
const MINIMAP_RETAINED_LINES: usize = 4096;
const MINIMAP_CANCELLATION_CHECK_BYTES: usize = 64 * 1024;
const MINIMAP_SCRATCH_REUSE_LIMIT: usize = 64 * 1024;
const MINIMAP_MAX_SYNTAX_LINE_BYTES: usize = 1024 * 1024;
const MINIMAP_NEUTRAL_COLOR: syntect::highlighting::Color = syntect::highlighting::Color {
    r: 128,
    g: 128,
    b: 128,
    a: 255,
};
const CODE_ROW_HEIGHT_BLOCK_SIZE: usize = 32;
static NEXT_INLINE_TEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy)]
struct CodeFragment {
    start_byte: usize,
    end_byte: usize,
}

/// Immutable text and layout facts that can be reused by independent viewers.
pub struct TextPreviewData {
    content: Arc<String>,
    language: &'static str,
    is_markdown: bool,
    code_fragments: Arc<Vec<CodeFragment>>,
    line_first_fragment: Arc<Vec<usize>>,
}

/// Compact, virtualized text preview for embedding in another view.
///
/// Each instance has its own scroll identity while sharing immutable source
/// data with sibling previews.
pub struct InlineTextPreview {
    data: Arc<TextPreviewData>,
    host_size: Vec2,
    id: egui::Id,
    scroll_offset: f32,
}

impl InlineTextPreview {
    /// Creates a compact preview with a fixed finite host size.
    pub fn new(data: Arc<TextPreviewData>, host_size: Vec2) -> Self {
        assert!(
            host_size.x.is_finite()
                && host_size.y.is_finite()
                && host_size.x > 0.0
                && host_size.y > 0.0,
            "inline text preview requires a positive finite host size"
        );
        Self {
            data,
            host_size,
            id: egui::Id::new((
                "inline-text-preview",
                NEXT_INLINE_TEXT_ID.fetch_add(1, Ordering::Relaxed),
            )),
            scroll_offset: 0.0,
        }
    }

    /// Renders only visible bounded source fragments inside the allocated host.
    pub fn show(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(self.host_size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return response;
        }

        let syntax = find_syntax(self.data.language);
        let data = Arc::clone(&self.data);
        let id = self.id;
        let scroll_offset = self.scroll_offset;
        let output = ui.allocate_ui_at_rect(rect, |ui| {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.surface_recessed))
                .stroke(Stroke::new(
                    1.0_f32,
                    hex_to_c32(&palette.colors.surface_panel),
                ))
                .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
                .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_SM))
                .show(ui, |ui| {
                    let fragments = &data.code_fragments;
                    let row_height =
                        ui.fonts(|fonts| fonts.row_height(&FontId::monospace(MONO_FONT_SIZE)));
                    let row_pitch = row_height + ui.spacing().item_spacing.y;
                    // Fragments are single source lines. Keep a fixed row height and clip long
                    // lines to the host width instead of wrapping them.
                    let output = egui::ScrollArea::vertical()
                        .id_source(id)
                        .auto_shrink([false, false])
                        .vertical_scroll_offset(scroll_offset)
                        .show_rows(ui, row_height, fragments.len(), |ui, rows| {
                            let viewport_origin = ui.max_rect().min;
                            let row_width = ui.available_width();
                            let first_row = rows.start;
                            for row in rows {
                                let fragment = fragments[row];
                                let source = &data.content[fragment.start_byte..fragment.end_byte];
                                let mut job =
                                    build_highlighted_layout_job(source, syntax, palette, &[]);
                                job.wrap.max_width = f32::INFINITY;
                                let galley = ui.fonts(|fonts| fonts.layout_job(job));
                                let row_rect = egui::Rect::from_min_size(
                                    egui::pos2(
                                        viewport_origin.x,
                                        viewport_origin.y + (row - first_row) as f32 * row_pitch,
                                    ),
                                    Vec2::new(row_width, row_height),
                                );
                                ui.painter().with_clip_rect(row_rect).galley(
                                    row_rect.min,
                                    galley,
                                    hex_to_c32(&palette.colors.text_primary),
                                );
                            }
                        });
                    output.state.offset.y
                })
                .inner
        });
        self.scroll_offset = output.inner;
        response
    }
}

impl TextPreviewData {
    /// Prepares shared source data from in-memory text using the preview's normal format detection.
    pub fn from_text(content: Arc<String>) -> Arc<Self> {
        let content = if content.starts_with('\u{feff}') {
            Arc::new(content[char::len_utf8('\u{feff}')..].to_owned())
        } else {
            content
        };
        let line_starts = line_start_offsets(&content);
        let (code_fragments, line_first_fragment) = code_fragments(&content, &line_starts);
        let is_markdown = detect_markdown_text(&content);
        let language = if is_markdown {
            "md"
        } else {
            detect_language(&content, None)
        };

        Arc::new(Self {
            content,
            language,
            is_markdown,
            code_fragments: Arc::new(code_fragments),
            line_first_fragment: Arc::new(line_first_fragment),
        })
    }

    fn from_file(content: String, extension: Option<&str>) -> Arc<Self> {
        let content = Arc::new(content);
        let line_starts = line_start_offsets(&content);
        let (code_fragments, line_first_fragment) = code_fragments(&content, &line_starts);
        let language = detect_language(&content, extension);
        let is_markdown = extension
            .map(|extension| {
                extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
            })
            .unwrap_or(false);

        Arc::new(Self {
            content,
            language,
            is_markdown,
            code_fragments: Arc::new(code_fragments),
            line_first_fragment: Arc::new(line_first_fragment),
        })
    }
}

fn code_fragments(text: &str, line_starts: &[usize]) -> (Vec<CodeFragment>, Vec<usize>) {
    let mut fragments = Vec::new();
    let mut line_first_fragment = Vec::with_capacity(line_starts.len());
    for (source_line, &start) in line_starts.iter().enumerate() {
        line_first_fragment.push(fragments.len());
        let end = line_starts
            .get(source_line + 1)
            .copied()
            .unwrap_or(text.len())
            .min(text.len());
        let end = if end > start && text.as_bytes()[end - 1] == b'\n' {
            end - 1
        } else {
            end
        };
        if end <= start {
            fragments.push(CodeFragment {
                start_byte: start,
                end_byte: end,
            });
            continue;
        }

        let mut fragment_start = start;
        while fragment_start < end {
            let mut fragment_end = (fragment_start + MAX_CODE_FRAGMENT_BYTES).min(end);
            while !text.is_char_boundary(fragment_end) {
                fragment_end -= 1;
            }
            // UTF-8 characters are at most four bytes, so this only applies
            // when a boundary falls inside one character.
            if fragment_end == fragment_start {
                fragment_end = text[fragment_start..end]
                    .char_indices()
                    .nth(1)
                    .map(|(offset, _)| fragment_start + offset)
                    .unwrap_or(end);
            }
            fragments.push(CodeFragment {
                start_byte: fragment_start,
                end_byte: fragment_end,
            });
            fragment_start = fragment_end;
        }
    }
    (fragments, line_first_fragment)
}

/// Cumulative row heights, updated through a block Fenwick tree as wrapping is
/// measured. Unmeasured rows start at one font row; only heights are retained.
#[derive(Default)]
struct CodeRowHeights {
    key: Option<(f32, f32, f32)>,
    heights: Vec<f32>,
    /// Fenwick sums over fixed-size blocks, not individual rows.
    sums: Vec<f64>,
    next_measurement: usize,
}

impl CodeRowHeights {
    fn reset(&mut self, count: usize, width: f32, height: f32, pixels_per_point: f32) {
        let key = (width, height, pixels_per_point);
        if self.key == Some(key) && self.heights.len() == count {
            return;
        }
        self.key = Some(key);
        self.heights = vec![height; count];
        let block_count = count.div_ceil(CODE_ROW_HEIGHT_BLOCK_SIZE);
        self.sums = vec![0.0; block_count + 1];
        for block in 0..block_count {
            let start = block * CODE_ROW_HEIGHT_BLOCK_SIZE;
            let end = (start + CODE_ROW_HEIGHT_BLOCK_SIZE).min(count);
            self.sums[block + 1] = (end - start) as f64 * height as f64;
        }
        for index in 1..=block_count {
            let parent = index + (index & index.wrapping_neg());
            if parent <= block_count {
                self.sums[parent] += self.sums[index];
            }
        }
        self.next_measurement = 0;
    }

    fn set_height(&mut self, row: usize, height: f32) {
        let delta = height as f64 - self.heights[row] as f64;
        self.heights[row] = height;
        let mut index = row / CODE_ROW_HEIGHT_BLOCK_SIZE + 1;
        while index < self.sums.len() {
            self.sums[index] += delta;
            index += index & index.wrapping_neg();
        }
    }

    fn offset(&self, row: usize) -> f32 {
        let row = row.min(self.heights.len());
        let mut block = row / CODE_ROW_HEIGHT_BLOCK_SIZE;
        let mut sum = 0.0_f64;
        while block > 0 {
            sum += self.sums[block];
            block &= block - 1;
        }
        let block_start = (row / CODE_ROW_HEIGHT_BLOCK_SIZE) * CODE_ROW_HEIGHT_BLOCK_SIZE;
        for height in &self.heights[block_start..row] {
            sum += *height as f64;
        }
        sum as f32
    }

    fn total_height(&self) -> f32 {
        self.offset(self.heights.len())
    }

    fn row_at(&self, offset: f32) -> usize {
        let block_count = self.heights.len().div_ceil(CODE_ROW_HEIGHT_BLOCK_SIZE);
        if block_count == 0 {
            return 0;
        }
        let mut block = 0;
        let mut sum = 0.0;
        let mut step = block_count.checked_next_power_of_two().unwrap_or(0);
        while step > 0 {
            let next = block + step;
            if next <= block_count && sum + self.sums[next] <= offset as f64 {
                block = next;
                sum += self.sums[next];
            }
            step >>= 1;
        }
        let start = (block * CODE_ROW_HEIGHT_BLOCK_SIZE).min(self.heights.len());
        let end = (start + CODE_ROW_HEIGHT_BLOCK_SIZE).min(self.heights.len());
        for row in start..end {
            if sum + self.heights[row] as f64 > offset as f64 {
                return row;
            }
            sum += self.heights[row] as f64;
        }
        self.heights.len().saturating_sub(1)
    }
}

/// A syntax-colored, character-free slice of one source line for the minimap.
#[derive(Debug, Clone, PartialEq)]
struct MinimapSegment {
    start_column: usize,
    end_column: usize,
    color: syntect::highlighting::Color,
}

#[derive(Debug, Clone, Default)]
struct MinimapLine {
    segments: Vec<MinimapSegment>,
}

enum MinimapResult {
    Lines(Vec<(usize, MinimapLine)>),
    Cancelled,
}

#[derive(Default)]
struct MinimapCache {
    lines: Vec<(usize, MinimapLine)>,
    job: Option<(JobTicket, Receiver<Result<MinimapResult, JobError>>)>,
    cancelled: Option<Arc<AtomicBool>>,
    started: bool,
    ready: bool,
    failed: bool,
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
    pub content: Arc<String>,
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
    matches: Option<SearchMatchSpool>,
    match_count: u64,
    pub active_match_idx: u64,
    pub regex_error: Option<String>,
    search_error: Option<String>,
    search_revision: u64,
    search_pending: bool,
    search_debounce_until: Option<Instant>,
    search_retry_at: Option<Instant>,
    search_job: Option<SearchMatchJob>,
    pub focus_find_input: bool,

    // Status / Toast notification
    pub status_toast: crate::ui_ext::StatusToast,

    // Scroll state
    pub scroll_to_line: Option<usize>,
    /// Exact pixel offset requested by minimap navigation.
    scroll_to_offset: Option<f32>,
    code_fragments: Arc<Vec<CodeFragment>>,
    line_first_fragment: Arc<Vec<usize>>,
    code_rows: CodeRowHeights,
    code_scroll_offset: f32,
    code_selection: Option<(usize, usize)>,
    code_selection_dragging: bool,
    pub minimap_fraction: f32,
    minimap_view_fraction: f32,
    minimap_content_height: f32,
    minimap_viewport_height: f32,
    minimap_lines: MinimapCache,
    minimap_overflows: bool,
}

fn line_start_offsets(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

pub use little_clip_core::syntax::detect_markdown_text;

#[cfg(test)]
pub(super) fn update_search_matches_and_wait(state: &mut TextPreviewState) {
    state.update_search_matches();
    state.search_debounce_until = Some(Instant::now());
    let context = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while state.search_pending && Instant::now() < deadline {
        state.service_search_jobs(&context, BackgroundJobs::global(), Instant::now());
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        !state.search_pending,
        "search job did not finish before timeout"
    );
}

impl TextPreviewState {
    /// Creates a new text preview from in-memory text.
    pub fn from_text(text: String, title: Option<String>, config: &AppConfig) -> Self {
        Self::from_data(TextPreviewData::from_text(Arc::new(text)), title, config)
    }

    /// Creates an independent view state from reusable immutable text data.
    pub fn from_data(
        data: Arc<TextPreviewData>,
        title: Option<String>,
        config: &AppConfig,
    ) -> Self {
        let title = title.unwrap_or_else(|| {
            if data.is_markdown {
                "Markdown Preview".to_string()
            } else if data.language != "txt" {
                format!("{} Code Preview", data.language.to_uppercase())
            } else {
                "Text Preview".to_string()
            }
        });
        let show_line_numbers = data.language != "txt" && !data.is_markdown;
        let view_mode = if data.is_markdown {
            TextViewMode::RenderedMarkdown
        } else {
            TextViewMode::Raw
        };

        Self::from_data_with_path(data, title, None, config, show_line_numbers, view_mode)
    }

    fn from_data_with_path(
        data: Arc<TextPreviewData>,
        title: String,
        path: Option<PathBuf>,
        config: &AppConfig,
        show_line_numbers: bool,
        view_mode: TextViewMode,
    ) -> Self {
        Self {
            title,
            content: Arc::clone(&data.content),
            path,
            language: data.language,
            is_markdown: data.is_markdown,
            view_mode,
            wrap_lines: true,
            show_line_numbers,
            editor_cmd: config.text.resolve_editor(),
            show_find: false,
            find_query: String::new(),
            is_regex: false,
            case_sensitive: false,
            matches: None,
            match_count: 0,
            active_match_idx: 0,
            regex_error: None,
            search_error: None,
            search_revision: 0,
            search_pending: false,
            search_debounce_until: None,
            search_retry_at: None,
            search_job: None,
            focus_find_input: false,
            status_toast: crate::ui_ext::StatusToast::new(),
            scroll_to_line: None,
            scroll_to_offset: None,
            code_fragments: Arc::clone(&data.code_fragments),
            line_first_fragment: Arc::clone(&data.line_first_fragment),
            code_rows: CodeRowHeights::default(),
            code_scroll_offset: 0.0,
            code_selection: None,
            code_selection_dragging: false,
            minimap_fraction: 0.0,
            minimap_view_fraction: 1.0,
            minimap_content_height: 1.0,
            minimap_viewport_height: 1.0,
            minimap_lines: MinimapCache::default(),
            minimap_overflows: false,
        }
    }

    /// Creates a new text preview from a file on disk.
    pub fn from_file<P: AsRef<Path>>(path: P, config: &AppConfig) -> Option<Self> {
        let path_ref = path.as_ref();
        let content = little_clip_ops::read_text_file_preview(path_ref, 2 * 1024 * 1024)?;
        let ext = path_ref.extension().and_then(|e| e.to_str());
        let data = TextPreviewData::from_file(content, ext);
        let title = path_ref
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Text file".to_string());
        let show_line_numbers = data.language != "txt" && !data.is_markdown;
        let view_mode = if data.is_markdown {
            TextViewMode::RenderedMarkdown
        } else {
            TextViewMode::Raw
        };
        Some(Self::from_data_with_path(
            data,
            title,
            Some(path_ref.to_path_buf()),
            config,
            show_line_numbers,
            view_mode,
        ))
    }

    fn display_row_for_byte(&self, byte: usize) -> usize {
        self.code_fragments
            .partition_point(|fragment| fragment.start_byte <= byte)
            .saturating_sub(1)
            .min(self.code_fragments.len().saturating_sub(1))
    }
}

/// Helper to find syntax definition for language string.
pub fn find_syntax(lang: &str) -> &'static SyntaxReference {
    let ps = &*SYNTAX_SET;
    ps.find_syntax_by_token(lang)
        .or_else(|| ps.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ps.find_syntax_plain_text())
}
#[cfg(test)]
mod tests;
