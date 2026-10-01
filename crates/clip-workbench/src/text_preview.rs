//! Rich interactive text and code preview workbench view.
//!
//! Provides syntax-highlighted code inspection, rendered Markdown viewing, text selection and copying,
//! toggleable line-wrapping, line numbers gutter, regex find toolbar, and external editor escape hatches.

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

impl MinimapCache {
    fn start(&mut self, text: Arc<String>, syntax_name: String, jobs: &BackgroundJobs) {
        if self.started {
            return;
        }
        self.started = true;
        let weak_text = Arc::downgrade(&text);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(std::time::Duration::MAX, move || {
            build_minimap_lines(&weak_text, find_syntax(&syntax_name), &worker_cancelled)
        });
        self.job = Some((ticket, receiver));
        self.cancelled = Some(cancelled);
    }

    fn poll(&mut self) -> bool {
        let Some((_, receiver)) = &self.job else {
            return false;
        };
        match receiver.try_recv() {
            Ok(Ok(MinimapResult::Lines(lines))) => {
                self.lines = lines;
                self.job = None;
                self.cancelled = None;
                self.ready = true;
                true
            }
            Ok(Err(JobError::QueueFull)) => {
                self.job = None;
                self.cancelled = None;
                self.started = false;
                false
            }
            Ok(Ok(MinimapResult::Cancelled))
            | Ok(Err(_))
            | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.cancelled = None;
                self.failed = true;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    fn cancel(&self) {
        if let Some(cancelled) = &self.cancelled {
            cancelled.store(true, Ordering::Release);
        }
        if let Some((ticket, _)) = &self.job {
            ticket.cancel();
        }
    }
}

impl Drop for MinimapCache {
    fn drop(&mut self) {
        // Queued jobs hold only a Weak source reference. Running work checks
        // cancellation between lines and owns only its current logical-line
        // scratch buffer while syntect processes that line.
        self.cancel();
    }
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

const SEARCH_MATCH_RECORD_BYTES: usize = 16;
const SEARCH_MATCH_MEMORY_LIMIT_BYTES: usize = 64 * 1024;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(80);
const SEARCH_RETRY_DELAY: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchMatchRange {
    start_byte: u64,
    end_byte: u64,
}

/// Stores fixed-size match ranges in memory until the bounded threshold, then spills to cache.
struct SearchMatchSpool {
    file: tempfile::SpooledTempFile,
    record_count: u64,
    cursor_at_end: bool,
    last_range: Option<SearchMatchRange>,
}

struct SearchMatchJob {
    revision: u64,
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<Result<SearchBuildOutput, String>, JobError>>,
}

impl Drop for SearchMatchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

enum SearchBuildOutput {
    Matches(SearchMatchSpool),
    InvalidRegex(String),
    Cancelled,
}

impl SearchMatchJob {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

fn build_search_matches(
    content: Arc<String>,
    query: String,
    is_regex: bool,
    case_sensitive: bool,
    cancelled: &AtomicBool,
) -> Result<SearchBuildOutput, String> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(SearchBuildOutput::Cancelled);
    }
    let pattern = if is_regex {
        query
    } else {
        regex::escape(&query)
    };
    let regex = match RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => return Ok(SearchBuildOutput::InvalidRegex(error.to_string())),
    };
    let mut spool = SearchMatchSpool::new().map_err(|error| error.to_string())?;
    for (index, matched) in regex.find_iter(content.as_str()).enumerate() {
        if index % 256 == 0 && cancelled.load(Ordering::Acquire) {
            return Ok(SearchBuildOutput::Cancelled);
        }
        spool
            .append(matched.start() as u64, matched.end() as u64)
            .map_err(|error| error.to_string())?;
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(SearchBuildOutput::Cancelled);
    }
    Ok(SearchBuildOutput::Matches(spool))
}

impl SearchMatchSpool {
    fn new() -> io::Result<Self> {
        let directory =
            little_clip_core::staging::get_staging_dir().map_err(|error| match error {
                little_clip_core::staging::StagingError::Io(error) => error,
            })?;
        Self::new_in(&directory)
    }

    fn new_in(directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        Ok(Self {
            file: tempfile::spooled_tempfile_in(SEARCH_MATCH_MEMORY_LIMIT_BYTES, directory),
            record_count: 0,
            cursor_at_end: true,
            last_range: None,
        })
    }

    fn len(&self) -> u64 {
        self.record_count
    }

    fn append(&mut self, start_byte: u64, end_byte: u64) -> io::Result<()> {
        if start_byte > end_byte
            || self
                .last_range
                .is_some_and(|last| start_byte < last.start_byte || end_byte < last.end_byte)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "match ranges must be ordered and non-reversed",
            ));
        }
        let record_count = self
            .record_count
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "match count overflow"))?;
        let byte_len = record_count
            .checked_mul(SEARCH_MATCH_RECORD_BYTES as u64)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "match spool size overflow")
            })?;
        let record = encode_search_match_range(start_byte, end_byte);

        if byte_len > SEARCH_MATCH_MEMORY_LIMIT_BYTES as u64 && !self.file.is_rolled() {
            self.file.roll()?;
        }
        if !self.cursor_at_end {
            let offset = self.record_count * SEARCH_MATCH_RECORD_BYTES as u64;
            self.file.seek(SeekFrom::Start(offset))?;
        }
        self.cursor_at_end = false;
        self.file.write_all(&record)?;
        self.cursor_at_end = true;
        self.record_count = record_count;
        self.last_range = Some(SearchMatchRange {
            start_byte,
            end_byte,
        });
        Ok(())
    }

    fn get(&mut self, index: u64) -> io::Result<SearchMatchRange> {
        if index >= self.record_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "match index is out of bounds",
            ));
        }
        let offset = index * SEARCH_MATCH_RECORD_BYTES as u64;
        let mut record = [0; SEARCH_MATCH_RECORD_BYTES];
        self.cursor_at_end = false;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut record)?;
        self.cursor_at_end = index + 1 == self.record_count;
        Ok(decode_search_match_range(record))
    }

    /// Finds matches intersecting one half-open source fragment using constant iterator storage.
    fn ranges_for_fragment(
        &mut self,
        fragment_start: u64,
        fragment_end: u64,
    ) -> io::Result<SearchMatchRangeIter<'_>> {
        let mut low = 0;
        let mut high = self.record_count;
        if fragment_start >= fragment_end {
            low = high;
        } else {
            while low < high {
                let middle = low + (high - low) / 2;
                if self.get(middle)?.end_byte <= fragment_start {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
        }
        Ok(SearchMatchRangeIter {
            spool: self,
            next_index: low,
            fragment_start,
            fragment_end,
            finished: false,
            needs_seek: true,
        })
    }

    #[cfg(test)]
    fn is_rolled(&self) -> bool {
        self.file.is_rolled()
    }
}

/// Streams intersecting ranges without allocating a fragment-sized match vector.
struct SearchMatchRangeIter<'a> {
    spool: &'a mut SearchMatchSpool,
    next_index: u64,
    fragment_start: u64,
    fragment_end: u64,
    finished: bool,
    needs_seek: bool,
}

impl Iterator for SearchMatchRangeIter<'_> {
    type Item = io::Result<(u64, SearchMatchRange)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        while self.next_index < self.spool.len() {
            let index = self.next_index;
            self.next_index += 1;
            if self.needs_seek {
                let offset = index * SEARCH_MATCH_RECORD_BYTES as u64;
                self.spool.cursor_at_end = false;
                if let Err(error) = self.spool.file.seek(SeekFrom::Start(offset)) {
                    self.finished = true;
                    return Some(Err(error));
                }
                self.needs_seek = false;
            }
            let mut record = [0; SEARCH_MATCH_RECORD_BYTES];
            self.spool.cursor_at_end = false;
            let range = match self.spool.file.read_exact(&mut record) {
                Ok(()) => {
                    let range = decode_search_match_range(record);
                    self.spool.cursor_at_end = index + 1 == self.spool.len();
                    range
                }
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            };
            if range.start_byte >= self.fragment_end {
                self.finished = true;
                return None;
            }
            // Existing projection hides matches on either edge but keeps interior zero-width hits.
            if range.end_byte > self.fragment_start && range.start_byte < self.fragment_end {
                return Some(Ok((index, range)));
            }
        }
        self.finished = true;
        None
    }
}

fn encode_search_match_range(start_byte: u64, end_byte: u64) -> [u8; SEARCH_MATCH_RECORD_BYTES] {
    let mut record = [0; SEARCH_MATCH_RECORD_BYTES];
    record[..8].copy_from_slice(&start_byte.to_le_bytes());
    record[8..].copy_from_slice(&end_byte.to_le_bytes());
    record
}

