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
use std::sync::{Arc, LazyLock, mpsc::Receiver};
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

#[derive(Debug, Clone, Copy)]
struct CodeFragment {
    start_byte: usize,
    end_byte: usize,
    source_line: usize,
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
                source_line,
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
                source_line,
            });
            fragment_start = fragment_end;
        }
    }
    (fragments, line_first_fragment)
}

/// Cumulative row heights, updated in logarithmic time as wrapping is measured.
/// Unmeasured rows start at one font row; the viewport retains its source anchor
/// while those estimates settle. Only heights are retained, not document galleys.
#[derive(Default)]
struct CodeRowHeights {
    key: Option<(f32, f32, f32)>,
    heights: Vec<f32>,
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
        self.sums = (0..=count)
            .map(|index| (index & index.wrapping_neg()) as f64 * height as f64)
            .collect();
        self.next_measurement = 0;
    }

    fn set_height(&mut self, row: usize, height: f32) {
        let delta = height as f64 - self.heights[row] as f64;
        self.heights[row] = height;
        let mut index = row + 1;
        while index < self.sums.len() {
            self.sums[index] += delta;
            index += index & index.wrapping_neg();
        }
    }

    fn offset(&self, row: usize) -> f32 {
        let mut index = row.min(self.heights.len());
        let mut sum = 0.0;
        while index > 0 {
            sum += self.sums[index];
            index &= index - 1;
        }
        sum as f32
    }

    fn total_height(&self) -> f32 {
        self.offset(self.heights.len())
    }

    fn row_at(&self, offset: f32) -> usize {
        let mut row = 0;
        let mut sum = 0.0;
        let mut step = self.heights.len().checked_next_power_of_two().unwrap_or(0);
        while step > 0 {
            let next = row + step;
            if next < self.sums.len() && sum + self.sums[next] <= offset as f64 {
                row = next;
                sum += self.sums[next];
            }
            step >>= 1;
        }
        row.min(self.heights.len().saturating_sub(1))
    }
}

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

type MinimapResult = Result<Vec<(usize, MinimapLine)>, JobError>;

