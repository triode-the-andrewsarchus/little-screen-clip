//! Binary diffing and alignment engine.
//!
//! Provides Myers-based aligned byte-level diffing (similar to `biodiff`), raw byte diffing,
//! diff block classification (mismatch, deletion, insertion, replacement), statistics calculation,
//! and hex/ASCII search capabilities across diff rows.

use little_clip_core::{ClipItem, ClipPayload, SlotId, SlotManager};
use std::path::Path;
use std::sync::Arc;

mod blocks;
mod rows;
pub use blocks::{BinaryDiffBlockSummary, BinaryDiffBlocks};
pub use rows::BinaryDiffRows;
use std::time::{Duration, Instant};

/// Indicates the comparison classification for an aligned byte cell or diff block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    /// Bytes match on both left and right sides.
    Match,
    /// Bytes differ between left and right sides.
    Mismatch,
    /// Byte exists only on the left side (deletion in right).
    LeftOnly,
    /// Byte exists only on the right side (insertion in right).
    RightOnly,
}

/// Comparison alignment strategy for binary diffing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryDiffMode {
    /// Aligned diffing using Myers diff algorithm on byte slices.
    Aligned,
    /// Simple positional offset matching without alignment gap insertion.
    Raw,
}

impl BinaryDiffMode {
    /// Returns a human-friendly display label for the mode.
    pub fn label(&self) -> &'static str {
        match self {
            BinaryDiffMode::Aligned => "Aligned (Biodiff)",
            BinaryDiffMode::Raw => "Raw / Unaligned",
        }
    }

    /// Toggles between aligned and raw diff modes.
    pub fn toggled(&self) -> Self {
        match self {
            BinaryDiffMode::Aligned => BinaryDiffMode::Raw,
            BinaryDiffMode::Raw => BinaryDiffMode::Aligned,
        }
    }
}

/// A single byte cell within an aligned or raw diff stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedCell {
    /// The byte value if present in this stream position, or `None` if padded/gap.
    pub byte_val: Option<u8>,
    /// The original file byte offset if present.
    pub offset: Option<usize>,
    /// The diff classification for this cell.
    pub kind: DiffKind,
}

/// A single row of side-by-side aligned byte cells.
#[derive(Debug, Clone, Default)]
pub struct BinaryDiffRow {
    /// Zero-based row index in the rendered diff.
    pub row_index: usize,
    /// Starting byte offset for the left stream in this row.
    pub left_offset: Option<usize>,
    /// List of cells on the left side for this row (length matches `bytes_per_row`).
    pub left_cells: Vec<AlignedCell>,
    /// Starting byte offset for the right stream in this row.
    pub right_offset: Option<usize>,
    /// List of cells on the right side for this row.
    pub right_cells: Vec<AlignedCell>,
    /// True if any cell in this row has a difference.
    pub has_diff: bool,
    /// Single-character visual glyph summarizing the row state (e.g., `│`, `≠`, `+`, `-`, `~`).
    pub row_glyph: &'static str,
}

/// Aggregate statistics for a binary diff comparison.
#[derive(Debug, Clone, Default)]
pub struct BinaryDiffStats {
    /// Total bytes in the left stream.
    pub left_total_bytes: usize,
    /// Total bytes in the right stream.
    pub right_total_bytes: usize,
    /// Number of matching bytes.
    pub match_count: usize,
    /// Number of mismatched/substituted bytes.
    pub mismatch_count: usize,
    /// Number of bytes present only on the left side.
    pub left_only_count: usize,
    /// Number of bytes present only on the right side.
    pub right_only_count: usize,
    /// Calculated percentage similarity (0.0 to 100.0).
    pub similarity_pct: f32,
}

/// A contiguous block of differences across one or more rows.
#[derive(Debug, Clone)]
pub struct BinaryDiffBlock {
    /// 1-indexed block identifier.
    pub id: usize,
    /// Starting row index (inclusive).
    pub start_row: usize,
    /// Ending row index (inclusive).
    pub end_row: usize,
    /// Left stream byte offset at the start of the block.
    pub left_offset: Option<usize>,
    /// Right stream byte offset at the start of the block.
    pub right_offset: Option<usize>,
    /// Total number of differing bytes in this block.
    pub byte_count: usize,
    /// Human-readable description of change kind ("Mismatch", "Deletion", "Insertion", "Replacement").
    pub kind_label: String,
    /// Primary diff kind.
    pub kind: DiffKind,
    /// Compact text or hex preview summary of the change.
    pub preview: String,
}