fn decode_search_match_range(record: [u8; SEARCH_MATCH_RECORD_BYTES]) -> SearchMatchRange {
    let mut start = [0; 8];
    let mut end = [0; 8];
    start.copy_from_slice(&record[..8]);
    end.copy_from_slice(&record[8..]);
    SearchMatchRange {
        start_byte: u64::from_le_bytes(start),
        end_byte: u64::from_le_bytes(end),
    }
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

    /// Recalculates search matches based on query, regex, and case sensitivity.
    pub fn update_search_matches(&mut self) {
        if let Some(job) = self.search_job.take() {
            job.cancel();
        }
        self.search_revision = self.search_revision.wrapping_add(1);
        self.matches = None;
        self.match_count = 0;
        self.regex_error = None;
        self.search_error = None;
        self.search_retry_at = None;
        if self.find_query.is_empty() {
            self.search_pending = false;
            self.search_debounce_until = None;
            self.active_match_idx = 0;
            return;
        }
        self.search_pending = true;
        self.search_debounce_until = Some(Instant::now() + SEARCH_DEBOUNCE);
    }

    fn service_search_jobs(&mut self, ctx: &egui::Context, jobs: &BackgroundJobs, now: Instant) {
        self.poll_search_job(now);
        if !self.search_pending {
            return;
        }
        if self.search_job.is_some() {
            ctx.request_repaint_after(SEARCH_RETRY_DELAY);
            return;
        }
        if let Some(until) = self
            .search_debounce_until
            .take()
            .filter(|until| *until > now)
        {
            ctx.request_repaint_after(until.saturating_duration_since(now));
            self.search_debounce_until = Some(until);
            return;
        }
        if let Some(until) = self.search_retry_at.take().filter(|until| *until > now) {
            ctx.request_repaint_after(until.saturating_duration_since(now));
            self.search_retry_at = Some(until);
            return;
        }

        let revision = self.search_revision;
        let content = Arc::clone(&self.content);
        let query = self.find_query.clone();
        let is_regex = self.is_regex;
        let case_sensitive = self.case_sensitive;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            build_search_matches(content, query, is_regex, case_sensitive, &worker_cancelled)
        });
        self.search_job = Some(SearchMatchJob {
            revision,
            ticket,
            cancelled,
            receiver,
        });
        ctx.request_repaint_after(SEARCH_RETRY_DELAY);
    }

    fn poll_search_job(&mut self, now: Instant) {
        let result = self.search_job.as_ref().map(|job| job.receiver.try_recv());
        let result = match result {
            Some(Ok(result)) => result,
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                let job = self.search_job.take().expect("polled search job");
                if job.revision == self.search_revision {
                    self.search_pending = false;
                    self.fail_search("search worker stopped before returning results".into());
                }
                return;
            }
        };
        let job = self.search_job.take().expect("completed search job");
        if job.revision != self.search_revision {
            return;
        }
        match result {
            Ok(Ok(SearchBuildOutput::Matches(spool))) => {
                self.match_count = spool.len();
                self.matches = Some(spool);
                self.active_match_idx = if self.active_match_idx >= self.match_count {
                    0
                } else {
                    self.active_match_idx
                };
                self.search_pending = false;
                self.search_retry_at = None;
            }
            Ok(Ok(SearchBuildOutput::InvalidRegex(error))) => {
                self.active_match_idx = 0;
                self.regex_error = Some(error);
                self.search_pending = false;
            }
            Ok(Ok(SearchBuildOutput::Cancelled)) | Err(JobError::QueueFull) => {
                self.search_pending = true;
                self.search_retry_at = Some(now + SEARCH_RETRY_DELAY);
            }
            Ok(Err(error)) => {
                self.fail_search(error);
                self.search_pending = false;
            }
            Err(JobError::Cancelled) => {
                self.search_pending = false;
            }
            Err(error) => {
                self.fail_search(error.to_string());
                self.search_pending = false;
            }
        }
    }

    fn search_match_at(&mut self, index: u64) -> Option<SearchMatch> {
        if index >= self.match_count {
            return None;
        }
        let range = match self.matches.as_mut()?.get(index) {
            Ok(range) => range,
            Err(error) => {
                self.fail_search(format!("could not read search results: {error}"));
                return None;
            }
        };
        let (start_byte, end_byte) = match (
            usize::try_from(range.start_byte),
            usize::try_from(range.end_byte),
        ) {
            (Ok(start), Ok(end)) => (start, end),
            _ => {
                self.fail_search("search result exceeds platform address size".into());
                return None;
            }
        };
        // Each source line's first display fragment retains its exact start
        // byte, so use that index instead of keeping a duplicate line-start Vec.
        let line = self.line_first_fragment.partition_point(|first_fragment| {
            self.code_fragments[*first_fragment].start_byte <= start_byte
        });
        Some(SearchMatch {
            start_byte,
            end_byte,
            line,
        })
    }

    fn fail_search(&mut self, error: String) {
        self.matches = None;
        self.match_count = 0;
        self.active_match_idx = 0;
        self.search_error = Some(error);
    }

    #[cfg(test)]
    pub fn match_count(&self) -> u64 {
        self.match_count
    }

    #[cfg(test)]
    pub fn search_match(&mut self, index: u64) -> Option<SearchMatch> {
        self.search_match_at(index)
    }

    /// Advances to the next search match.
    pub fn next_match(&mut self) {
        if self.match_count > 0 {
            self.active_match_idx = (self.active_match_idx + 1) % self.match_count;
            if let Some(matched) = self.search_match_at(self.active_match_idx) {
                self.scroll_to_line = Some(self.display_row_for_byte(matched.start_byte) + 1);
            }
        }
    }

    /// Moves to the previous search match.
    pub fn prev_match(&mut self) {
        if self.match_count > 0 {
            if self.active_match_idx == 0 {
                self.active_match_idx = self.match_count - 1;
            } else {
                self.active_match_idx -= 1;
            }
            if let Some(matched) = self.search_match_at(self.active_match_idx) {
                self.scroll_to_line = Some(self.display_row_for_byte(matched.start_byte) + 1);
            }
        }
    }

    fn display_row_for_byte(&self, byte: usize) -> usize {
        self.code_fragments
            .partition_point(|fragment| fragment.start_byte <= byte)
            .saturating_sub(1)
            .min(self.code_fragments.len().saturating_sub(1))
    }

    /// Triggers opening the text in the configured external editor (e.g. VS Code).
    pub fn trigger_escape_hatch(&mut self) {
        self.trigger_escape_hatch_with(|editor, command, target| {
            EscapeHatchLauncher::spawn_with_fallback(editor, command, target)
        });
    }

    fn trigger_escape_hatch_with(
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

        match launch(editor, editor, &target_path) {
            Ok(message) | Err(message) => self.status_toast.set(message, now),
        }
    }

    /// Copies the full text content to the system clipboard.
    pub fn copy_all_to_clipboard(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.content.as_str().to_owned());
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
        self.service_search_jobs(ctx, BackgroundJobs::global(), Instant::now());

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
                        TextViewMode::Raw => {
                            self.render_raw_code_view(ui, palette);
                        }
                    },
                );

                ui.add_space(crate::ui_ext::SPACE_XS);

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

        if !ctx.wants_keyboard_input()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F))
        {
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
            hex_to_c32(&palette.colors.surface_panel),
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
        let surface = hex_to_c32(&palette.colors.surface_recessed);
        if self.find_query.is_empty() {
            return;
        }
        if let Some(ref err) = self.regex_error {
            ui.horizontal_wrapped(|ui| {
                let error_color = crate::ui_ext::contrast_text_c32_on(
                    surface,
                    surface,
                    Some(hex_to_c32(&palette.diff_del)),
                    palette,
                );
                Icon::Alert.ui(ui, 13.0, error_color);
                ui.label(
                    egui::RichText::new(format!("Regex error: {}", err))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(error_color),
                );
            });
        } else if let Some(ref err) = self.search_error {
            ui.label(
                egui::RichText::new(format!("Search failed: {err}"))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.diff_del)),
            );
        } else if self.search_pending {
            ui.label(
                egui::RichText::new("Searching…")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_muted)),
            );
        } else if self.match_count == 0 {
            ui.label(
                egui::RichText::new("No matches")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_muted)),
            );
        } else {
            let count_text = format!("{}/{} matches", self.active_match_idx + 1, self.match_count);
            ui.label(
                egui::RichText::new(count_text)
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(crate::ui_ext::contrast_text_c32_on(
                        surface,
                        surface,
                        Some(hex_to_c32(&palette.harmonies.complementary)),
                        palette,
                    )),
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
        ui.add_space(crate::ui_ext::SPACE_SM);
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::BUTTON_PADDING_X,
                crate::ui_ext::BUTTON_PADDING_Y,
            ))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    Icon::Search.ui(ui, 15.0, hex_to_c32(&palette.colors.text_secondary));

                    let search_edit = egui::TextEdit::singleline(&mut self.find_query)
                        .hint_text("Find in text (Enter for next, Shift+Enter for prev)...");

                    let search_width = ui.available_width().min(280.0).max(120.0);
                    let response =
                        ui.add_sized([search_width, crate::ui_ext::CONTROL_HEIGHT], search_edit);
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
                        hex_to_c32(&palette.colors.surface_panel),
                        palette,
                    );
                    if ui.add(prev_btn).clicked() {
                        self.prev_match();
                    }

                    let next_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::ArrowDown),
                        "Next",
                        hex_to_c32(&palette.colors.surface_panel),
                        palette,
                    );
                    if ui.add(next_btn).clicked() {
                        self.next_match();
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);
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

    fn source_fragment(&self, row: usize) -> (CodeFragment, &str) {
        let fragment = self.code_fragments[row];
        (
            fragment,
            &self.content[fragment.start_byte..fragment.end_byte],
        )
    }

    /// Measure a small batch per frame. Long logical lines are split into
    /// bounded UTF-8 fragments before they reach egui's text shaper.
    fn measure_code_rows(&mut self, ui: &egui::Ui, width: f32, minimum_height: f32, font: &FontId) {
        let started = std::time::Instant::now();
        for _ in 0..64 {
            let row = self.code_rows.next_measurement;
            if row >= self.code_fragments.len() {
                break;
            }
            let (_, source) = self.source_fragment(row);
            let galley = ui.fonts(|fonts| {
                fonts.layout(source.to_owned(), font.clone(), Color32::WHITE, width)
            });
            self.code_rows
                .set_height(row, (galley.size().y + 4.0).max(minimum_height));
            self.code_rows.next_measurement += 1;
            if started.elapsed() >= std::time::Duration::from_millis(2) {
                break;
            }
        }
        if self.code_rows.next_measurement < self.code_fragments.len() {
            ui.ctx().request_repaint();
        }
    }

    /// Renders raw / code view with syntax highlighting, line numbers gutter, and search match highlighting.
    fn render_raw_code_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if !ui.ctx().wants_keyboard_input()
            && ui.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy))
                    || (input.key_pressed(egui::Key::C)
                        && (input.modifiers.command || input.modifiers.ctrl))
            })
        {
            let text = self
                .code_selection
                .filter(|(anchor, focus)| anchor != focus)
                .map(|(anchor, focus)| &self.content[anchor.min(focus)..anchor.max(focus)])
                .unwrap_or(&self.content);
            ui.ctx().copy_text(text.to_owned());
            // The platform output serves egui's clipboard integration. The
            // workbench also writes through its desktop clipboard adapter.
            #[cfg(not(test))]
            if let Err(error) = little_clip_core::copy_text_to_system_clipboard(text) {
                tracing::debug!(%error, "failed to copy raw text selection to system clipboard");
            }
        }
        let syntax = find_syntax(self.language);
        let code_font = FontId::monospace(crate::ui_ext::MONO_FONT_SIZE);
        let (line_height, digit_width) = ui.fonts(|fonts| {
            (
                fonts.row_height(&code_font) + 4.0,
                fonts.glyph_width(&code_font, '0'),
            )
        });

        let show_gutter = self.show_line_numbers;
        let mut scroll_area = egui::ScrollArea::new([!self.wrap_lines, true])
            .id_source("text_code_virtual_scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded);

        // Unified editor surface frame
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                let total_rows = self.code_fragments.len().max(1);
                let digits = self.line_first_fragment.len().to_string().len().max(2);
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
                            let gutter_width = if show_gutter {
                                digit_width * (digits + 1) as f32 + 12.0
                            } else {
                                0.0
                            };
                            let wrap_width = if self.wrap_lines {
                                (ui.available_width()
                                    - gutter_width
                                    - ui.spacing().scroll.allocated_width())
                                .max(digit_width)
                            } else {
                                f32::INFINITY
                            };
                            // egui rounds scroll offsets to physical pixels. A
                            // rounded boundary must still anchor the following
                            // source row when preceding estimates are updated.
                            let rounding_tolerance = 0.5 / ui.ctx().pixels_per_point();
                            let anchor_row = self
                                .code_rows
                                .row_at(self.code_scroll_offset + rounding_tolerance);
                            let anchor_inset = (self.code_scroll_offset
                                - self.code_rows.offset(anchor_row))
                            .max(0.0);
                            self.code_rows.reset(
                                total_rows,
                                wrap_width,
                                line_height,
                                ui.ctx().pixels_per_point(),
                            );
                            self.measure_code_rows(ui, wrap_width, line_height, &code_font);
                            let anchored_offset = self.code_rows.offset(anchor_row) + anchor_inset;
                            let requested_offset =
                                self.scroll_to_offset.take().unwrap_or_else(|| {
                                    self.scroll_to_line
                                        .take()
                                        .map(|line| self.code_rows.offset(line.saturating_sub(1)))
                                        .unwrap_or(anchored_offset)
                                });
                            scroll_area = scroll_area.vertical_scroll_offset(requested_offset);
                            let output = scroll_area.show_viewport(ui, |ui, viewport| {
                                let origin = ui.max_rect().min;
                                let mut row =
                                    self.code_rows.row_at(viewport.top() + rounding_tolerance);
                                // Keep row identities stable while the visible range changes.
                                ui.skip_ahead_auto_ids(row);
                                let pointer = ui.ctx().pointer_interact_pos();
                                let pressed = ui.input(|input| input.pointer.primary_pressed());
                                let down = ui.input(|input| input.pointer.primary_down());
                                let released = ui.input(|input| input.pointer.any_released());
                                let mut y = self.code_rows.offset(row);
                                while row < total_rows && y <= viewport.bottom() {
                                    let fragment = self.code_fragments[row];
                                    let source_line = self
                                        .line_first_fragment
                                        .partition_point(|first_fragment| *first_fragment <= row)
                                        .saturating_sub(1);
                                    let highlight_result = self.matches.as_mut().map(|spool| {
                                        line_highlight_ranges_from_spool(
                                            spool,
                                            self.active_match_idx,
                                            fragment.start_byte,
                                            fragment.end_byte,
                                        )
                                    });
                                    let (matches, search_read_error) = match highlight_result {
                                        Some(Ok(matches)) => (matches, None),
                                        Some(Err(error)) => (
                                            Vec::new(),
                                            Some(format!("could not read search results: {error}")),
                                        ),
                                        None => (Vec::new(), None),
                                    };
                                    let mut job = build_highlighted_layout_job(
                                        &self.content[fragment.start_byte..fragment.end_byte],
                                        syntax,
                                        palette,
                                        &matches,
                                    );
                                    if let Some(error) = search_read_error {
                                        self.fail_search(error);
                                    }
                                    job.wrap.max_width = wrap_width;
                                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                                    let line =
                                        &self.content[fragment.start_byte..fragment.end_byte];
                                    let height = (galley.size().y + 4.0).max(line_height);
                                    self.code_rows.set_height(row, height);
                                    let text_rect = egui::Rect::from_min_size(
                                        origin + egui::vec2(gutter_width, y + 2.0),
                                        galley.size(),
                                    );
                                    if show_gutter && self.line_first_fragment[source_line] == row {
                                        ui.painter().text(
                                            origin + egui::vec2(gutter_width - 10.0, y + 2.0),
                                            egui::Align2::RIGHT_TOP,
                                            format!("{}", source_line + 1),
                                            code_font.clone(),
                                            hex_to_c32(&palette.colors.text_muted),
                                        );
                                    }
                                    let response = ui
                                        .push_id(row, |ui| {
                                            ui.put(
                                                text_rect,
                                                egui::Label::new(galley.clone())
                                                    .selectable(false)
                                                    .sense(egui::Sense::click_and_drag()),
                                            )
                                        })
                                        .inner;
                                    if let Some(pointer) = pointer.filter(|point| {
                                        text_rect.contains(*point)
                                            && ui.clip_rect().contains(*point)
                                    }) {
                                        if (pressed && response.contains_pointer())
                                            || (self.code_selection_dragging && (down || released))
                                        {
                                            let cursor =
                                                galley.cursor_from_pos(pointer - text_rect.min);
                                            let local_byte = line
                                                .char_indices()
                                                .nth(cursor.ccursor.index)
                                                .map_or(line.len(), |(byte, _)| byte);
                                            let source_byte = fragment.start_byte + local_byte;
                                            if pressed {
                                                self.code_selection =
                                                    Some((source_byte, source_byte));
                                                self.code_selection_dragging = true;
                                            } else if let Some((_, focus)) =
                                                &mut self.code_selection
                                            {
                                                if *focus != source_byte {
                                                    *focus = source_byte;
                                                    ui.ctx().request_repaint();
                                                }
                                            }
                                        }
                                    }
                                    if let Some((anchor, focus)) = self.code_selection {
                                        paint_code_selection(
                                            ui,
                                            &galley,
                                            text_rect.min,
                                            line,
                                            fragment,
                                            anchor.min(focus),
                                            anchor.max(focus),
                                            hex_to_c32_alpha(&palette.harmonies.base, 90),
                                        );
                                    }
                                    y += height;
                                    row += 1;
                                }
                                if released {
                                    self.code_selection_dragging = false;
                                }
                                // `put` advances the layout cursor to the visible
                                // rows. Reserve the document from its origin,
                                // rather than adding its height after that cursor.
                                ui.expand_to_include_rect(egui::Rect::from_min_size(
                                    origin,
                                    egui::vec2(0.0, self.code_rows.total_height()),
                                ));
                            });
                            self.code_scroll_offset = output.state.offset.y;
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
                            if self.minimap_overflows {
                                self.minimap_lines.start(
                                    Arc::clone(&self.content),
                                    self.language.to_owned(),
                                    BackgroundJobs::global(),
                                );
                                if self.minimap_lines.poll() {
                                    ui.ctx().request_repaint();
                                } else if !self.minimap_lines.ready && !self.minimap_lines.failed {
                                    ui.ctx().request_repaint_after(
                                        std::time::Duration::from_millis(40),
                                    );
                                }
                            }
                        },
                    );
                    if show_minimap {
                        if let Some(fraction) = render_code_minimap(
                            ui,
                            &self.minimap_lines.lines,
                            &self.line_first_fragment,
                            &self.code_rows,
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
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.horizontal_wrapped(|ui| {
                let (size, color) = match level {
                    1 => (HEADING_FONT_SIZE, hex_to_c32(&palette.colors.text_primary)),
                    2 => (
                        SUBHEADING_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                    ),
                    _ => (BODY_FONT_SIZE, hex_to_c32(&palette.colors.text_secondary)),
                };
                for span in spans {
                    render_markdown_span(ui, &span, size, color, palette);
                }
            });
            ui.add_space(crate::ui_ext::SPACE_XS);
        }
        little_clip_render::markdown::MarkdownLine::Bullet { indent, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent as f32 * 16.0);
                ui.label(egui::RichText::new("•").size(BODY_FONT_SIZE).color(
                    crate::ui_ext::contrast_text_c32_on(
                        ui.visuals().panel_fill,
                        ui.visuals().panel_fill,
                        Some(hex_to_c32(&palette.harmonies.base)),
                        palette,
                    ),
                ));
                ui.add_space(crate::ui_ext::SPACE_XS);
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
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
                        .size(BODY_FONT_SIZE)
                        .strong()
                        .color(crate::ui_ext::contrast_text_c32_on(
                            ui.visuals().panel_fill,
                            ui.visuals().panel_fill,
                            Some(hex_to_c32(&palette.harmonies.base)),
                            palette,
                        )),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Quote { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.surface_recessed))
                .stroke(Stroke::new(3.0_f32, hex_to_c32(&palette.harmonies.base)))
                .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for span in spans {
                            render_markdown_span(
                                ui,
                                &span,
                                BODY_FONT_SIZE,
                                hex_to_c32(&palette.colors.text_secondary),
                                palette,
                            );
                        }
                    });
                });
        }
        little_clip_render::markdown::MarkdownLine::Rule => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        }
        little_clip_render::markdown::MarkdownLine::Paragraph { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Empty => {
            if *in_code_block {
                code_block_buf.push('\n');
            } else {
                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            }
        }
    }
}

/// Helper to find syntax definition for language string.
pub(crate) fn find_syntax(lang: &str) -> &'static SyntaxReference {
    let ps = &*SYNTAX_SET;
    ps.find_syntax_by_token(lang)
        .or_else(|| ps.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ps.find_syntax_plain_text())
}

