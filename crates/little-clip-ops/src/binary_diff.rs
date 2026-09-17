//! Binary diffing and alignment engine.
//!
//! Provides Myers-based aligned byte-level diffing (similar to `biodiff`), raw byte diffing,
//! diff block classification (mismatch, deletion, insertion, replacement), statistics calculation,
//! and hex/ASCII search capabilities across diff rows.

use little_clip_core::{ClipItem, ClipPayload, SlotId, SlotManager};
use std::path::Path;

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
#[derive(Debug, Clone)]
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
    /// All computed diff rows.
    pub rows: Vec<BinaryDiffRow>,
    /// Summary metrics.
    pub stats: BinaryDiffStats,
    /// List of row indices containing at least one difference.
    pub diff_row_indices: Vec<usize>,
    /// Clustered contiguous diff blocks.
    pub diff_blocks: Vec<BinaryDiffBlock>,
}

/// Extracts the raw binary bytes and human-readable label from any ClipItem.
pub fn get_item_binary_bytes(item: &ClipItem) -> (String, Vec<u8>) {
    match &item.payload {
        ClipPayload::Text(s) => {
            let trimmed = s.trim();
            let p = Path::new(trimmed);
            if p.is_file() {
                if let Ok(bytes) = std::fs::read(p) {
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
                if let Ok(bytes) = std::fs::read(first) {
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
                if let Ok(source_bytes) = std::fs::read(path) {
                    return (item.title.clone(), source_bytes);
                }
            }
            (item.title.clone(), bytes.clone())
        }
        ClipPayload::AudioRef { path, .. } => {
            let bytes = std::fs::read(path).unwrap_or_default();
            (item.title.clone(), bytes)
        }
        ClipPayload::Color { hex, r, g, b } => {
            let s = format!("{} ({}, {}, {})", hex, r, g, b);
            (item.title.clone(), s.into_bytes())
        }
        ClipPayload::Url(url) => (item.title.clone(), url.as_str().as_bytes().to_vec()),
        ClipPayload::MultiImage { paths, .. } => {
            if let Some(first) = paths.first() {
                if let Ok(bytes) = std::fs::read(first) {
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
            let bytes = std::fs::read(path).unwrap_or_default();
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
            let b1 = std::fs::read(&paths[0]).ok()?;
            let b2 = std::fs::read(&paths[1]).ok()?;
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
            let b1 = std::fs::read(&paths[0]).ok()?;
            let b2 = std::fs::read(&paths[1]).ok()?;
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
                    let b1 = std::fs::read(p1).ok()?;
                    let b2 = std::fs::read(p2).ok()?;
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
    let bpr = bytes_per_row.max(4);

    let (flat_left, flat_right) = match mode {
        BinaryDiffMode::Aligned => compute_aligned_stream(left_bytes, right_bytes),
        BinaryDiffMode::Raw => compute_raw_stream(left_bytes, right_bytes),
    };

    let total_cells = flat_left.len().max(flat_right.len());
    let mut rows = Vec::with_capacity((total_cells / bpr) + 1);
    let mut diff_row_indices = Vec::new();

    let mut match_count = 0;
    let mut mismatch_count = 0;
    let mut left_only_count = 0;
    let mut right_only_count = 0;

    let mut cell_idx = 0;
    let mut row_idx = 0;

    while cell_idx < total_cells {
        let mut left_cells = Vec::with_capacity(bpr);
        let mut right_cells = Vec::with_capacity(bpr);
        let mut has_diff = false;

        let mut row_has_sub = false;
        let mut row_has_ins = false;
        let mut row_has_del = false;

        let mut left_row_offset = None;
        let mut right_row_offset = None;

        for i in 0..bpr {
            let idx = cell_idx + i;
            let l_cell = flat_left.get(idx).copied().unwrap_or(AlignedCell {
                byte_val: None,
                offset: None,
                kind: DiffKind::Match,
            });
            let r_cell = flat_right.get(idx).copied().unwrap_or(AlignedCell {
                byte_val: None,
                offset: None,
                kind: DiffKind::Match,
            });

            if left_row_offset.is_none() && l_cell.offset.is_some() {
                left_row_offset = l_cell.offset;
            }
            if right_row_offset.is_none() && r_cell.offset.is_some() {
                right_row_offset = r_cell.offset;
            }

            // Determine combined cell diff kind
            let combined_kind = match (l_cell.byte_val, r_cell.byte_val) {
                (Some(b1), Some(b2)) => {
                    if b1 == b2 {
                        match_count += 1;
                        DiffKind::Match
                    } else {
                        mismatch_count += 1;
                        has_diff = true;
                        row_has_sub = true;
                        DiffKind::Mismatch
                    }
                }
                (Some(_), None) => {
                    left_only_count += 1;
                    has_diff = true;
                    row_has_del = true;
                    DiffKind::LeftOnly
                }
                (None, Some(_)) => {
                    right_only_count += 1;
                    has_diff = true;
                    row_has_ins = true;
                    DiffKind::RightOnly
                }
                (None, None) => DiffKind::Match,
            };

            let mut l_c = l_cell;
            let mut r_c = r_cell;
            l_c.kind = combined_kind;
            r_c.kind = combined_kind;

            left_cells.push(l_c);
            right_cells.push(r_c);
        }

        let row_glyph = if !has_diff {
            "│"
        } else if row_has_sub && !row_has_ins && !row_has_del {
            "≠"
        } else if row_has_ins && !row_has_del {
            "+"
        } else if row_has_del && !row_has_ins {
            "-"
        } else {
            "~"
        };

        if has_diff {
            diff_row_indices.push(row_idx);
        }

        rows.push(BinaryDiffRow {
            row_index: row_idx,
            left_offset: left_row_offset,
            left_cells,
            right_offset: right_row_offset,
            right_cells,
            has_diff,
            row_glyph,
        });

        cell_idx += bpr;
        row_idx += 1;
    }

    let total_compared = match_count + mismatch_count + left_only_count + right_only_count;
    let similarity_pct = if total_compared > 0 {
        (match_count as f32 / total_compared as f32) * 100.0
    } else {
        100.0
    };

    let stats = BinaryDiffStats {
        left_total_bytes: left_bytes.len(),
        right_total_bytes: right_bytes.len(),
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
        rows,
        stats,
        diff_row_indices,
        diff_blocks,
    }
}

/// Groups contiguous diff rows into discrete diff blocks for high-level navigation.
pub fn extract_diff_blocks(rows: &[BinaryDiffRow]) -> Vec<BinaryDiffBlock> {
    let mut blocks = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_end = 0;

    for (idx, row) in rows.iter().enumerate() {
        if row.has_diff {
            if current_start.is_none() {
                current_start = Some(idx);
            }
            current_end = idx;
        } else if let Some(start) = current_start {
            blocks.push(build_diff_block(blocks.len() + 1, start, current_end, rows));
            current_start = None;
        }
    }

    if let Some(start) = current_start {
        blocks.push(build_diff_block(blocks.len() + 1, start, current_end, rows));
    }

    blocks
}

/// Constructs a single `BinaryDiffBlock` from a span of rows.
fn build_diff_block(
    id: usize,
    start_row: usize,
    end_row: usize,
    rows: &[BinaryDiffRow],
) -> BinaryDiffBlock {
    let left_offset = rows[start_row].left_offset;
    let right_offset = rows[start_row].right_offset;

    let mut mismatch_count = 0;
    let mut left_only_count = 0;
    let mut right_only_count = 0;

    let mut left_preview = Vec::new();
    let mut right_preview = Vec::new();

    for r in &rows[start_row..=end_row] {
        for (lc, rc) in r.left_cells.iter().zip(r.right_cells.iter()) {
            match lc.kind {
                DiffKind::Mismatch => {
                    mismatch_count += 1;
                    if left_preview.len() < 4 {
                        if let Some(b) = lc.byte_val {
                            left_preview.push(b);
                        }
                    }
                    if right_preview.len() < 4 {
                        if let Some(b) = rc.byte_val {
                            right_preview.push(b);
                        }
                    }
                }
                DiffKind::LeftOnly => {
                    left_only_count += 1;
                    if left_preview.len() < 4 {
                        if let Some(b) = lc.byte_val {
                            left_preview.push(b);
                        }
                    }
                }
                DiffKind::RightOnly => {
                    right_only_count += 1;
                    if right_preview.len() < 4 {
                        if let Some(b) = rc.byte_val {
                            right_preview.push(b);
                        }
                    }
                }
                DiffKind::Match => {}
            }
        }
    }

    let byte_count = mismatch_count + left_only_count + right_only_count;

    let (kind_label, kind) = if left_only_count > 0 && right_only_count > 0 {
        ("Replacement".to_string(), DiffKind::Mismatch)
    } else if left_only_count > 0 && mismatch_count == 0 {
        ("Deletion".to_string(), DiffKind::LeftOnly)
    } else if right_only_count > 0 && mismatch_count == 0 {
        ("Insertion".to_string(), DiffKind::RightOnly)
    } else if mismatch_count > 0 && left_only_count == 0 && right_only_count == 0 {
        ("Mismatch".to_string(), DiffKind::Mismatch)
    } else {
        ("Mixed Diff".to_string(), DiffKind::Mismatch)
    };

    let l_str = if left_preview.is_empty() {
        "--".to_string()
    } else if left_preview.iter().all(|&b| b.is_ascii_graphic()) {
        String::from_utf8_lossy(&left_preview).to_string()
    } else {
        left_preview
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let r_str = if right_preview.is_empty() {
        "--".to_string()
    } else if right_preview.iter().all(|&b| b.is_ascii_graphic()) {
        String::from_utf8_lossy(&right_preview).to_string()
    } else {
        right_preview
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let preview = format!("{} ≠ {}", l_str, r_str);

    BinaryDiffBlock {
        id,
        start_row,
        end_row,
        left_offset,
        right_offset,
        byte_count,
        kind_label,
        kind,
        preview,
    }
}

/// Computes Myers-aligned cell streams for left and right byte slices.
fn compute_aligned_stream(
    left_bytes: &[u8],
    right_bytes: &[u8],
) -> (Vec<AlignedCell>, Vec<AlignedCell>) {
    let diff_ops = similar::capture_diff_slices(similar::Algorithm::Myers, left_bytes, right_bytes);

    let mut flat_left = Vec::new();
    let mut flat_right = Vec::new();

    for op in diff_ops {
        match op {
            similar::DiffOp::Equal {
                old_index,
                new_index,
                len,
            } => {
                for i in 0..len {
                    flat_left.push(AlignedCell {
                        byte_val: Some(left_bytes[old_index + i]),
                        offset: Some(old_index + i),
                        kind: DiffKind::Match,
                    });
                    flat_right.push(AlignedCell {
                        byte_val: Some(right_bytes[new_index + i]),
                        offset: Some(new_index + i),
                        kind: DiffKind::Match,
                    });
                }
            }
            similar::DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let common = old_len.min(new_len);
                for i in 0..common {
                    let b1 = left_bytes[old_index + i];
                    let b2 = right_bytes[new_index + i];
                    let kind = if b1 == b2 {
                        DiffKind::Match
                    } else {
                        DiffKind::Mismatch
                    };
                    flat_left.push(AlignedCell {
                        byte_val: Some(b1),
                        offset: Some(old_index + i),
                        kind,
                    });
                    flat_right.push(AlignedCell {
                        byte_val: Some(b2),
                        offset: Some(new_index + i),
                        kind,
                    });
                }

                if old_len > new_len {
                    for i in common..old_len {
                        flat_left.push(AlignedCell {
                            byte_val: Some(left_bytes[old_index + i]),
                            offset: Some(old_index + i),
                            kind: DiffKind::LeftOnly,
                        });
                        flat_right.push(AlignedCell {
                            byte_val: None,
                            offset: None,
                            kind: DiffKind::LeftOnly,
                        });
                    }
                } else if new_len > old_len {
                    for i in common..new_len {
                        flat_left.push(AlignedCell {
                            byte_val: None,
                            offset: None,
                            kind: DiffKind::RightOnly,
                        });
                        flat_right.push(AlignedCell {
                            byte_val: Some(right_bytes[new_index + i]),
                            offset: Some(new_index + i),
                            kind: DiffKind::RightOnly,
                        });
                    }
                }
            }
            similar::DiffOp::Delete {
                old_index, old_len, ..
            } => {
                for i in 0..old_len {
                    flat_left.push(AlignedCell {
                        byte_val: Some(left_bytes[old_index + i]),
                        offset: Some(old_index + i),
                        kind: DiffKind::LeftOnly,
                    });
                    flat_right.push(AlignedCell {
                        byte_val: None,
                        offset: None,
                        kind: DiffKind::LeftOnly,
                    });
                }
            }
            similar::DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for i in 0..new_len {
                    flat_left.push(AlignedCell {
                        byte_val: None,
                        offset: None,
                        kind: DiffKind::RightOnly,
                    });
                    flat_right.push(AlignedCell {
                        byte_val: Some(right_bytes[new_index + i]),
                        offset: Some(new_index + i),
                        kind: DiffKind::RightOnly,
                    });
                }
            }
        }
    }

    (flat_left, flat_right)
}

/// Computes unaligned cell streams for left and right byte slices at identical offsets.
fn compute_raw_stream(
    left_bytes: &[u8],
    right_bytes: &[u8],
) -> (Vec<AlignedCell>, Vec<AlignedCell>) {
    let max_len = left_bytes.len().max(right_bytes.len());
    let mut flat_left = Vec::with_capacity(max_len);
    let mut flat_right = Vec::with_capacity(max_len);

    for i in 0..max_len {
        let l_byte = left_bytes.get(i).copied();
        let r_byte = right_bytes.get(i).copied();

        let kind = match (l_byte, r_byte) {
            (Some(b1), Some(b2)) => {
                if b1 == b2 {
                    DiffKind::Match
                } else {
                    DiffKind::Mismatch
                }
            }
            (Some(_), None) => DiffKind::LeftOnly,
            (None, Some(_)) => DiffKind::RightOnly,
            (None, None) => DiffKind::Match,
        };

        flat_left.push(AlignedCell {
            byte_val: l_byte,
            offset: if l_byte.is_some() { Some(i) } else { None },
            kind,
        });

        flat_right.push(AlignedCell {
            byte_val: r_byte,
            offset: if r_byte.is_some() { Some(i) } else { None },
            kind,
        });
    }

    (flat_left, flat_right)
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

    for row in &result.rows {
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
    fn test_identical_binary_diff() {
        let a = b"Hello, World!";
        let b = b"Hello, World!";
        let res = compute_binary_diff("file_a", a, "file_b", b, BinaryDiffMode::Aligned, 16);
        assert_eq!(res.stats.match_count, 13);
        assert_eq!(res.stats.mismatch_count, 0);
        assert_eq!(res.stats.left_only_count, 0);
        assert_eq!(res.stats.right_only_count, 0);
        assert_eq!(res.stats.similarity_pct, 100.0);
        assert!(res.diff_row_indices.is_empty());
    }

    #[test]
    fn test_aligned_binary_diff_insertion() {
        // "Hello World" vs "Hello Brave World"
        let a = b"Hello World";
        let b = b"Hello Brave World";
        let res = compute_binary_diff("left", a, "right", b, BinaryDiffMode::Aligned, 16);
        assert_eq!(res.stats.right_only_count, 6); // "Brave "
        assert_eq!(res.stats.match_count, 11); // "Hello World"
        assert_eq!(res.stats.mismatch_count, 0);
        assert_eq!(res.diff_row_indices.len(), 1);
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