/// Complete binary comparison result including rows, summary stats, and diff blocks.
#[derive(Debug, Clone)]
pub struct BinaryDiffResult {
    /// Label for the left side item.
    pub left_label: String,
    /// Label for the right side item.
    pub right_label: String,
    /// Number of bytes displayed per row column.
    pub bytes_per_row: usize,
    /// Diff alignment mode used.
    pub mode: BinaryDiffMode,
    /// Whether a time or workspace limit made the alignment potentially approximate.
    ///
    /// All source bytes and offsets are still represented.
    pub alignment_limited: bool,
    /// Source-backed rows, materialized only when requested.
    pub rows: BinaryDiffRows,
    /// Summary metrics.
    pub stats: BinaryDiffStats,
    /// Number of rows containing at least one difference.
    pub diff_row_count: usize,
    /// Compact block summaries; display strings are materialized on request.
    pub diff_blocks: BinaryDiffBlocks,
}

// Bound the quadratic alignment search, independently of linear result construction.
const ALIGNMENT_BUDGET: Duration = Duration::from_millis(500);

const MAX_BINARY_DIFF_FILE_BYTES: usize = 64 * 1024 * 1024;

fn read_binary_diff_file(path: &Path) -> Option<Vec<u8>> {
    little_clip_core::kernel_interface::read_content_bounded(path, MAX_BINARY_DIFF_FILE_BYTES).ok()
}

/// Extracts the raw binary bytes and human-readable label from any ClipItem.
pub fn get_item_binary_bytes(item: &ClipItem) -> (String, Vec<u8>) {
    match &item.payload {
        ClipPayload::Text(s) => {
            let trimmed = s.trim();
            let p = Path::new(trimmed);
            if p.is_file() {
                if let Some(bytes) = read_binary_diff_file(p) {
                    let label = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(trimmed)
                        .to_string();
                    return (label, bytes);
                }
            }
            (item.title.clone(), s.as_bytes().to_vec())
        }
        ClipPayload::Markdown(s) => (item.title.clone(), s.as_bytes().to_vec()),
        ClipPayload::FileBatch(paths) => {
            if let Some(first) = paths.first() {
                if let Some(bytes) = read_binary_diff_file(first) {
                    let label = first
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_string();
                    return (label, bytes);
                }
            }
            (item.title.clone(), Vec::new())
        }
        ClipPayload::Image { bytes, path, .. } => {
            if let Some(path) = path {
                if let Some(source_bytes) = read_binary_diff_file(path) {
                    return (item.title.clone(), source_bytes);
                }
            }
            (item.title.clone(), bytes.clone())
        }
        ClipPayload::AudioRef { path, .. } => {
            let bytes = read_binary_diff_file(path).unwrap_or_default();
            (item.title.clone(), bytes)
        }
        ClipPayload::Color { hex, r, g, b } => {
            let s = format!("{} ({}, {}, {})", hex, r, g, b);
            (item.title.clone(), s.into_bytes())
        }
        ClipPayload::Url(url) => (item.title.clone(), url.as_str().as_bytes().to_vec()),
        ClipPayload::MultiImage { paths, .. } => {
            if let Some(first) = paths.first() {
                if let Some(bytes) = read_binary_diff_file(first) {
                    let label = first
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("image")
                        .to_string();
                    return (label, bytes);
                }
            }
            (item.title.clone(), Vec::new())
        }
        ClipPayload::Animation { path, .. } => {
            let bytes = read_binary_diff_file(path).unwrap_or_default();
            (item.title.clone(), bytes)
        }
        ClipPayload::Directory(p) => (item.title.clone(), p.to_string_lossy().as_bytes().to_vec()),
    }
}