#[derive(Default)]
struct MinimapCache {
    lines: Vec<(usize, MinimapLine)>,
    job: Option<(JobTicket, Receiver<MinimapResult>)>,
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
        let (ticket, receiver) = jobs.submit(std::time::Duration::MAX, move || {
            build_minimap_lines(text.as_str(), find_syntax(&syntax_name))
        });
        self.job = Some((ticket, receiver));
    }

    fn poll(&mut self) -> bool {
        let Some((_, receiver)) = &self.job else {
            return false;
        };
        match receiver.try_recv() {
            Ok(Ok(lines)) => {
                self.lines = lines;
                self.job = None;
                self.ready = true;
                true
            }
            Ok(Err(JobError::QueueFull)) => {
                self.job = None;
                self.started = false;
                false
            }
            Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.failed = true;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    fn cancel(&self) {
        if let Some((ticket, _)) = &self.job {
            ticket.cancel();
        }
    }
}

impl Drop for MinimapCache {
    fn drop(&mut self) {
        // BackgroundJobs suppresses queued/stale results; it cannot interrupt
        // a syntect operation that has already started.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchMatchRange {
    start_byte: u64,
    end_byte: u64,
}

/// Stores fixed-size match ranges in memory until the bounded threshold, then spills to cache.
#[allow(dead_code)] // This private storage layer is staged for search-model integration.
struct SearchMatchSpool {
    file: tempfile::SpooledTempFile,
    record_count: u64,
    cursor_at_end: bool,
    last_range: Option<SearchMatchRange>,
}

#[allow(dead_code)] // Keep the standalone abstraction warning-free before UI integration.
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
    type Item = io::Result<SearchMatchRange>;

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
                return Some(Ok(range));
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
    code_fragments: Vec<CodeFragment>,
    line_first_fragment: Vec<usize>,
    code_rows: CodeRowHeights,
    code_scroll_offset: f32,
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

impl TextPreviewState {
    /// Creates a new text preview from in-memory text.
    pub fn from_text(text: String, title: Option<String>, config: &AppConfig) -> Self {
        let text = if text.starts_with('\u{feff}') {
            text[char::len_utf8('\u{feff}')..].to_owned()
        } else {
            text
        };
        let line_starts = line_start_offsets(&text);
        let (code_fragments, line_first_fragment) = code_fragments(&text, &line_starts);
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
            content: Arc::new(text),
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
            code_fragments,
            line_first_fragment,
            code_rows: CodeRowHeights::default(),
            code_scroll_offset: 0.0,
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
        let line_starts = line_start_offsets(&content);
        let (code_fragments, line_first_fragment) = code_fragments(&content, &line_starts);
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
            content: Arc::new(content),
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
            code_fragments,
            line_first_fragment,
            code_rows: CodeRowHeights::default(),
            code_scroll_offset: 0.0,
            minimap_fraction: 0.0,
            minimap_view_fraction: 1.0,
            minimap_content_height: 1.0,
            minimap_viewport_height: 1.0,
            minimap_lines: MinimapCache::default(),
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

        let pattern = if self.is_regex {
            self.find_query.clone()
        } else {
            regex::escape(&self.find_query)
        };
        match RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .build()
        {
            Ok(regex) => {
                self.matches = regex
                    .find_iter(&self.content)
                    .map(|matched| SearchMatch {
                        start_byte: matched.start(),
                        end_byte: matched.end(),
                        line: self
                            .line_starts
                            .partition_point(|start| *start <= matched.start()),
                    })
                    .collect();
            }
            Err(error) => self.regex_error = Some(error.to_string()),
        }
        if self.active_match_idx >= self.matches.len() {
            self.active_match_idx = 0;
        }
    }

    /// Advances to the next search match.
    pub fn next_match(&mut self) {
        if !self.matches.is_empty() {
            self.active_match_idx = (self.active_match_idx + 1) % self.matches.len();
            self.scroll_to_line =
                Some(self.display_row_for_byte(self.matches[self.active_match_idx].start_byte) + 1);
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
            self.scroll_to_line =
                Some(self.display_row_for_byte(self.matches[self.active_match_idx].start_byte) + 1);
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

        EscapeHatchLauncher::launch(editor, editor, &target_path, &mut self.status_toast, now);
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
        } else if self.matches.is_empty() {
            ui.label(
                egui::RichText::new("No matches")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_muted)),
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
                let digits = self.line_starts.len().to_string().len().max(2);
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
                                // Keep selectable-label identities tied to source rows
                                // when virtualization changes the first visible row.
                                ui.skip_ahead_auto_ids(row);
                                let mut y = self.code_rows.offset(row);
                                while row < total_rows && y <= viewport.bottom() {
                                    let (fragment, line) = self.source_fragment(row);
                                    let matches = line_highlight_ranges(
                                        &self.matches,
                                        self.active_match_idx,
                                        fragment.start_byte,
                                        fragment.end_byte,
                                    );
                                    let mut job = build_highlighted_layout_job(
                                        line, syntax, palette, &matches,
                                    );
                                    job.wrap.max_width = wrap_width;
                                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                                    let height = (galley.size().y + 4.0).max(line_height);
                                    self.code_rows.set_height(row, height);
                                    let text_rect = egui::Rect::from_min_size(
                                        origin + egui::vec2(gutter_width, y + 2.0),
                                        galley.size(),
                                    );
                                    if show_gutter
                                        && self.line_first_fragment[fragment.source_line] == row
                                    {
                                        ui.painter().text(
                                            origin + egui::vec2(gutter_width - 10.0, y + 2.0),
                                            egui::Align2::RIGHT_TOP,
                                            format!("{}", fragment.source_line + 1),
                                            code_font.clone(),
                                            hex_to_c32(&palette.colors.text_muted),
                                        );
                                    }
                                    ui.push_id(row, |ui| {
                                        ui.put(text_rect, egui::Label::new(galley));
                                    });
                                    y += height;
                                    row += 1;
                                }
                                ui.set_min_height(self.code_rows.total_height());
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
fn find_syntax(lang: &str) -> &'static SyntaxReference {
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
}

fn build_minimap_lines(text: &str, syntax: &'static SyntaxReference) -> Vec<(usize, MinimapLine)> {
    let line_count = text.split('\n').count();
    let stride = line_count.div_ceil(MINIMAP_RETAINED_LINES).max(1);
    let mut highlighter = HighlightLines::new(syntax, &THEME_SET.themes["base16-eighties.dark"]);
    text.split('\n')
        .enumerate()
        .filter_map(|(source_line, line)| {
            let minimap_line = build_minimap_line(line, &mut highlighter);
            (source_line % stride == 0).then_some((source_line, minimap_line))
        })
        .collect()
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
fn build_highlighted_layout_job(
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

        let ranges = spool
            .ranges_for_fragment(4, 7)
            .unwrap()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
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

        let ranges = spool
            .ranges_for_fragment(4, 9)
            .unwrap()
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
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
                    .collect::<io::Result<Vec<_>>>()
                    .unwrap()
                    .is_empty()
            );
        }
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
                state.matches = matches;
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
        state.update_search_matches();
        assert_eq!(state.matches.len(), 1);
        let matched = state.matches[0];
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
        state.update_search_matches();
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
        state.update_search_matches();
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
    fn literal_search_preserves_unicode_offsets_and_escapes_metacharacters() {
        let mut state =
            TextPreviewState::from_text("İX\nCAFÉ café [x]".into(), None, &AppConfig::default());
        state.find_query = "x".into();
        state.update_search_matches();
        assert_eq!(
            &state.content[state.matches[0].start_byte..state.matches[0].end_byte],
            "X"
        );
        assert_eq!(state.matches[0].start_byte, "İ".len());
        state.find_query = "CAFÉ".into();
        state.update_search_matches();
        assert_eq!(state.matches.len(), 2);
        for matched in &state.matches {
            assert!(state.content.is_char_boundary(matched.start_byte));
            assert!(state.content.is_char_boundary(matched.end_byte));
            assert_eq!(matched.line, 2);
        }
        state.find_query = "[x]".into();
        state.update_search_matches();
        assert_eq!(state.matches.len(), 1);
        assert_eq!(
            &state.content[state.matches[0].start_byte..state.matches[0].end_byte],
            "[x]"
        );
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
    fn test_minimap_cache_uses_syntax_colored_blocks() {
        let text = "fn main() {\n    let answer = 42;\n}\n";
        let lines = build_minimap_lines(text, find_syntax("rs"));

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
        cache.start(text, "rs".to_owned(), &jobs);
        while !cache.poll() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert!(cache.ready);
        assert!(!cache.failed);
        assert!(!cache.lines.is_empty());
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