/// Precomputes compact syntax spans once, avoiding text layout and per-frame parsing.
fn build_minimap_line(line: &str, highlighter: &mut HighlightLines<'static>) -> MinimapLine {
    let mut column = 0;
    let mut segments = Vec::new();
    for (style, token) in highlighter
        .highlight_line(line, &SYNTAX_SET)
        .unwrap_or_default()
    {
        let token_width = token.chars().count();
        let leading_space = token.chars().take_while(|ch| ch.is_whitespace()).count();
        let visible_start = column + leading_space;
        let visible_end = column + token.trim_end().chars().count();
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
}

fn build_neutral_minimap_line(line: &str) -> MinimapLine {
    let mut first = None;
    let mut end = 0;
    for (column, character) in line.chars().take(MINIMAP_COLUMNS).enumerate() {
        if !character.is_whitespace() {
            first.get_or_insert(column);
            end = column + 1;
        }
    }
    MinimapLine {
        segments: first
            .map(|start_column| {
                vec![MinimapSegment {
                    start_column,
                    end_column: end,
                    color: MINIMAP_NEUTRAL_COLOR,
                }]
            })
            .unwrap_or_default(),
    }
}

fn minimap_source_shape(
    source: &str,
    mut is_cancelled: impl FnMut() -> bool,
) -> Option<(usize, bool)> {
    let mut line_count = 1;
    let mut line_start = 0;
    let mut neutral_fallback = false;
    let mut bytes_until_cancel_check = 0;
    for (index, byte) in source.as_bytes().iter().enumerate() {
        if bytes_until_cancel_check == 0 && is_cancelled() {
            return None;
        }
        bytes_until_cancel_check += 1;
        if bytes_until_cancel_check == MINIMAP_CANCELLATION_CHECK_BYTES {
            bytes_until_cancel_check = 0;
        }
        if *byte == b'\n' {
            line_count += 1;
            neutral_fallback |= index - line_start > MINIMAP_MAX_SYNTAX_LINE_BYTES;
            line_start = index + 1;
        }
    }
    neutral_fallback |= source.len() - line_start > MINIMAP_MAX_SYNTAX_LINE_BYTES;
    (!is_cancelled()).then_some((line_count, neutral_fallback))
}

fn build_minimap_lines(
    text: &std::sync::Weak<String>,
    syntax: &'static SyntaxReference,
    cancelled: &AtomicBool,
) -> MinimapResult {
    let Some(source) = text.upgrade() else {
        return MinimapResult::Cancelled;
    };
    let Some((line_count, neutral_fallback)) =
        minimap_source_shape(&source, || cancelled.load(Ordering::Acquire))
    else {
        return MinimapResult::Cancelled;
    };
    drop(source);
    if cancelled.load(Ordering::Acquire) {
        return MinimapResult::Cancelled;
    }
    let stride = line_count.div_ceil(MINIMAP_RETAINED_LINES).max(1);
    let mut highlighter = HighlightLines::new(syntax, &THEME_SET.themes["base16-eighties.dark"]);
    let mut lines = Vec::with_capacity(line_count.div_ceil(stride));
    let mut line_buffer = String::new();
    let mut cursor = 0;
    for source_line in 0..line_count {
        if cancelled.load(Ordering::Acquire) {
            return MinimapResult::Cancelled;
        }
        let Some(source) = text.upgrade() else {
            return MinimapResult::Cancelled;
        };
        let mut line_end = cursor;
        while line_end < source.len() {
            if cancelled.load(Ordering::Acquire) {
                return MinimapResult::Cancelled;
            }
            let chunk_end = (line_end + MINIMAP_CANCELLATION_CHECK_BYTES).min(source.len());
            if let Some(offset) = source.as_bytes()[line_end..chunk_end]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                line_end += offset;
                break;
            }
            line_end = chunk_end;
        }
        let next_cursor = (line_end + usize::from(line_end < source.len())).min(source.len());
        if neutral_fallback {
            // Skipping syntax for only the oversized line would leave the
            // parser state unknown for every line after it.
            if cancelled.load(Ordering::Acquire) {
                return MinimapResult::Cancelled;
            }
            if source_line % stride == 0 {
                lines.push((
                    source_line,
                    build_neutral_minimap_line(&source[cursor..line_end]),
                ));
            }
            cursor = next_cursor;
            drop(source);
            continue;
        }
        line_buffer.clear();
        let mut copy_cursor = cursor;
        while copy_cursor < line_end {
            if cancelled.load(Ordering::Acquire) {
                return MinimapResult::Cancelled;
            }
            let mut copy_end = (copy_cursor + MINIMAP_CANCELLATION_CHECK_BYTES).min(line_end);
            while !source.is_char_boundary(copy_end) {
                copy_end -= 1;
            }
            if copy_end == copy_cursor {
                copy_end = (copy_cursor + 4).min(line_end);
                while !source.is_char_boundary(copy_end) {
                    copy_end += 1;
                }
            }
            line_buffer.push_str(&source[copy_cursor..copy_end]);
            copy_cursor = copy_end;
        }
        if next_cursor > line_end {
            // The syntax set uses newline-aware grammars. Python comments and
            // other line scopes need the source newline to terminate.
            line_buffer.push('\n');
        }
        cursor = next_cursor;
        // Syntect needs a complete logical line to preserve grammar state. Do
        // not keep the full document Arc alive while that line is highlighted.
        drop(source);
        if cancelled.load(Ordering::Acquire) {
            return MinimapResult::Cancelled;
        }
        let minimap_line = build_minimap_line(&line_buffer, &mut highlighter);
        if cancelled.load(Ordering::Acquire) {
            return MinimapResult::Cancelled;
        }
        if source_line % stride == 0 {
            lines.push((source_line, minimap_line));
        }
        if line_buffer.capacity() > MINIMAP_SCRATCH_REUSE_LIMIT {
            line_buffer = String::new();
        }
    }
    if cancelled.load(Ordering::Acquire) {
        MinimapResult::Cancelled
    } else {
        MinimapResult::Lines(lines)
    }
}

/// Draws a VS Code-style overview from syntax-colored rectangles, not glyphs.
fn render_code_minimap(
    ui: &mut egui::Ui,
    lines: &[(usize, MinimapLine)],
    line_first_fragment: &[usize],
    rows: &CodeRowHeights,
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
        hex_to_c32(&palette.colors.surface_panel),
    );

    let content_rect = map_rect.shrink2(Vec2::new(4.0, 3.0));
    let line_count = line_first_fragment.len().max(1);
    // Do not emit thousands of tiny shapes for large sources. Each sampled row
    // still represents its source position, retaining the document's color rhythm.
    let stride =
        ((lines.len() as f32 / content_rect.height().ceil().max(1.0)).ceil() as usize).max(1);
    for (sample_index, (line_index, line)) in lines.iter().enumerate().step_by(stride) {
        let display_row = line_first_fragment.get(*line_index).copied().unwrap_or(0);
        let row_top = content_rect.top()
            + content_rect.height() * rows.offset(display_row) / rows.total_height().max(1.0);
        let next_line = lines
            .get(sample_index + stride)
            .map(|(line, _)| *line)
            .unwrap_or(line_count);
        let next_display_row = line_first_fragment
            .get(next_line)
            .copied()
            .unwrap_or(rows.heights.len());
        let row_bottom = (content_rect.top()
            + content_rect.height() * rows.offset(next_display_row) / rows.total_height().max(1.0))
        .max(row_top + 1.0)
        .min(content_rect.bottom());
        for segment in &line.segments {
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

/// Projects document search ranges into one source line.
#[cfg(test)]
fn line_highlight_ranges(
    matches: &[SearchMatch],
    active_index: usize,
    line_start: usize,
    line_end: usize,
) -> Vec<(usize, usize, bool)> {
    let first = matches.partition_point(|matched| matched.end_byte <= line_start);
    matches[first..]
        .iter()
        .enumerate()
        .take_while(|(_, matched)| matched.start_byte < line_end)
        .map(|(offset, matched)| {
            (
                matched.start_byte.max(line_start) - line_start,
                matched.end_byte.min(line_end) - line_start,
                first + offset == active_index,
            )
        })
        .collect()
}

fn line_highlight_ranges_from_spool(
    spool: &mut SearchMatchSpool,
    active_index: u64,
    line_start: usize,
    line_end: usize,
) -> io::Result<Vec<(usize, usize, bool)>> {
    spool
        .ranges_for_fragment(line_start as u64, line_end as u64)?
        .map(|matched| {
            let (index, matched) = matched?;
            let start = usize::try_from(matched.start_byte).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "match start exceeds platform size",
                )
            })?;
            let end = usize::try_from(matched.end_byte).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "match end exceeds platform size",
                )
            })?;
            Ok((
                start.max(line_start) - line_start,
                end.min(line_end) - line_start,
                index == active_index,
            ))
        })
        .collect()
}

fn paint_code_selection(
    ui: &egui::Ui,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    line: &str,
    fragment: CodeFragment,
    selection_start: usize,
    selection_end: usize,
    color: Color32,
) {
    let start = selection_start.max(fragment.start_byte);
    let end = selection_end.min(fragment.end_byte);
    if start >= end {
        return;
    }
    let start_char = line[..start - fragment.start_byte].chars().count();
    let end_char = line[..end - fragment.start_byte].chars().count();
    let mut row_start = 0;
    for row in &galley.rows {
        let row_end = row_start + row.char_count_including_newline();
        let from = start_char.max(row_start).min(row_end) - row_start;
        let to = end_char.max(row_start).min(row_end) - row_start;
        if from < to {
            let rect = egui::Rect::from_min_max(
                galley_pos + egui::vec2(row.x_offset(from), row.min_y()),
                galley_pos + egui::vec2(row.x_offset(to), row.max_y()),
            );
            ui.painter().rect_filled(rect, egui::Rounding::ZERO, color);
        }
        row_start = row_end;
    }
}