/// Checks if a ClipItem represents two files (e.g. FileBatch with 2+ files or newline-separated paths).
pub fn resolve_two_files_from_single_item(
    item: &ClipItem,
) -> Option<((String, Vec<u8>), (String, Vec<u8>))> {
    match &item.payload {
        ClipPayload::FileBatch(paths) if paths.len() >= 2 => {
            let b1 = read_binary_diff_file(&paths[0])?;
            let b2 = read_binary_diff_file(&paths[1])?;
            let l1 = paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file_a")
                .to_string();
            let l2 = paths[1]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file_b")
                .to_string();
            Some(((l1, b1), (l2, b2)))
        }
        ClipPayload::MultiImage { paths, .. } if paths.len() >= 2 => {
            let b1 = read_binary_diff_file(&paths[0])?;
            let b2 = read_binary_diff_file(&paths[1])?;
            let l1 = paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image_a")
                .to_string();
            let l2 = paths[1]
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image_b")
                .to_string();
            Some(((l1, b1), (l2, b2)))
        }
        ClipPayload::Text(text) => {
            let lines: Vec<&str> = text
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect();
            if lines.len() == 2 {
                let p1 = Path::new(lines[0]);
                let p2 = Path::new(lines[1]);
                if p1.is_file() && p2.is_file() {
                    let b1 = read_binary_diff_file(p1)?;
                    let b2 = read_binary_diff_file(p2)?;
                    let l1 = p1
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(lines[0])
                        .to_string();
                    let l2 = p2
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(lines[1])
                        .to_string();
                    return Some(((l1, b1), (l2, b2)));
                }
            }
            None
        }
        _ => None,
    }
}

/// Resolves the two binary targets given selected items and slot state.
pub fn resolve_binary_diff_pair(
    selected_items: &[&ClipItem],
    state: &SlotManager,
) -> Option<((String, Vec<u8>), (String, Vec<u8>))> {
    if selected_items.len() >= 2 {
        let (l_lbl, l_bytes) = get_item_binary_bytes(selected_items[0]);
        let (r_lbl, r_bytes) = get_item_binary_bytes(selected_items[1]);
        return Some(((l_lbl, l_bytes), (r_lbl, r_bytes)));
    }

    if selected_items.len() == 1 {
        let single = selected_items[0];
        if let Some(pair) = resolve_two_files_from_single_item(single) {
            return Some(pair);
        }

        // Pair with another slot in priority order: Right Hand -> Left Hand -> LeftPrior1 -> Undo
        let (l_lbl, l_bytes) = get_item_binary_bytes(single);
        let other = state
            .right_hand()
            .filter(|i| i.id != single.id)
            .or_else(|| state.left_hand().filter(|i| i.id != single.id))
            .or_else(|| state.get(SlotId::LeftPrior1).filter(|i| i.id != single.id))
            .or_else(|| state.get(SlotId::Undo).filter(|i| i.id != single.id));

        if let Some(other_item) = other {
            let (r_lbl, r_bytes) = get_item_binary_bytes(other_item);
            return Some(((l_lbl, l_bytes), (r_lbl, r_bytes)));
        }
    }

    // Default: pair Left Hand with Right Hand, or Left Hand with Left Prior / Undo
    if let (Some(lh), Some(rh)) = (state.left_hand(), state.right_hand()) {
        let (l_lbl, l_bytes) = get_item_binary_bytes(lh);
        let (r_lbl, r_bytes) = get_item_binary_bytes(rh);
        return Some(((l_lbl, l_bytes), (r_lbl, r_bytes)));
    } else if let Some(lh) = state.left_hand() {
        if let Some(pair) = resolve_two_files_from_single_item(lh) {
            return Some(pair);
        }
        let prior = state
            .get(SlotId::LeftPrior1)
            .or_else(|| state.get(SlotId::Undo))
            .or_else(|| state.get(SlotId::LeftPrior2));
        if let Some(pr) = prior {
            let (l_lbl, l_bytes) = get_item_binary_bytes(lh);
            let (r_lbl, r_bytes) = get_item_binary_bytes(pr);
            return Some(((l_lbl, l_bytes), (r_lbl, r_bytes)));
        }
    }

    None
}

/// Computes the complete binary diff result between two byte slices.
pub fn compute_binary_diff(
    left_label: &str,
    left_bytes: &[u8],
    right_label: &str,
    right_bytes: &[u8],
    mode: BinaryDiffMode,
    bytes_per_row: usize,
) -> BinaryDiffResult {
    compute_binary_diff_shared(
        left_label,
        Arc::new(left_bytes.to_vec()),
        right_label,
        Arc::new(right_bytes.to_vec()),
        mode,
        bytes_per_row,
    )
}

/// Computes a diff while sharing the caller's source buffers with lazy rows.
pub fn compute_binary_diff_shared(
    left_label: &str,
    left_bytes: Arc<Vec<u8>>,
    right_label: &str,
    right_bytes: Arc<Vec<u8>>,
    mode: BinaryDiffMode,
    bytes_per_row: usize,
) -> BinaryDiffResult {
    let bpr = bytes_per_row.max(4);
    let left_total_bytes = left_bytes.len();
    let right_total_bytes = right_bytes.len();
    let (rows, alignment_limited) = BinaryDiffRows::new(
        left_bytes,
        right_bytes,
        mode,
        bpr,
        Instant::now() + ALIGNMENT_BUDGET,
    );
    let mut diff_row_count = 0;
    let mut match_count = 0;
    let mut mismatch_count = 0;
    let mut left_only_count = 0;
    let mut right_only_count = 0;
    let mut row = BinaryDiffRow::default();
    for row_index in 0..rows.len() {
        rows.write_row(row_index, &mut row);
        for cell in &row.left_cells {
            match cell.kind {
                DiffKind::Match if cell.byte_val.is_some() => match_count += 1,
                DiffKind::Mismatch => mismatch_count += 1,
                DiffKind::LeftOnly => left_only_count += 1,
                DiffKind::RightOnly => right_only_count += 1,
                DiffKind::Match => {}
            }
        }
        if row.has_diff {
            diff_row_count += 1;
        }
    }

    let total_compared = match_count + mismatch_count + left_only_count + right_only_count;
    let similarity_pct = if total_compared > 0 {
        (match_count as f32 / total_compared as f32) * 100.0
    } else {
        100.0
    };

    let stats = BinaryDiffStats {
        left_total_bytes,
        right_total_bytes,
        match_count,
        mismatch_count,
        left_only_count,
        right_only_count,
        similarity_pct,
    };

    let diff_blocks = extract_diff_blocks(&rows);

    BinaryDiffResult {
        left_label: left_label.to_string(),
        right_label: right_label.to_string(),
        bytes_per_row: bpr,
        mode,
        alignment_limited,
        rows,
        stats,
        diff_row_count,
        diff_blocks,
    }
}

/// Groups contiguous diff rows into compact blocks for high-level navigation.
pub fn extract_diff_blocks(rows: &BinaryDiffRows) -> BinaryDiffBlocks {
    BinaryDiffBlocks::new(rows.clone())
}

/// Converts a byte into a printable ASCII character or '.'
pub fn byte_to_ascii_char(b: u8) -> char {
    if b.is_ascii_graphic() || b == b' ' {
        b as char
    } else {
        '.'
    }
}