/// Appends only the intersecting part of a syntax token with a search background.
fn append_highlighted_token(
    job: &mut egui::text::LayoutJob,
    token: &str,
    token_start: usize,
    foreground: Color32,
    palette: &AppPalette,
    matches: &[(usize, usize, bool)],
) {
    let token_end = token_start + token.len();
    let mut boundaries = vec![token_start, token_end];
    for &(start, end, _) in matches {
        if start < token_end && end > token_start {
            boundaries.extend([start.max(token_start), end.min(token_end)]);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for range in boundaries.windows(2) {
        let matched = matches
            .iter()
            .find(|&&(start, end, _)| start <= range[0] && end >= range[1]);
        let background = match matched {
            Some((_, _, true)) => hex_to_c32(&palette.harmonies.complementary),
            Some(_) => hex_to_c32_alpha(&palette.diff_mod, 100),
            None => Color32::TRANSPARENT,
        };
        let color = if matched.is_some() {
            crate::ui_ext::contrast_text_c32(background, Some(foreground), palette)
        } else {
            foreground
        };
        job.append(
            &token[range[0] - token_start..range[1] - token_start],
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(MONO_FONT_SIZE),
                color,
                background,
                ..Default::default()
            },
        );
    }
}

/// Builds syntax highlighting with exact, precomputed search ranges.
pub(crate) fn build_highlighted_layout_job(
    text: &str,
    syntax: &SyntaxReference,
    palette: &AppPalette,
    match_ranges: &[(usize, usize, bool)],
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let syn_theme = &THEME_SET.themes["base16-eighties.dark"];
    let mut highlighter = HighlightLines::new(syntax, syn_theme);
    let mut byte_offset = 0;
    for line in text.split_inclusive('\n') {
        for (style, token) in highlighter
            .highlight_line(line, &SYNTAX_SET)
            .unwrap_or_default()
        {
            append_highlighted_token(
                &mut job,
                token,
                byte_offset,
                map_syntect_color_to_theme(style.foreground, palette),
                palette,
                match_ranges,
            );
            byte_offset += token.len();
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
                .fill(hex_to_c32(&palette.colors.surface_panel))
                .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(s)
                            .font(FontId::monospace(size.max(MONO_FONT_SIZE)))
                            .color(crate::ui_ext::contrast_text_c32_on(
                                hex_to_c32(&palette.colors.surface_panel),
                                hex_to_c32(&palette.colors.surface_panel),
                                Some(hex_to_c32(&palette.harmonies.triadic_2)),
                                palette,
                            )),
                    );
                });
        }
        little_clip_render::markdown::MarkdownSpan::Link { label, url } => {
            ui.hyperlink_to(
                egui::RichText::new(label)
                    .size(size)
                    .color(crate::ui_ext::contrast_text_c32_on(
                        hex_to_c32(&palette.colors.surface_panel),
                        hex_to_c32(&palette.colors.surface_panel),
                        Some(hex_to_c32(&palette.harmonies.base)),
                        palette,
                    ))
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
        .fill(hex_to_c32(&palette.colors.surface_recessed))
        .stroke(Stroke::new(
            1.0_f32,
            hex_to_c32(&palette.colors.surface_panel),
        ))
        .inner_margin(egui::Margin::same(10.0))
        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
        .show(ui, |ui| {
            if !lang.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lang.to_uppercase())
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_muted)),
                    );
                });
                ui.add_space(crate::ui_ext::SPACE_XS);
            }

            let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let mut layout_job = build_highlighted_layout_job(text, syntax, palette, &[]);
                layout_job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(layout_job))
            };

            let mut code_clone = code.to_string();
            let text_edit = egui::TextEdit::multiline(&mut code_clone)
                .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
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
    fn inline_text_previews_share_source_with_independent_scroll_state() {
        let data = TextPreviewData::from_text(Arc::new(
            (0..400).map(|line| format!("row-{line:04}\n")).collect(),
        ));
        let mut first = InlineTextPreview::new(Arc::clone(&data), Vec2::new(180.0, 72.0));
        let mut second = InlineTextPreview::new(Arc::clone(&data), Vec2::new(180.0, 72.0));
        assert!(Arc::ptr_eq(&first.data, &second.data));
        assert_ne!(first.id, second.id);

        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let mut rects = [egui::Rect::NOTHING; 2];
        let render_frame = |ctx: &egui::Context,
                            first: &mut InlineTextPreview,
                            second: &mut InlineTextPreview,
                            events: Vec<egui::Event>,
                            time: f64,
                            rects: &mut [egui::Rect; 2]| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(420.0, 180.0),
                    )),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            rects[0] = first.show(ui, &palette).rect;
                            rects[1] = second.show(ui, &palette).rect;
                        });
                    });
                },
            )
        };
        let initial = render_frame(&ctx, &mut first, &mut second, Vec::new(), 0.0, &mut rects);
        assert!(initial.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.job.text == "row-0000")
        }));
        let initial_first_host_rows: Vec<_> = initial
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if rects[0].contains(text.pos) && text.galley.job.text.starts_with("row-") =>
                {
                    text.galley.job.text[4..].parse::<usize>().ok()
                }
                _ => None,
            })
            .collect();
        let initial_first_host_max = *initial_first_host_rows.iter().max().unwrap();

        render_frame(
            &ctx,
            &mut first,
            &mut second,
            vec![egui::Event::PointerMoved(rects[0].center())],
            0.1,
            &mut rects,
        );
        render_frame(
            &ctx,
            &mut first,
            &mut second,
            vec![
                egui::Event::PointerMoved(rects[0].center()),
                egui::Event::Scroll(Vec2::new(0.0, -260.0)),
            ],
            0.2,
            &mut rects,
        );
        let scrolled = render_frame(&ctx, &mut first, &mut second, Vec::new(), 0.3, &mut rects);
        assert!(first.scroll_offset > 0.0, "hovered host should scroll");
        assert_eq!(second.scroll_offset, 0.0, "other host should stay at top");
        assert!(scrolled.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text) if rects[1].contains(text.pos) && text.galley.job.text == "row-0000")
        }), "unhovered host should still show its first row");

        let row_height = ctx.fonts(|fonts| fonts.row_height(&FontId::monospace(MONO_FONT_SIZE)));
        let mut first_host_rows: Vec<_> = scrolled
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text)
                    if rects[0].contains(text.pos) && text.galley.job.text.starts_with("row-") =>
                {
                    Some((text.pos.y, text.galley.job.text.clone()))
                }
                _ => None,
            })
            .collect();
        assert!(
            first_host_rows.iter().any(|(_, label)| {
                label
                    .strip_prefix("row-")
                    .and_then(|index| index.parse::<usize>().ok())
                    .is_some_and(|index| index > initial_first_host_max)
            }),
            "scrolled first host should render a row beyond its initial visible range: {first_host_rows:?}"
        );
        first_host_rows.sort_by(|left, right| left.0.total_cmp(&right.0));
        assert!(
            first_host_rows.len() >= 2,
            "scrolled host should show multiple rows"
        );
        let row_pitch = row_height + ctx.style().spacing.item_spacing.y;
        assert!(
            ((first_host_rows[1].0 - first_host_rows[0].0) - row_pitch).abs() < 1.0,
            "virtual row y positions must match measured row height: {first_host_rows:?}"
        );
        assert!(first.scroll_offset > 0.0);
        assert_eq!(second.scroll_offset, 0.0);
    }

    #[test]
    fn inline_text_preview_skips_layout_when_host_is_clipped() {
        let data = TextPreviewData::from_text(Arc::new("visible only in source".repeat(500)));
        let mut preview = InlineTextPreview::new(data, Vec2::new(180.0, 100.0));
        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(300.0, 100.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add_space(200.0);
                    preview.show(ui, &palette);
                });
            },
        );
        assert!(preview.scroll_offset.is_finite());
        assert_eq!(preview.scroll_offset, 0.0);
        assert!(
            output
                .shapes
                .iter()
                .all(|shape| { !matches!(&shape.shape, egui::epaint::Shape::Text(_)) })
        );
    }

    #[cfg(target_os = "linux")]
    fn open_spool_files_in(directory: &Path) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .filter(|target| target.starts_with(directory))
            .count()
    }

    #[test]
    fn external_editor_handoff_keeps_text_source_search_and_scroll_state() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("source.txt");
        std::fs::write(&path, "first line\nmatch line\nlast line\n").unwrap();
        let mut state = TextPreviewState::from_file(&path, &AppConfig::default()).unwrap();
        state.editor_cmd = "test-editor".to_owned();
        state.show_find = true;
        state.find_query = "match".to_owned();
        state.scroll_to_line = Some(2);
        let content = Arc::clone(&state.content);
        let mut launched_path = None;

        state.trigger_escape_hatch_with(|app, command, target| {
            assert_eq!(app, "test-editor");
            assert_eq!(command, "test-editor");
            launched_path = Some(target.to_path_buf());
            Ok("Launched test editor".to_owned())
        });

        assert_eq!(launched_path.as_deref(), Some(path.as_path()));
        assert!(Arc::ptr_eq(&state.content, &content));
        assert_eq!(state.path.as_deref(), Some(path.as_path()));
        assert!(state.show_find);
        assert_eq!(state.find_query, "match");
        assert_eq!(state.scroll_to_line, Some(2));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        assert_eq!(
            state.status_toast.message(now),
            Some("Launched test editor")
        );
    }

    #[test]
    fn independent_text_views_share_immutable_data_and_keep_view_state_separate() {
        let original_content = Arc::new("fn main() {\n    println!(\"shared\");\n}\n".to_owned());
        let source = TextPreviewData::from_text(Arc::clone(&original_content));
        assert!(Arc::ptr_eq(&source.content, &original_content));
        let mut first = TextPreviewState::from_data(
            Arc::clone(&source),
            Some("First view".to_owned()),
            &AppConfig::default(),
        );
        let second = TextPreviewState::from_data(
            Arc::clone(&source),
            Some("Second view".to_owned()),
            &AppConfig::default(),
        );

        assert!(Arc::ptr_eq(&first.content, &second.content));
        assert!(Arc::ptr_eq(&first.code_fragments, &second.code_fragments));
        assert!(Arc::ptr_eq(
            &first.line_first_fragment,
            &second.line_first_fragment
        ));
        assert_eq!(first.language, "rs");
        assert_eq!(second.language, "rs");
        assert_ne!(first.title, second.title);

        first.show_find = true;
        first.find_query = "shared".to_owned();
        first.scroll_to_line = Some(2);
        first.code_selection = Some((0, 2));

        assert!(!second.show_find);
        assert!(second.find_query.is_empty());
        assert_eq!(second.scroll_to_line, None);
        assert_eq!(second.code_selection, None);
    }

    #[test]
    fn inline_text_source_seeds_independent_full_studios_without_loading_again() {
        let source = TextPreviewData::from_text(Arc::new(
            "fn gallery() {\n    println!(\"source\");\n}\n".to_owned(),
        ));
        let inline = InlineTextPreview::new(Arc::clone(&source), Vec2::new(180.0, 72.0));
        let mut first = TextPreviewState::from_data(
            Arc::clone(&source),
            Some("First editor".to_owned()),
            &AppConfig::default(),
        );
        let second = TextPreviewState::from_data(
            Arc::clone(&source),
            Some("Second editor".to_owned()),
            &AppConfig::default(),
        );

        assert!(Arc::ptr_eq(&inline.data, &source));
        assert!(Arc::ptr_eq(&first.content, &source.content));
        assert!(Arc::ptr_eq(&second.content, &source.content));
        assert!(Arc::ptr_eq(&first.code_fragments, &source.code_fragments));
        assert!(!first.search_pending && first.search_job.is_none());
        assert!(!second.search_pending && second.search_job.is_none());

        first.show_find = true;
        first.find_query = "source".to_owned();
        first.scroll_to_line = Some(2);
        first.code_selection = Some((0, 3));
        assert!(!second.show_find);
        assert!(second.find_query.is_empty());
        assert_eq!(second.scroll_to_line, None);
        assert_eq!(second.code_selection, None);
    }

    #[test]
    fn async_search_coalesces_queries_and_publishes_only_latest_revision() {
        let mut state = TextPreviewState::from_text(
            "old target; newest target; newest target".to_owned(),
            None,
            &AppConfig::default(),
        );
        state.find_query = "old".to_owned();
        state.update_search_matches();
        let first_revision = state.search_revision;
        state.find_query = "newest".to_owned();
        state.update_search_matches();
        assert!(state.search_revision > first_revision);
        assert!(state.search_pending);
        assert_eq!(state.match_count(), 0);

        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 2);
        for index in 0..state.match_count() {
            let matched = state.search_match(index).unwrap();
            assert_eq!(
                &state.content[matched.start_byte..matched.end_byte],
                "newest"
            );
        }
    }

    #[test]
    fn changing_search_query_cooperatively_cancels_previous_job() {
        let jobs = BackgroundJobs::new(1, 1);
        let (ticket, receiver) = jobs.submit(Duration::MAX, || {
            Ok::<_, String>(SearchBuildOutput::Cancelled)
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut state =
            TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
        state.search_job = Some(SearchMatchJob {
            revision: state.search_revision,
            ticket,
            cancelled: Arc::clone(&cancelled),
            receiver,
        });

        state.find_query = "updated".to_owned();
        state.update_search_matches();

        assert!(cancelled.load(Ordering::Acquire));
        assert!(state.search_pending);
        assert!(state.search_job.is_none());
    }

    #[test]
    fn completed_stale_search_revision_is_never_published() {
        let jobs = BackgroundJobs::new(1, 1);
        let (ticket, _ticket_receiver) = jobs.submit(Duration::MAX, || ());
        let mut spool = SearchMatchSpool::new().unwrap();
        spool.append(0, 6).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Ok(Ok(SearchBuildOutput::Matches(spool))))
            .unwrap();

        let mut state =
            TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
        state.search_revision = 2;
        state.search_pending = true;
        state.search_job = Some(SearchMatchJob {
            revision: 1,
            ticket,
            cancelled: Arc::new(AtomicBool::new(false)),
            receiver,
        });
        state.poll_search_job(Instant::now());

        assert!(state.search_pending);
        assert_eq!(state.match_count(), 0);
        assert!(state.matches.is_none());
    }

    #[test]
    fn pending_search_completes_when_find_toolbar_is_hidden() {
        let mut state =
            TextPreviewState::from_text("needle here".to_owned(), None, &AppConfig::default());
        state.find_query = "needle".to_owned();
        state.update_search_matches();
        state.search_debounce_until = Some(Instant::now());
        assert!(!state.show_find);

        let context = egui::Context::default();
        let palette = AppPalette::default();
        let mut close = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while state.search_pending && Instant::now() < deadline {
            let _ = context.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(ui, ctx, &palette, &mut close);
                });
            });
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!state.search_pending);
        assert_eq!(state.match_count(), 1);
    }

    #[test]
    fn spool_read_failure_clears_completed_search_results() {
        let directory = tempfile::tempdir().unwrap();
        let mut state =
            TextPreviewState::from_text("needle".to_owned(), None, &AppConfig::default());
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
        for index in 0..=records_at_limit as u64 {
            spool.append(index * 2, index * 2 + 1).unwrap();
        }
        assert!(spool.is_rolled());
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 1);
        spool.file.set_len(0).unwrap();
        state.matches = Some(spool);
        state.match_count = 1;
        state.active_match_idx = 0;
        state.search_match_at(0);

        assert!(
            state
                .search_error
                .as_deref()
                .is_some_and(|error| { error.starts_with("could not read search results:") })
        );
        assert_eq!(state.match_count(), 0);
        assert_eq!(state.active_match_idx, 0);
        assert!(state.matches.is_none());
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 0);
    }

    #[test]
    fn dropping_preview_releases_completed_search_spool() {
        let directory = tempfile::tempdir().unwrap();
        let mut state =
            TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
        for index in 0..=records_at_limit as u64 {
            spool.append(index * 2, index * 2 + 1).unwrap();
        }
        assert!(spool.is_rolled());
        state.match_count = spool.len();
        state.matches = Some(spool);

        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 1);
        drop(state);
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 0);
    }

    #[test]
    fn replacing_preview_releases_previous_search_spool() {
        let directory = tempfile::tempdir().unwrap();
        let mut previous =
            TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
        for index in 0..=records_at_limit as u64 {
            spool.append(index * 2, index * 2 + 1).unwrap();
        }
        previous.match_count = spool.len();
        previous.matches = Some(spool);

        let mut active_preview = Some(previous);
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 1);
        let replaced = active_preview.replace(TextPreviewState::from_text(
            "new preview".to_owned(),
            None,
            &AppConfig::default(),
        ));
        drop(replaced);
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 0);
    }

    #[test]
    fn closing_preview_cancels_running_search_and_releases_spilled_file() {
        let directory = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let worker_directory = directory.path().to_path_buf();
        let (spooled_tx, spooled_rx) = std::sync::mpsc::channel();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            let mut spool = SearchMatchSpool::new_in(&worker_directory).unwrap();
            let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
            for index in 0..=records_at_limit as u64 {
                spool.append(index * 2, index * 2 + 1).unwrap();
            }
            assert!(spool.is_rolled());
            spooled_tx.send(()).unwrap();
            while !worker_cancelled.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            drop(spool);
            dropped_tx.send(()).unwrap();
            Ok::<_, String>(SearchBuildOutput::Cancelled)
        });
        let mut state =
            TextPreviewState::from_text("target".to_owned(), None, &AppConfig::default());
        state.search_job = Some(SearchMatchJob {
            revision: state.search_revision,
            ticket,
            cancelled: Arc::clone(&cancelled),
            receiver,
        });

        spooled_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 1);
        drop(state);

        assert!(cancelled.load(Ordering::Acquire));
        dropped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(open_spool_files_in(directory.path()), 0);
    }

    #[test]
    fn out_of_range_search_lookup_preserves_valid_results() {
        let mut state =
            TextPreviewState::from_text("needle".to_owned(), None, &AppConfig::default());
        let mut spool = SearchMatchSpool::new().unwrap();
        spool.append(0, 6).unwrap();
        state.matches = Some(spool);
        state.match_count = 1;

        assert!(state.search_match_at(1).is_none());
        assert_eq!(state.match_count(), 1);
        assert!(state.search_error.is_none());
        assert!(state.matches.is_some());
    }

    #[test]
    fn recomputing_search_preserves_or_resets_active_match_ordinal() {
        let mut state = TextPreviewState::from_text(
            "one two two two two".to_owned(),
            None,
            &AppConfig::default(),
        );
        state.is_regex = true;
        state.find_query = "two|one".to_owned();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 5);
        state.active_match_idx = 2;
        state.find_query = "two".to_owned();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 4);
        assert_eq!(state.active_match_idx, 2);

        state.active_match_idx = 4;
        state.find_query = "one".to_owned();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 1);
        assert_eq!(state.active_match_idx, 0);

        state.is_regex = true;
        state.find_query = "(".to_owned();
        update_search_matches_and_wait(&mut state);
        assert!(state.regex_error.is_some());
        assert_eq!(state.active_match_idx, 0);
    }

    #[test]
    fn async_search_retries_after_background_queue_saturation() {
        let jobs = BackgroundJobs::new(1, 1);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_blocker_ticket, _blocker_rx) = jobs.submit(Duration::MAX, move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (_queued_ticket, queued_receiver) = jobs.submit(Duration::MAX, || ());

        let mut state =
            TextPreviewState::from_text("target target".to_owned(), None, &AppConfig::default());
        state.find_query = "target".to_owned();
        state.update_search_matches();
        let now = Instant::now();
        state.search_debounce_until = Some(now);
        let context = egui::Context::default();
        state.service_search_jobs(&context, &jobs, now);
        state.service_search_jobs(&context, &jobs, now);
        assert!(state.search_pending);
        assert_eq!(state.match_count(), 0);
        let retry_at = state
            .search_retry_at
            .expect("saturated queue schedules retry");

        release_tx.send(()).unwrap();
        queued_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        state.service_search_jobs(&context, &jobs, retry_at + SEARCH_RETRY_DELAY);
        let deadline = Instant::now() + Duration::from_secs(2);
        while state.search_pending && Instant::now() < deadline {
            state.service_search_jobs(&context, &jobs, Instant::now());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            !state.search_pending,
            "retry should publish completed results"
        );
        assert_eq!(state.match_count(), 2);
    }

    fn collect_fragment_ranges(
        spool: &mut SearchMatchSpool,
        start: u64,
        end: u64,
    ) -> Vec<SearchMatchRange> {
        spool
            .ranges_for_fragment(start, end)
            .unwrap()
            .map(|result| result.map(|(_, range)| range))
            .collect::<io::Result<Vec<_>>>()
            .unwrap()
    }

    #[test]
    fn search_match_spool_uses_the_application_staging_directory() {
        let mut spool = SearchMatchSpool::new().unwrap();
        spool.append(10, 20).unwrap();
        assert_eq!(
            spool.get(0).unwrap(),
            SearchMatchRange {
                start_byte: 10,
                end_byte: 20
            }
        );
    }

    #[test]
    fn search_match_spool_round_trips_records_by_index_after_reads() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        spool.append(3, 8).unwrap();
        assert_eq!(
            spool.get(0).unwrap(),
            SearchMatchRange {
                start_byte: 3,
                end_byte: 8
            }
        );

        spool
            .append(u32::MAX as u64 + 9, u32::MAX as u64 + 19)
            .unwrap();
        assert_eq!(spool.len(), 2);
        assert_eq!(
            spool.get(1).unwrap(),
            SearchMatchRange {
                start_byte: u32::MAX as u64 + 9,
                end_byte: u32::MAX as u64 + 19,
            }
        );
        assert_eq!(
            spool.get(0).unwrap(),
            SearchMatchRange {
                start_byte: 3,
                end_byte: 8
            }
        );
        assert_eq!(
            spool.get(2).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn search_match_spool_preserves_zero_width_records() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        spool.append(42, 42).unwrap();

        assert_eq!(
            spool.get(0).unwrap(),
            SearchMatchRange {
                start_byte: 42,
                end_byte: 42,
            }
        );
    }

    #[test]
    fn search_match_spool_fragment_lookup_includes_crossing_ranges() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        for range in [(0, 2), (3, 6), (6, 8), (10, 10), (13, 15)] {
            spool.append(range.0, range.1).unwrap();
        }

        let ranges = collect_fragment_ranges(&mut spool, 4, 7);
        assert_eq!(
            ranges,
            vec![
                SearchMatchRange {
                    start_byte: 3,
                    end_byte: 6,
                },
                SearchMatchRange {
                    start_byte: 6,
                    end_byte: 8,
                },
            ]
        );
    }

    #[test]
    fn search_match_spool_fragment_lookup_observes_half_open_and_zero_width_edges() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        for range in [(2, 4), (4, 4), (5, 5), (6, 9), (9, 9)] {
            spool.append(range.0, range.1).unwrap();
        }

        let ranges = collect_fragment_ranges(&mut spool, 4, 9);
        assert_eq!(
            ranges,
            vec![
                SearchMatchRange {
                    start_byte: 5,
                    end_byte: 5,
                },
                SearchMatchRange {
                    start_byte: 6,
                    end_byte: 9,
                },
            ]
        );
        assert!(
            spool
                .ranges_for_fragment(9, 10)
                .unwrap()
                .map(|result| result.map(|(_, range)| range))
                .collect::<io::Result<Vec<_>>>()
                .unwrap()
                .is_empty()
        );
        assert!(spool.ranges_for_fragment(5, 5).unwrap().next().is_none());
    }

    #[test]
    fn search_match_spool_fragment_lookup_handles_empty_and_outside_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        spool.append(5, 7).unwrap();
        spool.append(10, 12).unwrap();

        for (start, end) in [(0, 5), (12, 20), (8, 8)] {
            assert!(
                spool
                    .ranges_for_fragment(start, end)
                    .unwrap()
                    .map(|result| result.map(|(_, range)| range))
                    .collect::<io::Result<Vec<_>>>()
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn search_match_spool_requires_ordered_ranges_and_recovers_after_partial_iteration() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        spool.append(2, 4).unwrap();
        spool.append(5, 7).unwrap();
        assert!(spool.append(4, 6).is_err(), "starts must be ordered");
        assert!(spool.append(8, 6).is_err(), "ranges cannot be reversed");
        assert!(spool.append(6, 6).is_err(), "ends must be ordered");

        let mut ranges = spool.ranges_for_fragment(0, 10).unwrap();
        assert_eq!(ranges.next().unwrap().unwrap().1.start_byte, 2);
        drop(ranges);

        spool.append(9, 10).unwrap();
        assert_eq!(spool.len(), 3);
        assert_eq!(spool.get(2).unwrap().start_byte, 9);
    }

    #[test]
    fn search_match_spool_rolls_over_only_after_the_memory_threshold_and_cleans_up() {
        let directory = tempfile::tempdir().unwrap();
        let mut spool = SearchMatchSpool::new_in(directory.path()).unwrap();
        let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
        for index in 0..records_at_limit as u64 {
            spool.append(index * 2, index * 2 + 1).unwrap();
        }
        assert!(!spool.is_rolled());
        assert_eq!(spool.len(), records_at_limit as u64);

        spool.append(100_000, 100_001).unwrap();
        assert!(spool.is_rolled());
        assert_eq!(
            spool.get(0).unwrap(),
            SearchMatchRange {
                start_byte: 0,
                end_byte: 1
            }
        );
        assert_eq!(
            spool.get(records_at_limit as u64).unwrap(),
            SearchMatchRange {
                start_byte: 100_000,
                end_byte: 100_001,
            }
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);

        drop(spool);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn search_match_spool_reports_directory_and_rollover_io_errors() {
        let parent = tempfile::tempdir().unwrap();
        let not_a_directory = parent.path().join("regular-file");
        std::fs::write(&not_a_directory, b"file").unwrap();
        assert!(SearchMatchSpool::new_in(&not_a_directory).is_err());

        let spill_directory = parent.path().join("spill");
        let mut spool = SearchMatchSpool::new_in(&spill_directory).unwrap();
        let records_at_limit = SEARCH_MATCH_MEMORY_LIMIT_BYTES / SEARCH_MATCH_RECORD_BYTES;
        for index in 0..records_at_limit as u64 {
            spool.append(index, index + 1).unwrap();
        }
        std::fs::remove_dir(&spill_directory).unwrap();
        assert!(spool.append(10_000, 10_000).is_err());
        assert!(!spool.is_rolled());
        assert_eq!(spool.len(), records_at_limit as u64);
        assert_eq!(
            spool.get(0).unwrap(),
            SearchMatchRange {
                start_byte: 0,
                end_byte: 1
            }
        );
        std::fs::create_dir(&spill_directory).unwrap();
        spool.append(10_000, 10_000).unwrap();
        assert!(spool.is_rolled());
        assert_eq!(spool.len(), records_at_limit as u64 + 1);
        assert_eq!(
            spool.get(records_at_limit as u64).unwrap(),
            SearchMatchRange {
                start_byte: 10_000,
                end_byte: 10_000,
            }
        );
    }

    #[test]
    fn find_shortcut_yields_to_focused_text_control() {
        let ctx = egui::Context::default();
        let mut text = String::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.add(egui::TextEdit::singleline(&mut text));
                response.request_focus();
            });
        });
        assert!(ctx.wants_keyboard_input());

        let mut state =
            TextPreviewState::from_text("editable content".to_owned(), None, &AppConfig::default());
        let mut close = false;
        let modifiers = egui::Modifiers::COMMAND;
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::F,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| state.handle_shortcuts(ctx, &mut close),
        );
        assert!(!state.show_find);
        assert!(!close);
    }

    #[test]
    fn find_shortcut_focuses_search_and_escape_dismisses_only_find() {
        let ctx = egui::Context::default();
        let palette = AppPalette::default();
        let mut state = TextPreviewState::from_text(
            "searchable content".to_owned(),
            None,
            &AppConfig::default(),
        );
        let mut close = false;
        let modifiers = egui::Modifiers::COMMAND;
        let _ = ctx.run(
            egui::RawInput {
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::F,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                state.handle_shortcuts(ctx, &mut close);
                if state.show_find {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_search_toolbar(ui, &palette);
                    });
                }
            },
        );
        assert!(state.show_find);
        assert!(ctx.wants_keyboard_input());

        let _ = ctx.run(
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
            |ctx| state.handle_shortcuts(ctx, &mut close),
        );
        assert!(!state.show_find);
        assert!(!close);
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        little_clip_core::palette::contrast_ratio((a.r(), a.g(), a.b()), (b.r(), b.g(), b.b()))
    }

    #[test]
    fn syntax_and_markdown_custom_labels_meet_contrast_in_both_appearances() {
        let syntax_colors = [
            [255, 0, 0],
            [255, 128, 0],
            [255, 255, 0],
            [0, 255, 0],
            [0, 255, 255],
            [0, 0, 255],
            [255, 0, 255],
            [80, 80, 80],
            [220, 220, 220],
        ];
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = AppPalette::for_appearance(appearance);
            let recessed = hex_to_c32(&palette.colors.surface_recessed);
            for [r, g, b] in syntax_colors {
                let foreground = map_syntect_color_to_theme(
                    syntect::highlighting::Color { r, g, b, a: 255 },
                    &palette,
                );
                assert!(
                    contrast(recessed, foreground) >= 4.5,
                    "{appearance:?} syntax hue [{r}, {g}, {b}] lacks contrast"
                );
            }

            let panel = hex_to_c32(&palette.colors.surface_panel);
            for preferred in [
                hex_to_c32(&palette.harmonies.triadic_2),
                hex_to_c32(&palette.harmonies.base),
            ] {
                let foreground =
                    crate::ui_ext::contrast_text_c32_on(panel, panel, Some(preferred), &palette);
                assert!(
                    contrast(panel, foreground) >= 4.5,
                    "{appearance:?} Markdown code/link label lacks contrast"
                );
            }

            for preferred in [
                hex_to_c32(&palette.diff_del),
                hex_to_c32(&palette.harmonies.complementary),
            ] {
                let foreground = crate::ui_ext::contrast_text_c32_on(
                    recessed,
                    recessed,
                    Some(preferred),
                    &palette,
                );
                assert!(
                    contrast(recessed, foreground) >= 4.5,
                    "{appearance:?} text-search status label lacks contrast"
                );
            }
        }
    }

    #[test]
    fn text_search_toolbar_wraps_status_and_controls_at_compact_widths() {
        for width in [390.0, 760.0, 1280.0] {
            for (query, regex_error, matches, expected_status) in [
                (
                    "(".to_string(),
                    Some("unclosed group".to_string()),
                    Vec::new(),
                    "Regex error:",
                ),
                ("missing".to_string(), None, Vec::new(), "No matches"),
                (
                    "Alpha".to_string(),
                    None,
                    vec![SearchMatch {
                        start_byte: 0,
                        end_byte: 5,
                        line: 0,
                    }],
                    "1/1 matches",
                ),
            ] {
                let ctx = egui::Context::default();
                crate::ui_ext::configure_workbench_style(&ctx);
                let palette = AppPalette::default();
                let mut state = TextPreviewState::from_text(
                    "Alpha beta".to_owned(),
                    None,
                    &AppConfig::default(),
                );
                state.find_query = query;
                state.regex_error = regex_error;
                if !matches.is_empty() {
                    let mut spool = SearchMatchSpool::new().unwrap();
                    for matched in matches {
                        spool
                            .append(matched.start_byte as u64, matched.end_byte as u64)
                            .unwrap();
                    }
                    state.match_count = spool.len();
                    state.matches = Some(spool);
                }
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 300.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_search_toolbar(ui, &palette);
                        });
                    },
                );

                for required in [expected_status, "Prev", "Next", "Aa", ".*"] {
                    let shape = output.shapes.iter().find_map(|clipped| {
                        let egui::Shape::Text(text) = &clipped.shape else {
                            return None;
                        };
                        text.galley
                            .job
                            .text
                            .contains(required)
                            .then_some((clipped.clip_rect, text))
                    });
                    let Some((clip_rect, text)) = shape else {
                        panic!("{required:?} missing at width {width}");
                    };
                    let bounds = text.galley.rect.translate(text.pos.to_vec2());
                    assert!(!text.galley.elided, "{required:?} elided at width {width}");
                    assert!(
                        bounds.left() >= clip_rect.left() - 0.5
                            && bounds.right() <= clip_rect.right() + 0.5,
                        "{required:?} exceeds {width}-point search toolbar: {bounds:?} clipped to {clip_rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn text_viewer_chrome_stays_inside_compact_and_wide_hosts() {
        for width in [760.0, 1280.0, 1760.0] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette = AppPalette::default();
            let mut state = TextPreviewState::from_text(
                "fn alpha() {}\nfn bravo() {}\nfn charlie() {}".to_owned(),
                Some("Example.rs".to_owned()),
                &AppConfig::default(),
            );
            state.language = "rs";
            state.show_line_numbers = true;
            state.wrap_lines = true;
            let mut close = false;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 720.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_ui(ui, ctx, &palette, &mut close);
                    });
                },
            );

            for required in [
                "Copy All",
                "Find (Ctrl+F)",
                "Wrap: ON",
                "Lines: ON",
                "3 lines",
                "Ctrl+F: Find",
            ] {
                let shape = output.shapes.iter().find_map(|clipped| {
                    let egui::epaint::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    text.galley
                        .job
                        .text
                        .contains(required)
                        .then_some((clipped.clip_rect, text))
                });
                let Some((clip_rect, text)) = shape else {
                    panic!("text viewer chrome omitted {required:?} at {width} points");
                };
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                assert!(!text.galley.elided, "{required:?} elided at {width} points");
                assert!(
                    bounds.left() >= clip_rect.left() - 0.5
                        && bounds.right() <= clip_rect.right() + 0.5,
                    "{required:?} exceeds text viewer at {width} points: {bounds:?} clipped to {clip_rect:?}"
                );
            }
        }
    }

    #[test]
    fn rendered_copy_all_button_copies_full_text_preview_source() {
        let content = format!(
            "First line\n日本語 and emoji 🧪\n{}",
            "full source row\n".repeat(64)
        );
        let mut state = TextPreviewState::from_text(
            content.clone(),
            Some("Copy fixture.txt".to_owned()),
            &AppConfig::default(),
        );
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = AppPalette::default();
        let mut close = false;
        let mut time = 0.0;
        let mut render =
            |state: &mut TextPreviewState, time: &mut f64, events: Vec<egui::Event>| {
                *time += 0.1;
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1200.0, 800.0),
                        )),
                        time: Some(*time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_ui(ui, ctx, &palette, &mut close);
                        });
                    },
                )
            };

        let frame = render(&mut state, &mut time, Vec::new());
        let copy_position = frame
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().contains("Copy All") => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .expect("rendered Copy All button label");
        render(
            &mut state,
            &mut time,
            vec![egui::Event::PointerMoved(copy_position)],
        );
        render(
            &mut state,
            &mut time,
            vec![
                egui::Event::PointerMoved(copy_position),
                egui::Event::PointerButton {
                    pos: copy_position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let copied = render(
            &mut state,
            &mut time,
            vec![
                egui::Event::PointerMoved(copy_position),
                egui::Event::PointerButton {
                    pos: copy_position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );

        assert_eq!(copied.platform_output.copied_text, content);
        assert!(!close);
    }

    #[test]
    fn markdown_body_list_and_quote_wrap_without_clipping_at_compact_and_wide_widths() {
        let markers = ["BODY_MARKER", "LIST_MARKER", "QUOTE_MARKER"];
        let content = format!(
            "BODY_MARKER {}\n\n- LIST_MARKER {}\n\n> QUOTE_MARKER {}",
            "paragraph text that should wrap naturally without being elided or clipped ".repeat(5),
            "list text that should wrap naturally without being elided or clipped ".repeat(5),
            "quoted text that should wrap naturally without being elided or clipped ".repeat(5),
        );

        for (label, width) in [("compact", 390.0), ("wide", 1400.0)] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette = AppPalette::default();
            let mut state =
                TextPreviewState::from_text(content.clone(), None, &AppConfig::default());
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 1800.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_markdown_view(ui, &palette);
                    });
                },
            );

            for marker in markers {
                let matching: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|clipped| match &clipped.shape {
                        egui::Shape::Text(text) if text.galley.text().contains(marker) => {
                            Some((clipped.clip_rect, text))
                        }
                        _ => None,
                    })
                    .collect();
                assert!(
                    !matching.is_empty(),
                    "{label} Markdown render omitted {marker}"
                );

                let mut rendered_rows = 0;
                for (clip_rect, text) in matching {
                    let bounds = text.galley.rect.translate(text.pos.to_vec2());
                    assert!(
                        bounds.left() >= clip_rect.left() - 0.5
                            && bounds.right() <= clip_rect.right() + 0.5,
                        "{label} {marker} exceeds horizontal clip bounds: {bounds:?} vs {clip_rect:?}"
                    );
                    assert!(
                        bounds.top() >= clip_rect.top() - 0.5
                            && bounds.bottom() <= clip_rect.bottom() + 0.5,
                        "{label} {marker} exceeds vertical clip bounds: {bounds:?} vs {clip_rect:?}"
                    );
                    assert!(!text.galley.elided, "{label} {marker} was elided");
                    assert!(
                        text.galley.job.sections.iter().all(|section| {
                            (section.format.font_id.size - BODY_FONT_SIZE).abs() < 0.1
                        }),
                        "{label} {marker} did not use the shared body font size"
                    );
                    rendered_rows += text.galley.rows.len();
                }
                if label == "compact" {
                    assert!(
                        rendered_rows > 1,
                        "compact {marker} did not wrap: {rendered_rows} rendered rows"
                    );
                }
            }
        }
    }

    #[test]
    fn measured_row_offsets_find_wrapped_rows_and_invalidate_on_width_change() {
        let mut rows = CodeRowHeights::default();
        rows.reset(4, 200.0, 20.0, 1.0);
        rows.set_height(0, 80.0);
        rows.set_height(2, 40.0);
        assert_eq!(rows.offset(2), 100.0);
        assert_eq!(rows.total_height(), 160.0);
        assert_eq!(rows.row_at(79.0), 0);
        assert_eq!(rows.row_at(80.0), 1);
        assert_eq!(rows.row_at(139.0), 2);
        assert_eq!(rows.row_at(1000.0), 3);
        rows.reset(4, 200.0, 20.0, 1.0);
        assert_eq!(rows.total_height(), 160.0);
        rows.reset(4, 400.0, 20.0, 1.0);
        assert_eq!(rows.total_height(), 80.0);
        rows.reset(0, 400.0, 20.0, 1.0);
        assert_eq!(rows.row_at(0.0), 0);
        assert_eq!(rows.total_height(), 0.0);
    }

    #[test]
    fn block_height_index_handles_boundaries_updates_and_partial_final_block() {
        let mut rows = CodeRowHeights::default();
        rows.reset(70, 200.0, 10.0, 1.0);
        assert_eq!(
            rows.sums.len(),
            4,
            "three blocks need three Fenwick entries plus zero"
        );
        assert_eq!(rows.total_height(), 700.0);
        assert_eq!(rows.offset(CODE_ROW_HEIGHT_BLOCK_SIZE), 320.0);
        assert_eq!(rows.row_at(319.0), 31);
        assert_eq!(
            rows.row_at(320.0),
            32,
            "exact block edge begins the next row"
        );

        rows.set_height(0, 20.0);
        rows.set_height(31, 15.0);
        rows.set_height(32, 7.0);
        rows.set_height(69, 30.0);
        assert_eq!(rows.offset(32), 335.0);
        assert_eq!(rows.offset(33), 342.0);
        assert_eq!(rows.row_at(334.0), 31);
        assert_eq!(rows.row_at(335.0), 32);
        assert_eq!(rows.row_at(342.0), 33);
        assert_eq!(rows.total_height(), 732.0);
        assert_eq!(rows.offset(usize::MAX), 732.0);
        assert_eq!(rows.row_at(732.0), 69);
        assert_eq!(rows.row_at(10_000.0), 69);

        rows.set_height(32, 6.0);
        rows.set_height(32, 7.0);
        assert_eq!(
            rows.offset(33),
            342.0,
            "repeated updates apply only the latest delta"
        );

        let mut empty = CodeRowHeights::default();
        empty.reset(0, 200.0, 10.0, 1.0);
        assert!(empty.heights.is_empty());
        assert_eq!(empty.sums, vec![0.0]);
        assert_eq!(empty.offset(0), 0.0);
        assert_eq!(empty.row_at(0.0), 0);
    }

    #[test]
    fn block_height_index_resets_all_measurements_when_geometry_key_changes() {
        let mut rows = CodeRowHeights::default();
        rows.reset(70, 200.0, 10.0, 1.0);
        rows.set_height(34, 28.0);
        rows.next_measurement = 19;
        rows.reset(70, 400.0, 10.0, 1.0);
        assert_eq!(
            rows.total_height(),
            700.0,
            "width change restores estimates"
        );
        assert_eq!(rows.next_measurement, 0);

        rows.set_height(34, 28.0);
        rows.next_measurement = 19;
        rows.reset(70, 400.0, 12.0, 1.0);
        assert_eq!(
            rows.total_height(),
            840.0,
            "line-height change restores estimates"
        );
        assert_eq!(rows.next_measurement, 0);

        rows.set_height(34, 28.0);
        rows.next_measurement = 19;
        rows.reset(70, 400.0, 12.0, 1.5);
        assert_eq!(
            rows.total_height(),
            840.0,
            "scale change restores estimates"
        );
        assert_eq!(rows.next_measurement, 0);

        rows.set_height(34, 28.0);
        rows.next_measurement = 19;
        rows.reset(70, 400.0, 12.0, 1.5);
        assert_eq!(
            rows.total_height(),
            856.0,
            "an unchanged key preserves measurements"
        );
        assert_eq!(rows.next_measurement, 19);
    }

    #[test]
    fn huge_single_line_uses_bounded_utf8_fragments_and_keeps_full_search_source() {
        let marker = "MATCH_AT_END";
        let content = format!("{}{}", "界ab".repeat(40_000), marker);
        let config = AppConfig::default();
        let mut state = TextPreviewState::from_text(content.clone(), None, &config);

        assert!(state.code_fragments.len() > 1);
        assert!(
            state
                .code_fragments
                .iter()
                .all(|fragment| fragment.end_byte - fragment.start_byte <= MAX_CODE_FRAGMENT_BYTES)
        );
        assert!(state.code_fragments.iter().all(|fragment| {
            content.is_char_boundary(fragment.start_byte)
                && content.is_char_boundary(fragment.end_byte)
        }));
        assert_eq!(
            state
                .code_fragments
                .iter()
                .map(|fragment| &content[fragment.start_byte..fragment.end_byte])
                .collect::<String>(),
            content
        );

        state.find_query = marker.to_owned();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 1);
        let matched = state.search_match(0).unwrap();
        assert_eq!(&state.content[matched.start_byte..matched.end_byte], marker);
        state.next_match();
        let row = state.scroll_to_line.unwrap() - 1;
        let fragment = state.code_fragments[row];
        assert!(fragment.start_byte <= matched.start_byte);
        assert!(matched.start_byte < fragment.end_byte);
    }

    #[test]
    fn huge_line_rendering_only_shapes_bounded_fragments() {
        let ctx = egui::Context::default();
        let content = "long text ".repeat(10_000);
        let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 240.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_raw_code_view(ui, &AppPalette::default());
                });
            },
        );
        let rendered_fragment_lengths: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().len()),
                _ => None,
            })
            .collect();
        assert!(!rendered_fragment_lengths.is_empty());
        assert!(
            rendered_fragment_lengths
                .iter()
                .all(|length| *length <= MAX_CODE_FRAGMENT_BYTES)
        );
    }

    #[test]
    fn gutter_labels_wrapped_source_line_once_on_its_first_fragment() {
        let ctx = egui::Context::default();
        let long_line = "x".repeat(MAX_CODE_FRAGMENT_BYTES + 900);
        let content = format!("fn first() {{}}\n{long_line}\nfn last() {{}}\n");
        let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
        state.show_line_numbers = true;
        state.wrap_lines = false;
        let target_first_row = state.line_first_fragment[1];
        let target_fragment = state.code_fragments[target_first_row];
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(700.0, 280.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_raw_code_view(ui, &AppPalette::default());
                });
            },
        );
        let label_shapes: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "2" => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(
            label_shapes.len(),
            1,
            "logical line label should be painted once"
        );
        let first_fragment_text =
            &state.content[target_fragment.start_byte..target_fragment.end_byte];
        let first_fragment_shape = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == first_fragment_text => Some(text),
                _ => None,
            })
            .expect("first wrapped fragment should be visible");
        assert_eq!(label_shapes[0].pos.y, first_fragment_shape.pos.y);
    }

    #[test]
    fn raw_selection_copies_exact_source_across_fragments_and_virtual_scroll() {
        let prefix = (0..180)
            .map(|row| format!("before-{row}\n"))
            .collect::<String>();
        let long_line = format!("{}界{}", "a".repeat(4090), "b".repeat(4200));
        let content = format!("{prefix}{long_line}\nafter\n");
        let mut state = TextPreviewState::from_text(content.clone(), None, &AppConfig::default());
        state.wrap_lines = true;
        let first_row = state.line_first_fragment[180];
        let first = state.code_fragments[first_row];
        let second = state.code_fragments[first_row + 1];
        let start = first.end_byte - 12;
        let end = second.start_byte + 16;
        assert!(content.is_char_boundary(start));
        let ctx = egui::Context::default();
        let mut time = 0.0;
        state.scroll_to_line = Some(first_row + 1);
        let render = |state: &mut TextPreviewState, time: &mut f64, events: Vec<egui::Event>| {
            *time += 0.05;
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(5_000.0, 700.0),
                    )),
                    time: Some(*time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_raw_code_view(ui, &AppPalette::default());
                    });
                },
            )
        };
        let mut frame = render(&mut state, &mut time, Vec::new());
        for _ in 0..4 {
            frame = render(&mut state, &mut time, Vec::new());
        }
        assert!(frame.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == &content[first.start_byte..first.end_byte] && text.galley.rows.len() > 1)
        }));
        let point_for = |frame: &egui::FullOutput, fragment: CodeFragment, byte: usize| {
            let fragment_text = &content[fragment.start_byte..fragment.end_byte];
            frame
                .shapes
                .iter()
                .find_map(|clipped| {
                    let egui::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    if text.galley.text() != fragment_text {
                        return None;
                    }
                    let char_index = content[fragment.start_byte..byte].chars().count();
                    let cursor = text
                        .galley
                        .pos_from_ccursor(egui::text::CCursor::new(char_index));
                    assert_eq!(
                        text.galley
                            .cursor_from_pos(cursor.center().to_vec2())
                            .ccursor
                            .index,
                        char_index,
                        "galley cursor round-trip failed for source byte {byte}"
                    );
                    let point = text.pos + cursor.center().to_vec2();
                    clipped.clip_rect.contains(point).then_some(point)
                })
                .expect("source cursor must be visible")
        };
        let start_pos = point_for(&frame, first, start);
        let end_pos = point_for(&frame, second, end);
        for (from, to, expected_anchor, expected_focus) in [
            (start_pos, end_pos, start, end),
            (end_pos, start_pos, end, start),
        ] {
            render(
                &mut state,
                &mut time,
                vec![
                    egui::Event::PointerMoved(from),
                    egui::Event::PointerButton {
                        pos: from,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            let dragged = render(&mut state, &mut time, vec![egui::Event::PointerMoved(to)]);
            let selection_color = hex_to_c32_alpha(&AppPalette::default().harmonies.base, 90);
            assert!(dragged.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == selection_color)
            }));
            render(
                &mut state,
                &mut time,
                vec![egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(
                state.code_selection,
                Some((expected_anchor, expected_focus))
            );
            assert_eq!(
                render(&mut state, &mut time, vec![egui::Event::Copy])
                    .platform_output
                    .copied_text,
                content[start..end]
            );
        }
        state.scroll_to_line = Some(1);
        let offscreen = render(&mut state, &mut time, Vec::new());
        assert!(!offscreen.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == &content[first.start_byte..first.end_byte])
        }));
        assert_eq!(state.code_selection, Some((end, start)));
        assert_eq!(
            render(&mut state, &mut time, vec![egui::Event::Copy])
                .platform_output
                .copied_text,
            content[start..end]
        );

        state.code_selection = None;
        assert_eq!(
            render(&mut state, &mut time, vec![egui::Event::Copy])
                .platform_output
                .copied_text,
            content
        );

        state.scroll_to_line = Some(first_row);
        let mut frame = render(&mut state, &mut time, Vec::new());
        // This row is near EOF. Let egui clamp the requested top row to
        // the final viewport before capturing positions for pointer input.
        for _ in 0..3 {
            frame = render(&mut state, &mut time, Vec::new());
        }
        let previous = state.code_fragments[first_row - 1];
        let line_start = previous.end_byte - 3;
        let line_end = first.start_byte + 9;
        let from = point_for(&frame, previous, line_start);
        let to = point_for(&frame, first, line_end);
        render(
            &mut state,
            &mut time,
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        render(&mut state, &mut time, vec![egui::Event::PointerMoved(to)]);
        render(
            &mut state,
            &mut time,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(state.code_selection, Some((line_start, line_end)));
        assert!(content[line_start..line_end].contains('\n'));
        assert_eq!(
            render(&mut state, &mut time, vec![egui::Event::Copy])
                .platform_output
                .copied_text,
            content[line_start..line_end]
        );
    }

    #[test]
    fn minimap_scroll_extent_stays_at_document_height_at_end_of_file() {
        for wrap in [false, true] {
            let ctx = egui::Context::default();
            let content = (0..100)
                .map(|line| format!("LINE_{line:03} {}\n", "word ".repeat(25)))
                .collect::<String>();
            let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
            state.wrap_lines = wrap;
            let render = |state: &mut TextPreviewState, events: Vec<egui::Event>| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(480.0, 400.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_raw_code_view(ui, &AppPalette::default())
                        });
                    },
                )
            };
            for _ in 0..30 {
                let _ = render(&mut state, Vec::new());
            }
            let frame = render(&mut state, Vec::new());
            let map = frame
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if (rect.rect.width() - MINIMAP_WIDTH).abs() < 0.1 => {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .expect("overflowing file should paint a minimap");
            let click = egui::pos2(map.center().x, map.bottom() - 4.0);
            for pressed in [true, false] {
                let _ = render(
                    &mut state,
                    vec![
                        egui::Event::PointerMoved(click),
                        egui::Event::PointerButton {
                            pos: click,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert!(
                state.scroll_to_offset.is_some(),
                "minimap click should request navigation"
            );
            for _ in 0..5 {
                let frame = render(&mut state, Vec::new());
                assert!(
                    (state.minimap_content_height - state.code_rows.total_height()).abs() < 1.0,
                    "scroll extent {} must match document height {} (wrap={wrap})",
                    state.minimap_content_height,
                    state.code_rows.total_height()
                );
                assert!(
                    (state.code_scroll_offset
                        - (state.code_rows.total_height() - state.minimap_viewport_height)
                            .max(0.0))
                    .abs()
                        < 1.0,
                    "clicking at the bottom should reach the final viewport"
                );
                assert!(frame.shapes.iter().any(|shape| matches!(
                    &shape.shape, egui::Shape::Text(text) if text.galley.job.text.contains("LINE_099")
                )), "the final source line should remain visible after a bottom minimap click");
                assert!(
                    state.code_scroll_offset
                        <= (state.code_rows.total_height() - state.minimap_viewport_height)
                            .max(0.0)
                            + 1.0
                );
            }
        }
    }

    #[test]
    fn minimap_wrapped_line_marker_and_click_use_virtual_row_geometry() {
        let target_line = 24;
        let mut first_fragment = Vec::new();
        let mut rows = CodeRowHeights::default();
        let heights: Vec<f32> = (0..80)
            .flat_map(|line| {
                first_fragment.push(if line == 0 {
                    0
                } else {
                    first_fragment[line - 1] + if line - 1 == target_line { 8 } else { 1 }
                });
                std::iter::repeat_n(20.0, if line == target_line { 8 } else { 1 })
            })
            .collect();
        rows.reset(heights.len(), 500.0, 20.0, 1.0);
        for (index, height) in heights.iter().enumerate() {
            rows.set_height(index, *height);
        }
        let lines: Vec<_> = (0..80)
            .map(|line| {
                let color = if line == target_line {
                    syntect::highlighting::Color {
                        r: 255,
                        g: 0,
                        b: 0,
                        a: 255,
                    }
                } else {
                    syntect::highlighting::Color {
                        r: 0,
                        g: 0,
                        b: 255,
                        a: 255,
                    }
                };
                (
                    line,
                    MinimapLine {
                        segments: vec![MinimapSegment {
                            start_column: 0,
                            end_column: MINIMAP_COLUMNS,
                            color,
                        }],
                    },
                )
            })
            .collect();
        let palette = AppPalette::default();
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut clicked_fraction = None;
        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 700.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        clicked_fraction = render_code_minimap(
                            ui,
                            &lines,
                            &first_fragment,
                            &rows,
                            640.0,
                            0.0,
                            0.2,
                            &palette,
                        );
                    });
                },
            )
        };
        let frame = render(Vec::new());
        let marker_color =
            map_syntect_color_to_theme(lines[target_line].1.segments[0].color, &palette);
        let marker = frame
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == marker_color => Some(rect.rect),
                _ => None,
            })
            .expect("wrapped source line marker should be painted");
        let map_rect = frame
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.fill == hex_to_c32(&palette.colors.surface_panel)
                        && (rect.rect.width() - MINIMAP_WIDTH).abs() < 0.1 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("minimap panel should be painted");
        let content = map_rect.shrink2(Vec2::new(4.0, 3.0));
        let expected_top = content.top()
            + content.height() * rows.offset(first_fragment[target_line]) / rows.total_height();
        let expected_bottom = content.top()
            + content.height() * rows.offset(first_fragment[target_line + 1]) / rows.total_height();
        assert!(
            (marker.top() - expected_top).abs() < 1.0
                && (marker.bottom() - expected_bottom).abs() < 1.0
        );
        let click = marker.center();
        render(vec![
            egui::Event::PointerMoved(click),
            egui::Event::PointerButton {
                pos: click,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        render(vec![egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let fraction =
            clicked_fraction.expect("marker click should yield a normalized scroll target");
        let marker_fraction = (click.y - content.top()) / content.height();
        assert!((fraction - marker_fraction).abs() < 0.01);
        let viewport_height = rows.total_height() * 0.2;
        let requested = (fraction * rows.total_height() - viewport_height * 0.5)
            .clamp(0.0, rows.total_height() - viewport_height);
        assert!(
            requested < rows.offset(first_fragment[target_line + 1])
                && requested + viewport_height > rows.offset(first_fragment[target_line]),
            "centered synthetic viewport should overlap the wrapped source-line span"
        );
    }

    #[test]
    fn unwrapped_raw_line_scrolls_horizontally_without_eliding_its_end() {
        let ctx = egui::Context::default();
        let content = format!("LEFT_EDGE{}RIGHT_EDGE", "M".repeat(600));
        let mut state = TextPreviewState::from_text(content, None, &AppConfig::default());
        state.wrap_lines = false;
        let mut time = 0.0;

        let mut render = |events: Vec<egui::Event>| {
            time += 0.05;
            ctx.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(540.0, 360.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_raw_code_view(ui, &AppPalette::default());
                    });
                },
            )
        };

        let first = render(Vec::new());
        let text_shape = |output: &egui::FullOutput| {
            output.shapes.iter().find_map(|clipped| {
                let egui::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                text.galley.text().starts_with("LEFT_EDGE").then_some((
                    clipped.clip_rect,
                    text.pos.x,
                    text.galley.size().x,
                    text.galley.elided,
                ))
            })
        };
        let (clip, first_x, first_width, first_elided) =
            text_shape(&first).expect("initial code row should render");
        assert!(first_width > clip.width() * 2.0);
        assert!(!first_elided);

        let scrolled = render(vec![
            egui::Event::PointerMoved(clip.center()),
            egui::Event::Scroll(egui::vec2(-8_000.0, 0.0)),
        ]);
        let mut settled = scrolled;
        for _ in 0..8 {
            settled = render(Vec::new());
        }
        let (scrolled_clip, scrolled_x, _, scrolled_elided) =
            text_shape(&settled).expect("code row should remain visible after horizontal scroll");
        assert!(scrolled_clip.width() <= clip.width());
        assert!(
            scrolled_x < first_x - 20.0,
            "horizontal wheel should move code left: {} -> {}",
            first_x,
            scrolled_x
        );
        assert!(!scrolled_elided);
        let scrolled_right = scrolled_x + first_width;
        assert!(
            scrolled_right <= scrolled_clip.right() + 1.0 && scrolled_right >= scrolled_clip.left(),
            "horizontal scrolling should reach the line end: right edge {scrolled_right}, viewport {:?}",
            scrolled_clip
        );
    }

    #[test]
    fn wrapped_view_places_next_line_below_the_entire_galley() {
        for scale in [1.0, 1.5, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(scale);
            let _ = ctx.run(egui::RawInput::default(), |_| {});
            let mut state = TextPreviewState::from_text(
                format!("{}\nNEXT_ROW\n{}", "word ".repeat(30), "tail\n".repeat(60)),
                None,
                &AppConfig::default(),
            );
            let render_frame = |state: &mut TextPreviewState| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(480.0, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            state.render_raw_code_view(ui, &AppPalette::default())
                        });
                    },
                )
            };
            let output = render_frame(&mut state);
            assert!(
                state.code_rows.heights[0] > state.code_rows.heights[1] * 2.0,
                "key {:?}, heights {:?}",
                state.code_rows.key,
                &state.code_rows.heights[..3]
            );
            let text_shapes: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape {
                        Some(text)
                    } else {
                        None
                    }
                })
                .collect();
            let first = text_shapes
                .iter()
                .find(|shape| shape.galley.text().starts_with("word "))
                .unwrap();
            let next = text_shapes
                .iter()
                .find(|shape| shape.galley.text() == "NEXT_ROW")
                .unwrap();
            assert!(
                next.pos.y >= first.pos.y + first.galley.size().y,
                "wrapped rows overlap at scale {scale}"
            );
            state.scroll_to_line = Some(2);
            let _ = render_frame(&mut state);
            assert!(
                (state.code_scroll_offset - state.code_rows.offset(1)).abs() < 1.0,
                "search uses wrapped source offset"
            );
            state.wrap_lines = false;
            state.scroll_to_line = Some(1);
            let _ = render_frame(&mut state);
            assert!((state.code_rows.heights[0] - state.code_rows.heights[1]).abs() < 1.0);

            // Navigate past the background measurement cursor, then measure more
            // preceding wrapped lines. The same source row must stay at the top.
            state = TextPreviewState::from_text(
                format!("{}\n", "word ".repeat(30)).repeat(1024),
                None,
                &AppConfig::default(),
            );
            let _ = render_frame(&mut state);
            let _ = render_frame(&mut state);
            state.scroll_to_line = Some(801);
            let _ = render_frame(&mut state);
            for _ in 0..6 {
                let _ = render_frame(&mut state);
                assert!(
                    (state.code_scroll_offset - state.code_rows.offset(800)).abs() < 1.0,
                    "incremental measurements moved the source anchor at scale {scale}: actual {} expected {} row {}",
                    state.code_scroll_offset,
                    state.code_rows.offset(800),
                    state.code_rows.row_at(state.code_scroll_offset)
                );
            }
        }
    }

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
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 2);
        assert_eq!(state.active_match_idx, 0);

        state.next_match();
        assert_eq!(state.active_match_idx, 1);

        state.next_match();
        assert_eq!(state.active_match_idx, 0);

        state.prev_match();
        assert_eq!(state.active_match_idx, 1);
    }

    #[test]
    fn text_preview_strips_only_a_leading_bom() {
        let config = AppConfig::default();
        let plain = "clipboard content".to_owned();
        let state = TextPreviewState::from_text(plain.clone(), None, &config);
        assert_eq!(state.content.as_str(), plain);

        let bom_prefixed = format!("\u{feff}{}", plain);
        let state = TextPreviewState::from_text(bom_prefixed, None, &config);
        assert_eq!(state.content.as_str(), plain);
    }

    #[test]
    fn find_editor_enter_navigates_once_without_closing_the_preview() {
        let ctx = egui::Context::default();
        let mut state = TextPreviewState::from_text(
            "match one\nmatch two\nmatch three".to_owned(),
            None,
            &AppConfig::default(),
        );
        state.show_find = true;
        state.find_query = "match".to_owned();
        update_search_matches_and_wait(&mut state);
        state.active_match_idx = 0;

        let render = |ctx: &egui::Context, state: &mut TextPreviewState, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_search_toolbar(ui, &AppPalette::default());
                    });
                },
            )
        };

        state.focus_find_input = true;
        render(&ctx, &mut state, Vec::new());
        assert!(
            ctx.wants_keyboard_input(),
            "Find editor should own keyboard focus"
        );

        render(
            &ctx,
            &mut state,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(state.active_match_idx, 1, "Enter advances one match");
        assert!(state.show_find, "Enter must leave the Find toolbar open");

        render(&ctx, &mut state, Vec::new());
        assert_eq!(
            state.active_match_idx, 1,
            "one Enter must not advance twice"
        );
    }

    #[test]
    fn find_editor_shift_enter_navigates_to_previous_match_once() {
        let ctx = egui::Context::default();
        let mut state = TextPreviewState::from_text(
            "match one\nmatch two\nmatch three".to_owned(),
            None,
            &AppConfig::default(),
        );
        state.show_find = true;
        state.find_query = "match".to_owned();
        update_search_matches_and_wait(&mut state);
        state.active_match_idx = 1;
        state.focus_find_input = true;

        let mut render = |events, modifiers| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 400.0),
                    )),
                    events,
                    modifiers,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        state.render_search_toolbar(ui, &AppPalette::default());
                    });
                },
            )
        };

        render(Vec::new(), egui::Modifiers::NONE);
        render(
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
            egui::Modifiers::SHIFT,
        );
        assert_eq!(
            state.active_match_idx, 0,
            "Shift+Enter moves to the prior match"
        );
        assert!(
            state.show_find,
            "Shift+Enter must leave the Find toolbar open"
        );
    }

    #[test]
    fn test_text_preview_regex_search() {
        let config = AppConfig::default();
        let content = "const VALUE_A = 100;\nconst VALUE_B = 200;\nconst OTHER = 300;".to_string();
        let mut state = TextPreviewState::from_text(content, None, &config);

        state.find_query = r"VALUE_[A-Z]".to_string();
        state.is_regex = true;
        update_search_matches_and_wait(&mut state);

        assert!(state.regex_error.is_none());
        assert_eq!(state.match_count(), 2);

        // Invalid regex
        state.find_query = r"[invalid".to_string();
        update_search_matches_and_wait(&mut state);
        assert!(state.regex_error.is_some());
        assert_eq!(state.match_count(), 0);
    }

    #[test]
    fn literal_search_preserves_unicode_offsets_and_escapes_metacharacters() {
        let mut state =
            TextPreviewState::from_text("İX\nCAFÉ café [x]".into(), None, &AppConfig::default());
        state.find_query = "x".into();
        update_search_matches_and_wait(&mut state);
        let matched = state.search_match(0).unwrap();
        assert_eq!(&state.content[matched.start_byte..matched.end_byte], "X");
        assert_eq!(matched.start_byte, "İ".len());
        state.find_query = "CAFÉ".into();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 2);
        for index in 0..state.match_count() {
            let matched = state.search_match(index).unwrap();
            assert!(state.content.is_char_boundary(matched.start_byte));
            assert!(state.content.is_char_boundary(matched.end_byte));
            assert_eq!(matched.line, 2);
        }
        state.find_query = "[x]".into();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 1);
        let matched = state.search_match(0).unwrap();
        assert_eq!(&state.content[matched.start_byte..matched.end_byte], "[x]");
    }

    #[test]
    fn search_line_numbers_derive_from_first_fragments_for_empty_and_trailing_lines() {
        let mut empty = TextPreviewState::from_text(String::new(), None, &AppConfig::default());
        empty.find_query = "(?m)^".into();
        empty.is_regex = true;
        update_search_matches_and_wait(&mut empty);
        assert_eq!(empty.line_first_fragment.len(), 1);
        assert_eq!(empty.search_match(0).unwrap().line, 1);

        let mut trailing =
            TextPreviewState::from_text("alpha\nbeta\n".into(), None, &AppConfig::default());
        trailing.find_query = "(?m)^".into();
        trailing.is_regex = true;
        update_search_matches_and_wait(&mut trailing);
        assert_eq!(trailing.line_first_fragment.len(), 3);
        assert_eq!(
            trailing.search_match(2).unwrap().start_byte,
            trailing.content.len()
        );
        assert_eq!(trailing.search_match(2).unwrap().line, 3);
    }

    #[test]
    fn highlights_project_active_and_multiline_ranges_to_source_lines() {
        let matches = vec![
            SearchMatch {
                start_byte: 1,
                end_byte: 3,
                line: 1,
            },
            SearchMatch {
                start_byte: 6,
                end_byte: 12,
                line: 2,
            },
        ];
        assert_eq!(
            line_highlight_ranges(&matches, 1, 5, 10),
            vec![(1, 5, true)]
        );
        assert_eq!(
            line_highlight_ranges(&matches, 1, 11, 15),
            vec![(0, 1, true)]
        );
        assert!(line_highlight_ranges(&matches, 1, 15, 20).is_empty());
    }

    #[test]
    fn search_background_covers_only_the_matched_part_of_a_token() {
        let palette = AppPalette::default();
        let mut job = egui::text::LayoutJob::default();
        append_highlighted_token(
            &mut job,
            "identifier",
            0,
            Color32::WHITE,
            &palette,
            &[(2, 5, true)],
        );
        assert_eq!(job.text, "identifier");
        let highlighted: Vec<_> = job
            .sections
            .iter()
            .filter(|section| section.format.background != Color32::TRANSPARENT)
            .collect();
        assert_eq!(highlighted.len(), 1);
        assert_eq!(highlighted[0].byte_range, 2..5);
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
    fn python_minimap_ends_comment_scope_at_each_source_newline() {
        let text = Arc::new(
            "# module comment\nimport os\ndef greet(name):\n    return \"hello\"\n".to_owned(),
        );
        let syntax = find_syntax("py");
        let MinimapResult::Lines(lines) =
            build_minimap_lines(&Arc::downgrade(&text), syntax, &AtomicBool::new(false))
        else {
            panic!("Python minimap should complete")
        };
        let mut highlighter =
            HighlightLines::new(syntax, &THEME_SET.themes["base16-eighties.dark"]);
        for ((_, actual), source) in lines.iter().zip(text.split_inclusive('\n')) {
            let expected = build_minimap_line(source, &mut highlighter);
            assert_eq!(actual.segments.len(), expected.segments.len());
            for (actual, expected) in actual.segments.iter().zip(&expected.segments) {
                assert_eq!(actual.color, expected.color, "syntax colors for {source:?}");
            }
        }
        assert!(
            lines[1]
                .1
                .segments
                .iter()
                .any(|segment| segment.color != lines[0].1.segments[0].color)
        );
    }

    #[test]
    fn rust_minimap_handles_unicode_before_line_ending_whitespace() {
        let text = Arc::new("//界\n/*\n界\n*/\nfn main() {}\n".to_owned());
        let MinimapResult::Lines(lines) = build_minimap_lines(
            &Arc::downgrade(&text),
            find_syntax("rs"),
            &AtomicBool::new(false),
        ) else {
            panic!("Unicode Rust minimap should complete")
        };
        for index in [0, 1, 2, 3, 4] {
            assert!(
                !lines[index].1.segments.is_empty(),
                "source line {index} needs visible blocks"
            );
        }
        assert_eq!(lines[2].1.segments[0].start_column, 0);
        assert_eq!(lines[2].1.segments[0].end_column, 1);

        // A worker panic disconnects the receiver and leaves only the scrollbar.
        // Verify that this source reaches the published cache as well.
        let jobs = BackgroundJobs::new(1, 1);
        let mut cache = MinimapCache::default();
        cache.start(Arc::clone(&text), "rs".to_owned(), &jobs);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cache.poll() {
            assert!(
                Instant::now() < deadline,
                "Unicode minimap worker did not finish"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(cache.ready);
        assert!(!cache.failed);
        assert_eq!(cache.lines[2].1.segments, lines[2].1.segments);
    }

    #[test]
    fn test_minimap_cache_uses_syntax_colored_blocks() {
        let text = Arc::new("fn main() {\n    let answer = 42;\n}\n".to_owned());
        let lines = build_minimap_lines(
            &Arc::downgrade(&text),
            find_syntax("rs"),
            &AtomicBool::new(false),
        );
        let MinimapResult::Lines(lines) = lines else {
            panic!("small source should build minimap lines")
        };

        assert_eq!(lines.len(), 4, "one minimap row per source line");
        assert!(
            lines
                .iter()
                .take(3)
                .all(|(_, line)| !line.segments.is_empty())
        );
        assert!(
            lines
                .iter()
                .flat_map(|(_, line)| &line.segments)
                .all(|segment| {
                    segment.start_column < segment.end_column
                        && segment.end_column <= MINIMAP_COLUMNS
                })
        );
    }

    #[test]
    fn test_minimap_builder_cancellation_returns_no_partial_samples() {
        let text = Arc::new("fn main() {}\n".repeat(10_000));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            build_minimap_lines(&Arc::downgrade(&text), find_syntax("rs"), &cancelled),
            MinimapResult::Cancelled
        ));
    }

    #[test]
    fn test_minimap_builder_preserves_complete_long_lines() {
        let text = Arc::new(format!("{}\nlet after = 1;", "x".repeat(300_000)));
        let MinimapResult::Lines(lines) = build_minimap_lines(
            &Arc::downgrade(&text),
            find_syntax("rs"),
            &AtomicBool::new(false),
        ) else {
            panic!("long logical lines remain supported")
        };
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].0, 0);
        assert_eq!(lines[1].0, 1);
    }

    #[test]
    fn minimap_syntax_limit_is_inclusive_and_counts_utf8_bytes() {
        let at_limit = "x".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES);
        assert_eq!(minimap_source_shape(&at_limit, || false), Some((1, false)));
        let exact_source = Arc::new(at_limit.clone());
        let MinimapResult::Lines(exact_lines) = build_minimap_lines(
            &Arc::downgrade(&exact_source),
            find_syntax("txt"),
            &AtomicBool::new(false),
        ) else {
            panic!("the inclusive boundary should retain syntax minimap output")
        };
        assert_ne!(exact_lines[0].1.segments[0].color, MINIMAP_NEUTRAL_COLOR);
        assert_eq!(
            minimap_source_shape(&(at_limit.clone() + "x"), || false),
            Some((1, true))
        );
        assert_eq!(
            minimap_source_shape(&(at_limit + "\nafter"), || false),
            Some((2, false))
        );
        let multibyte = "界".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES / 3);
        assert_eq!(minimap_source_shape(&multibyte, || false), Some((1, false)));
        assert_eq!(
            minimap_source_shape(&(multibyte + "界"), || false),
            Some((1, true))
        );
    }

    #[test]
    fn minimap_shape_scan_checks_cancellation_within_64_kib() {
        let text = "x".repeat(3 * MINIMAP_CANCELLATION_CHECK_BYTES);
        let mut checks = 0;
        assert_eq!(
            minimap_source_shape(&text, || {
                checks += 1;
                checks == 3
            }),
            None
        );
        assert_eq!(checks, 3);
    }

    #[test]
    fn oversized_line_makes_the_entire_minimap_neutral_and_keeps_later_rows() {
        let text = Arc::new(format!(
            "  let before = 1;\n/*{}\nlet after = 2;\n",
            "x".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES)
        ));
        let MinimapResult::Lines(lines) = build_minimap_lines(
            &Arc::downgrade(&text),
            find_syntax("rs"),
            &AtomicBool::new(false),
        ) else {
            panic!("oversized source should publish a neutral minimap")
        };
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
            (0..4).collect::<Vec<_>>()
        );
        assert!(lines[3].1.segments.is_empty());
        for (_, line) in lines.iter().take(3) {
            assert_eq!(line.segments.len(), 1);
            let segment = &line.segments[0];
            assert_eq!(segment.color, MINIMAP_NEUTRAL_COLOR);
            assert!(segment.start_column < segment.end_column);
            assert!(segment.end_column <= MINIMAP_COLUMNS);
        }
        assert_eq!(lines[0].1.segments[0].start_column, 2);
        assert_eq!(lines[2].1.segments[0].start_column, 0);

        let mut state = TextPreviewState::from_text((*text).clone(), None, &AppConfig::default());
        assert_eq!(state.content.as_str(), text.as_str());
        state.find_query = "let after = 2".to_owned();
        update_search_matches_and_wait(&mut state);
        assert_eq!(state.match_count(), 1);
        let matched = state.search_match(0).unwrap();
        assert_eq!(
            &state.content[matched.start_byte..matched.end_byte],
            "let after = 2"
        );
    }

    #[test]
    fn neutral_minimap_retains_bounded_source_position_samples_and_cancels() {
        let text = Arc::new(format!(
            "{}\n{}",
            "x".repeat(MINIMAP_MAX_SYNTAX_LINE_BYTES + 1),
            "  row\n".repeat(5_000)
        ));
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            build_minimap_lines(&Arc::downgrade(&text), find_syntax("rs"), &cancelled),
            MinimapResult::Cancelled
        ));
        let MinimapResult::Lines(lines) = build_minimap_lines(
            &Arc::downgrade(&text),
            find_syntax("rs"),
            &AtomicBool::new(false),
        ) else {
            panic!("neutral minimap should complete")
        };
        let source_lines: usize = 5_002;
        let stride = source_lines.div_ceil(MINIMAP_RETAINED_LINES);
        assert_eq!(lines.len(), source_lines.div_ceil(stride));
        assert!(lines.len() <= MINIMAP_RETAINED_LINES);
        assert!(
            lines
                .iter()
                .enumerate()
                .all(|(sample, (row, line))| *row == sample * stride && line.segments.len() <= 1)
        );
        assert_eq!(lines.last().unwrap().0, 5_000);
    }

    #[test]
    fn test_minimap_parses_unsampled_lines_to_preserve_syntax_state() {
        let mut text = String::from("let first = 1;\n/*\ncomment body\n*/\n");
        text.push_str(&"let filler = 0;\n".repeat(MINIMAP_RETAINED_LINES));
        let text = Arc::new(text);
        let MinimapResult::Lines(lines) = build_minimap_lines(
            &Arc::downgrade(&text),
            find_syntax("rs"),
            &AtomicBool::new(false),
        ) else {
            panic!("fixture should complete")
        };
        assert_eq!(
            lines[1].0, 2,
            "line 2 should be sampled while line 1 is not"
        );

        let mut expected_highlighter =
            HighlightLines::new(find_syntax("rs"), &THEME_SET.themes["base16-eighties.dark"]);
        let _ = expected_highlighter.highlight_line("let first = 1;", &SYNTAX_SET);
        let _ = expected_highlighter.highlight_line("/*", &SYNTAX_SET);
        let expected = build_minimap_line("comment body", &mut expected_highlighter);
        assert_eq!(
            lines[1].1.segments, expected.segments,
            "the unsampled block-comment opener must update syntect state"
        );
    }

    #[test]
    fn test_dropping_queued_minimap_releases_source_before_worker_runs() {
        let jobs = BackgroundJobs::new(1, 2);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_blocker_ticket, _blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let source = Arc::new("let queued = true;\n".repeat(100));
        let weak_source = Arc::downgrade(&source);
        let mut cache = MinimapCache::default();
        cache.start(Arc::clone(&source), "rs".to_owned(), &jobs);
        drop(source);
        drop(cache);
        assert!(
            weak_source.upgrade().is_none(),
            "queued closure must retain only Weak"
        );

        release_tx.send(()).unwrap();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let (_ticket, _receiver) = jobs.submit(std::time::Duration::MAX, move || {
            finished_tx.send(()).unwrap();
        });
        finished_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
    }

    #[test]
    fn test_minimap_worker_publishes_bounded_samples_without_changing_navigation() {
        let text = "let x = 1;\n".repeat(MINIMAP_RETAINED_LINES * 2 + 1);
        let mut state = TextPreviewState::from_text(text, None, &AppConfig::default());
        let byte = state.content.len() / 2;
        let expected_row = state.display_row_for_byte(byte);
        let mut cache = MinimapCache::default();
        let jobs = BackgroundJobs::new(1, 1);
        cache.start(Arc::clone(&state.content), "rs".to_owned(), &jobs);
        while !cache.poll() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!cache.failed);
        assert!(cache.lines.len() <= MINIMAP_RETAINED_LINES);
        assert_eq!(cache.lines.first().map(|(line, _)| *line), Some(0));
        assert!(cache.lines.windows(2).all(|pair| pair[0].0 < pair[1].0));
        state.minimap_lines = cache;
        assert_eq!(state.display_row_for_byte(byte), expected_row);
    }

    #[test]
    fn test_minimap_job_cancellation_prevents_stale_result_publication() {
        let jobs = BackgroundJobs::new(1, 1);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_blocker_ticket, _blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let mut cache = MinimapCache::default();
        cache.start(Arc::new("fn main() {}".to_owned()), "rs".to_owned(), &jobs);
        cache.cancel();
        release_tx.send(()).unwrap();
        while !cache.poll() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(cache.failed);
        assert!(cache.lines.is_empty());
    }

    #[test]
    fn test_minimap_job_retries_after_background_queue_saturation() {
        let jobs = BackgroundJobs::new(1, 1);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (_blocker_ticket, _blocker_rx) = jobs.submit(std::time::Duration::MAX, move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        let (_queued_ticket, queued_receiver) = jobs.submit(std::time::Duration::MAX, || ());

        let mut cache = MinimapCache::default();
        let text = Arc::new("fn main() {}".to_owned());
        cache.start(Arc::clone(&text), "rs".to_owned(), &jobs);
        assert!(!cache.poll(), "a full queue should remain retryable");
        assert!(!cache.started);

        release_tx.send(()).unwrap();
        queued_receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .unwrap();
        cache.start(Arc::clone(&text), "rs".to_owned(), &jobs);
        while !cache.poll() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert!(cache.ready);
        assert!(!cache.failed);
        assert!(!cache.lines.is_empty());
    }

    #[test]
    #[ignore = "headless CPU benchmark; run explicitly with --release and --nocapture"]
    fn benchmark_large_text_preview_frames_across_width_dpi_appearance() {
        use little_clip_core::config::WorkbenchAppearance;

        const LOGICAL_LINES: usize = 12_000;
        const TIMED_FRAMES: usize = 60;
        let mut fixture = String::new();
        for line in 0..LOGICAL_LINES {
            if line % 257 == 0 {
                fixture.push_str(&format!(
                    "fn row_{line}() {{ let value = {line}; let padding = \"{}\"; }}\n",
                    "x".repeat(8 * 1024)
                ));
            } else {
                fixture.push_str(&format!("fn row_{line}() {{ let value = {line}; }}\n"));
            }
        }
        let fixture_bytes = fixture.len();

        let cases = [
            (egui::vec2(640.0, 520.0), 1.0, WorkbenchAppearance::Light),
            (egui::vec2(640.0, 520.0), 1.5, WorkbenchAppearance::Light),
            (egui::vec2(640.0, 520.0), 1.0, WorkbenchAppearance::Dark),
            (egui::vec2(640.0, 520.0), 1.5, WorkbenchAppearance::Dark),
            (egui::vec2(1180.0, 720.0), 1.0, WorkbenchAppearance::Light),
            (egui::vec2(1180.0, 720.0), 1.5, WorkbenchAppearance::Light),
            (egui::vec2(1180.0, 720.0), 1.0, WorkbenchAppearance::Dark),
            (egui::vec2(1180.0, 720.0), 1.5, WorkbenchAppearance::Dark),
        ];
        let render_frame = |state: &mut TextPreviewState,
                            ctx: &egui::Context,
                            palette: &AppPalette,
                            time: &mut f64,
                            size: Vec2,
                            pixels_per_point: f32,
                            events: Vec<egui::Event>| {
            *time += 1.0 / 60.0;
            let started = std::time::Instant::now();
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                time: Some(*time),
                events,
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&input.viewport_id)
                .expect("root viewport exists")
                .native_pixels_per_point = Some(pixels_per_point);
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut close = false;
                    state.render_ui(ui, ctx, palette, &mut close);
                });
            });
            let shape_count = output.shapes.len();
            (started.elapsed().as_secs_f64() * 1000.0, shape_count)
        };
        let summarize = |values: &mut Vec<f64>| {
            values.sort_by(f64::total_cmp);
            let at = |fraction: f64| {
                let index = ((values.len() - 1) as f64 * fraction).ceil() as usize;
                values[index]
            };
            (at(0.50), at(0.95), values[values.len() - 1])
        };

        for (size, pixels_per_point, appearance) in cases {
            let construction_started = std::time::Instant::now();
            let mut state =
                TextPreviewState::from_text(fixture.clone(), None, &AppConfig::default());
            let construction_ms = construction_started.elapsed().as_secs_f64() * 1000.0;

            // Provide deterministic, ready minimap samples so timed production
            // frames include minimap painting/index lookups but exclude worker
            // scheduling and syntax preparation.
            let source_lines = state.line_first_fragment.len();
            let minimap_stride = source_lines.div_ceil(MINIMAP_RETAINED_LINES).max(1);
            state.minimap_lines.lines = (0..source_lines)
                .step_by(minimap_stride)
                .map(|source_line| {
                    (
                        source_line,
                        MinimapLine {
                            segments: vec![MinimapSegment {
                                start_column: 0,
                                end_column: MINIMAP_COLUMNS,
                                color: syntect::highlighting::Color {
                                    r: 128,
                                    g: 160,
                                    b: 192,
                                    a: 255,
                                },
                            }],
                        },
                    )
                })
                .collect();
            state.minimap_lines.started = true;
            state.minimap_lines.ready = true;
            state.minimap_overflows = true;

            let palette = crate::ui_appearance::palette(appearance);
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            crate::ui_appearance::apply_visuals(&ctx, &palette);
            let mut time = 0.0;
            let (cold_ms, cold_shapes) = render_frame(
                &mut state,
                &ctx,
                &palette,
                &mut time,
                size,
                pixels_per_point,
                Vec::new(),
            );

            let warm_frame_limit = state.code_fragments.len().div_ceil(8) + 32;
            let mut warm_frames = 0;
            while state.code_rows.next_measurement < state.code_fragments.len()
                && warm_frames < warm_frame_limit
            {
                let _ = render_frame(
                    &mut state,
                    &ctx,
                    &palette,
                    &mut time,
                    size,
                    pixels_per_point,
                    Vec::new(),
                );
                warm_frames += 1;
            }
            assert_eq!(
                state.code_rows.next_measurement,
                state.code_fragments.len(),
                "geometry warm-up must finish before steady-state samples"
            );

            let mut idle_ms = Vec::with_capacity(TIMED_FRAMES);
            let mut idle_shapes = 0;
            for _ in 0..TIMED_FRAMES {
                let (elapsed, shapes) = render_frame(
                    &mut state,
                    &ctx,
                    &palette,
                    &mut time,
                    size,
                    pixels_per_point,
                    Vec::new(),
                );
                idle_ms.push(elapsed);
                idle_shapes = shapes;
            }
            let idle = summarize(&mut idle_ms);

            let pointer = egui::pos2(size.x * 0.45, size.y * 0.5);
            let mut scroll_ms = Vec::with_capacity(TIMED_FRAMES);
            let mut scroll_shapes = 0;
            for frame in 0..TIMED_FRAMES {
                let delta = if frame % 2 == 0 { -240.0 } else { 240.0 };
                let (elapsed, shapes) = render_frame(
                    &mut state,
                    &ctx,
                    &palette,
                    &mut time,
                    size,
                    pixels_per_point,
                    vec![
                        egui::Event::PointerMoved(pointer),
                        egui::Event::Scroll(egui::vec2(0.0, delta)),
                    ],
                );
                scroll_ms.push(elapsed);
                scroll_shapes = shapes;
            }
            let scroll = summarize(&mut scroll_ms);

            let reflow_size = egui::vec2(size.x - 80.0, size.y);
            let reflow_pixels_per_point = if pixels_per_point == 1.0 { 1.5 } else { 1.0 };
            let (reflow_first_ms, _) = render_frame(
                &mut state,
                &ctx,
                &palette,
                &mut time,
                reflow_size,
                reflow_pixels_per_point,
                Vec::new(),
            );
            let settle_started = std::time::Instant::now();
            let mut reflow_frames = 1;
            while state.code_rows.next_measurement < state.code_fragments.len()
                && reflow_frames < warm_frame_limit
            {
                let _ = render_frame(
                    &mut state,
                    &ctx,
                    &palette,
                    &mut time,
                    reflow_size,
                    reflow_pixels_per_point,
                    Vec::new(),
                );
                reflow_frames += 1;
            }
            assert_eq!(state.code_rows.next_measurement, state.code_fragments.len());
            let reflow_settle_ms = settle_started.elapsed().as_secs_f64() * 1000.0;

            println!(
                "text-preview appearance={appearance:?} host={}x{} ppp={pixels_per_point:.1} source_bytes={fixture_bytes} fragments={} samples={} construct_ms={construction_ms:.3} cold_ms={cold_ms:.3} cold_shapes={cold_shapes} warm_frames={warm_frames} idle_ms[p50/p95/max]={:.3}/{:.3}/{:.3} idle_shapes={idle_shapes} scroll_ms[p50/p95/max]={:.3}/{:.3}/{:.3} scroll_shapes={scroll_shapes} reflow_first_ms={reflow_first_ms:.3} reflow_settle_frames={reflow_frames} reflow_settle_ms={reflow_settle_ms:.3}",
                size.x as usize,
                size.y as usize,
                state.code_fragments.len(),
                state.minimap_lines.lines.len(),
                idle.0,
                idle.1,
                idle.2,
                scroll.0,
                scroll.1,
                scroll.2,
            );
        }
    }

    #[test]
    fn test_gutter_text_alignment() {
        let ctx = egui::Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    let gutter_resp = ui.label(
                        egui::RichText::new("1\n2\n3")
                            .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE)),
                    );
                    let mut text = "#include <stdio.h>\nint main() {}\n".to_string();
                    let syntax = find_syntax("c");
                    let palette = AppPalette::default();
                    let mut layouter = |ui: &egui::Ui, text: &str, _wrap_width: f32| {
                        let layout_job = build_highlighted_layout_job(text, syntax, &palette, &[]);
                        ui.fonts(|f| f.layout_job(layout_job))
                    };
                    let edit = egui::TextEdit::multiline(&mut text)
                        .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
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