/// Parses a search query into candidate byte sequence (if valid hex) or ASCII bytes.
pub fn parse_search_query(q: &str) -> Vec<Vec<u8>> {
    let q_clean = q.trim();
    if q_clean.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();

    // 1. Try interpreting as hex string: "48 65 6c" or "0x48, 0x65" or "48656c"
    let hex_chars: String = q_clean.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex_chars.len() >= 2 && hex_chars.len() % 2 == 0 {
        if let Ok(bytes) = (0..hex_chars.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex_chars[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
        {
            candidates.push(bytes);
        }
    }

    // 2. Interpret as literal ASCII / UTF-8
    let ascii_bytes = q_clean.as_bytes().to_vec();
    if !candidates.contains(&ascii_bytes) {
        candidates.push(ascii_bytes);
    }

    // 3. Case variations for ASCII
    let lower_bytes = q_clean.to_lowercase().into_bytes();
    if !candidates.contains(&lower_bytes) {
        candidates.push(lower_bytes);
    }
    let upper_bytes = q_clean.to_uppercase().into_bytes();
    if !candidates.contains(&upper_bytes) {
        candidates.push(upper_bytes);
    }

    candidates
}

/// Finds all row indices matching the search pattern in either Left or Right streams.
pub fn search_rows(result: &BinaryDiffResult, query: &str) -> Vec<usize> {
    let candidates = parse_search_query(query);
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut matching_rows = Vec::new();

    let mut row = BinaryDiffRow::default();
    for index in 0..result.rows.len() {
        result.rows.write_row(index, &mut row);
        let left_bytes: Vec<u8> = row.left_cells.iter().filter_map(|c| c.byte_val).collect();
        let right_bytes: Vec<u8> = row.right_cells.iter().filter_map(|c| c.byte_val).collect();

        let matched = candidates.iter().any(|pattern| {
            if pattern.is_empty() {
                return false;
            }
            left_bytes
                .windows(pattern.len())
                .any(|w| w == pattern.as_slice())
                || right_bytes
                    .windows(pattern.len())
                    .any(|w| w == pattern.as_slice())
        });

        if matched {
            matching_rows.push(row.row_index);
        }
    }

    matching_rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::ClipPayload;
    use uuid::Uuid;

    fn item(payload: ClipPayload, title: &str) -> ClipItem {
        ClipItem {
            id: Uuid::new_v4(),
            created_at: 0,
            payload,
            title: title.to_string(),
            subtitle: "display metadata, not source data".to_string(),
            details: Vec::new(),
            handler_tag: "TEST".to_string(),
        }
    }

    #[test]
    fn expired_alignment_preserves_every_byte_and_offset() {
        let left = b"prefix:abcbdbebfbgb:tail";
        let right = b"prefix:abxbxbxbxbxbxbxb:tail";
        let (rows, limited) = BinaryDiffRows::new(
            Arc::new(left.to_vec()),
            Arc::new(right.to_vec()),
            BinaryDiffMode::Aligned,
            4,
            Instant::now() - Duration::from_secs(1),
        );
        let materialized: Vec<_> = rows.iter().collect();
        let left_cells: Vec<_> = materialized
            .iter()
            .flat_map(|row| row.left_cells.iter().copied())
            .collect();
        let right_cells: Vec<_> = materialized
            .iter()
            .flat_map(|row| row.right_cells.iter().copied())
            .collect();
        assert!(limited);
        assert_eq!(left_cells.len(), right_cells.len());
        for (source, cells) in [
            (left.as_slice(), &left_cells),
            (right.as_slice(), &right_cells),
        ] {
            let recovered: Vec<_> = cells.iter().filter_map(|cell| cell.byte_val).collect();
            let offsets: Vec<_> = cells.iter().filter_map(|cell| cell.offset).collect();
            assert_eq!(recovered, source);
            assert_eq!(offsets, (0..source.len()).collect::<Vec<_>>());
        }
        for (left, right) in left_cells.iter().zip(&right_cells) {
            if left.kind == DiffKind::Match || right.kind == DiffKind::Match {
                assert_eq!(left.byte_val, right.byte_val);
            }
        }
        assert_eq!(left_cells.first().unwrap().kind, DiffKind::Match);
        assert_eq!(
            left_cells
                .iter()
                .rev()
                .find(|cell| cell.byte_val.is_some())
                .unwrap()
                .kind,
            DiffKind::Match
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn large_fragmented_diffs_fit_a_bounded_process_heap() {
        const CHILD: &str = "LITTLE_CLIP_BINARY_DIFF_MEMORY_TEST";
        if std::env::var_os(CHILD).is_none() {
            // Isolate the allocation ceiling from the parallel test runner.
            // The legacy eager cells need hundreds of MiB for these inputs.
            let output = std::process::Command::new("/bin/sh")
                .args([
                    "-c",
                    "ulimit -c 0; ulimit -d 262144 || exit; exec \"$@\"",
                    "binary-diff-memory",
                ])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "binary_diff::tests::large_fragmented_diffs_fit_a_bounded_process_heap",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let len = 8 * 1024 * 1024;
        let left = Arc::new(vec![0; len]);
        let right = Arc::new(
            (0..len)
                .map(|i| u8::from((i / 4) % 2 == 0))
                .collect::<Vec<_>>(),
        );
        for mode in [BinaryDiffMode::Raw, BinaryDiffMode::Aligned] {
            let result = compute_binary_diff_shared(
                "a",
                Arc::clone(&left),
                "b",
                Arc::clone(&right),
                mode,
                4,
            );
            assert_eq!(result.rows.len(), len / 4);
            assert_eq!(result.diff_row_count, len / 8);
            assert_eq!(result.stats.match_count, len / 2);
            assert_eq!(result.stats.mismatch_count, len / 2);
            assert_eq!(result.diff_blocks.len(), len / 8);
            assert_eq!(result.diff_blocks.get(0).unwrap().byte_count, 4);
            let last = result
                .diff_blocks
                .get(result.diff_blocks.len() - 1)
                .unwrap();
            assert_eq!(last.start_row, len / 4 - 2);
            assert_eq!(last.left_offset, Some(len - 8));
            assert_eq!(last.preview, "00 00 00 00 ≠ 01 01 01 01");
            assert!(result.rows.get(result.rows.len()).is_none());
            assert!(result.diff_blocks.get(result.diff_blocks.len()).is_none());
            assert_eq!(result.alignment_limited, mode == BinaryDiffMode::Aligned);
            assert!(
                Arc::strong_count(&left) > 1,
                "the result must share its source buffer"
            );
        }
        let (identical, limited) = BinaryDiffRows::new(
            Arc::clone(&left),
            Arc::clone(&left),
            BinaryDiffMode::Aligned,
            16,
            Instant::now() - Duration::from_secs(1),
        );
        assert!(!limited, "identical inputs require no approximate search");
        assert_eq!(identical.len(), len / 16);
        let insertion = compute_binary_diff_shared(
            "empty",
            Arc::new(Vec::new()),
            "full",
            Arc::clone(&left),
            BinaryDiffMode::Aligned,
            16,
        );
        assert!(!insertion.alignment_limited);
        assert_eq!(insertion.stats.right_only_count, len);
        assert_eq!(
            insertion.diff_blocks.get(0).unwrap().kind,
            DiffKind::RightOnly
        );
        let mut nearly_equal = left.as_ref().clone();
        nearly_equal[len / 2] = 1;
        let result = compute_binary_diff_shared(
            "a",
            Arc::clone(&left),
            "b",
            Arc::new(nearly_equal),
            BinaryDiffMode::Aligned,
            16,
        );
        assert_eq!(result.stats.match_count, len - 1);
        assert_eq!(result.stats.mismatch_count, 1);
        assert_eq!(result.diff_blocks.len(), 1);
        assert_eq!(
            result.diff_blocks.get(0).unwrap().left_offset,
            Some(len / 2)
        );
    }

    #[test]
    fn lazy_rows_preserve_sources_offsets_and_block_statistics() {
        let mut seed = 0x12345678_u64;
        for case in 0..100 {
            let mut make = |len| -> Vec<u8> {
                (0..len)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        b'a' + (seed % 5) as u8
                    })
                    .collect()
            };
            let left = make(case % 53);
            let right = make((case * 13) % 59);
            for mode in [BinaryDiffMode::Raw, BinaryDiffMode::Aligned] {
                let result = compute_binary_diff("a", &left, "b", &right, mode, 4 + case % 17);
                let rows: Vec<_> = result.rows.iter().collect();
                for (source, is_left) in [(&left, true), (&right, false)] {
                    let cells: Vec<_> = rows
                        .iter()
                        .flat_map(|row| {
                            if is_left {
                                &row.left_cells
                            } else {
                                &row.right_cells
                            }
                        })
                        .collect();
                    let bytes: Vec<_> = cells.iter().filter_map(|cell| cell.byte_val).collect();
                    let offsets: Vec<_> = cells.iter().filter_map(|cell| cell.offset).collect();
                    assert_eq!(bytes, *source);
                    assert_eq!(offsets, (0..source.len()).collect::<Vec<_>>());
                }
                assert_eq!(
                    result.diff_row_count,
                    rows.iter().filter(|row| row.has_diff).count()
                );
                assert_eq!(
                    result.stats.match_count
                        + result.stats.mismatch_count
                        + result.stats.left_only_count,
                    left.len()
                );
                assert_eq!(
                    result.stats.match_count
                        + result.stats.mismatch_count
                        + result.stats.right_only_count,
                    right.len()
                );
                let mut block_bytes = 0;
                let mut block_rows = 0;
                for block in result.diff_blocks.iter() {
                    assert!(
                        rows[block.start_row..=block.end_row]
                            .iter()
                            .all(|row| row.has_diff)
                    );
                    block_bytes += block.byte_count;
                    block_rows += block.end_row - block.start_row + 1;
                }
                assert_eq!(block_rows, result.diff_row_count);
                assert_eq!(
                    block_bytes,
                    result.stats.mismatch_count
                        + result.stats.left_only_count
                        + result.stats.right_only_count
                );
            }
        }
    }

    #[test]
    fn lazy_blocks_keep_preview_text_and_gap_offsets() {
        let raw = compute_binary_diff(
            "a",
            b"sameOld!sameTAIL",
            "b",
            b"sameNew!same",
            BinaryDiffMode::Raw,
            4,
        );
        assert_eq!(raw.diff_blocks.len(), 2);
        let mismatch = raw.diff_blocks.get(0).unwrap();
        assert_eq!(mismatch.start_row, 1);
        assert_eq!(mismatch.byte_count, 3);
        assert_eq!(mismatch.kind_label, "Mismatch");
        assert_eq!(mismatch.preview, "Old ≠ New");
        let deletion = raw.diff_blocks.get(1).unwrap();
        assert_eq!(deletion.left_offset, Some(12));
        assert_eq!(deletion.right_offset, None);
        assert_eq!(deletion.kind_label, "Deletion");
        assert_eq!(deletion.preview, "TAIL ≠ --");
        assert_eq!(search_rows(&raw, "TAIL"), vec![3]);
        let aligned = compute_binary_diff(
            "a",
            b"abcdWXYZefgh",
            "b",
            b"abcdefgh",
            BinaryDiffMode::Aligned,
            4,
        );
        assert_eq!(aligned.stats.match_count, 8);
        let gap = aligned.diff_blocks.get(0).unwrap();
        assert_eq!(gap.left_offset, Some(4));
        assert_eq!(gap.right_offset, None);
        assert_eq!(gap.preview, "WXYZ ≠ --");
    }

    #[test]
    fn raw_diff_never_reports_an_alignment_limit() {
        let result = compute_binary_diff("a", b"abc", "b", b"axc", BinaryDiffMode::Raw, 16);
        assert!(!result.alignment_limited);
        assert_eq!(result.stats.mismatch_count, 1);
    }

    #[test]
    fn test_identical_binary_diff() {
        let a = b"Hello, World!";
        let b = b"Hello, World!";
        let res = compute_binary_diff("file_a", a, "file_b", b, BinaryDiffMode::Aligned, 16);
        assert_eq!(res.stats.match_count, 13);
        assert_eq!(res.stats.mismatch_count, 0);
        assert_eq!(res.stats.left_only_count, 0);
        assert_eq!(res.stats.right_only_count, 0);
        assert_eq!(res.stats.similarity_pct, 100.0);
        assert!(res.diff_row_count == 0);
    }

    #[test]
    fn test_aligned_binary_diff_insertion() {
        // "Hello World" vs "Hello Brave World"
        let a = b"Hello World";
        let b = b"Hello Brave World";
        let res = compute_binary_diff("left", a, "right", b, BinaryDiffMode::Aligned, 16);
        assert!(!res.alignment_limited);
        assert_eq!(res.stats.right_only_count, 6); // "Brave "
        assert_eq!(res.stats.match_count, 11); // "Hello World"
        assert_eq!(res.stats.mismatch_count, 0);
        assert_eq!(res.diff_row_count, 1);
    }

    #[test]
    fn test_search_rows() {
        let a = b"ELF header binary pattern test";
        let b = b"ELF header modified pattern test";
        let res = compute_binary_diff("left", a, "right", b, BinaryDiffMode::Aligned, 16);
        let matches = search_rows(&res, "ELF");
        assert_eq!(matches, vec![0]);

        // Search hex 0x45 0x4C 0x46 ("ELF")
        let hex_matches = search_rows(&res, "45 4C 46");
        assert_eq!(hex_matches, vec![0]);
    }

    #[test]
    fn binary_bytes_use_payload_paths_instead_of_display_titles() {
        let temp_dir = tempfile::tempdir().unwrap();
        let image_path = temp_dir.path().join("source-image.bin");
        let audio_path = temp_dir.path().join("source-audio.bin");
        std::fs::write(&image_path, b"original encoded image bytes").unwrap();
        std::fs::write(&audio_path, b"original audio bytes").unwrap();

        let image = item(
            ClipPayload::Image {
                bytes: b"in-memory preview bytes".to_vec(),
                path: Some(image_path),
                width: 1,
                height: 1,
                mime: "image/png".to_string(),
                thumbnail: None,
            },
            "A renamed image",
        );
        let audio = item(
            ClipPayload::AudioRef {
                path: audio_path,
                duration_ms: 0,
                sample_rate: 0,
                channels: 0,
                format: "wav".to_string(),
                waveform: None,
            },
            "A renamed audio clip",
        );

        assert_eq!(
            get_item_binary_bytes(&image).1,
            b"original encoded image bytes"
        );
        assert_eq!(get_item_binary_bytes(&audio).1, b"original audio bytes");
    }
}
