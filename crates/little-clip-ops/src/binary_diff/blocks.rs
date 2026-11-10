//! Compact block metadata, with strings created only for visible cards.

use super::{BinaryDiffBlock, BinaryDiffRow, BinaryDiffRows, DiffKind};
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
enum BlockClass {
    Replacement,
    Deletion,
    Insertion,
    Mismatch,
    Mixed,
}

impl BlockClass {
    fn from_counts(mismatches: usize, deletions: usize, insertions: usize) -> Self {
        match (mismatches > 0, deletions > 0, insertions > 0) {
            (_, true, true) => Self::Replacement,
            (false, true, false) => Self::Deletion,
            (false, false, true) => Self::Insertion,
            (true, false, false) => Self::Mismatch,
            _ => Self::Mixed,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Replacement => "Replacement",
            Self::Deletion => "Deletion",
            Self::Insertion => "Insertion",
            Self::Mismatch => "Mismatch",
            Self::Mixed => "Mixed Diff",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct PreviewBytes {
    bytes: [u8; 4],
    len: u8,
}

impl PreviewBytes {
    fn push(&mut self, value: Option<u8>) {
        if let Some(byte) = value.filter(|_| self.len < 4) {
            self.bytes[self.len as usize] = byte;
            self.len += 1;
        }
    }

    fn text(self) -> String {
        let bytes = &self.bytes[..self.len as usize];
        if bytes.is_empty() {
            "--".to_owned()
        } else if bytes.iter().all(|b| b.is_ascii_graphic()) {
            String::from_utf8_lossy(bytes).into_owned()
        } else {
            bytes
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Fixed-size metadata for one contiguous run of differing rows.
#[derive(Debug, Clone)]
pub struct BinaryDiffBlockSummary {
    /// First differing row, inclusive.
    pub start_row: usize,
    /// Last differing row, inclusive.
    pub end_row: usize,
    /// Number of differing aligned columns.
    pub byte_count: usize,
    class: BlockClass,
    left: PreviewBytes,
    right: PreviewBytes,
}

impl BinaryDiffBlockSummary {
    /// Returns the marker color classification without allocating display strings.
    pub fn kind(&self) -> DiffKind {
        match self.class {
            BlockClass::Deletion => DiffKind::LeftOnly,
            BlockClass::Insertion => DiffKind::RightOnly,
            _ => DiffKind::Mismatch,
        }
    }
}

/// Shared compact summaries of contiguous difference blocks.
///
/// Display strings and source offsets are generated only for requested blocks;
/// cloning the collection shares its metadata and source-backed rows.
#[derive(Debug, Clone)]
pub struct BinaryDiffBlocks {
    rows: BinaryDiffRows,
    summaries: Arc<Vec<BinaryDiffBlockSummary>>,
}

impl BinaryDiffBlocks {
    pub(super) fn new(rows: BinaryDiffRows) -> Self {
        let mut summaries = Vec::new();
        let mut row = BinaryDiffRow::default();
        let mut builder: Option<BlockBuilder> = None;
        for index in 0..rows.len() {
            rows.write_row(index, &mut row);
            if row.has_diff {
                builder
                    .get_or_insert_with(|| BlockBuilder::new(index))
                    .observe(&row);
            } else if let Some(block) = builder.take() {
                summaries.push(block.finish());
            }
        }
        if let Some(block) = builder {
            summaries.push(block.finish());
        }
        Self {
            rows,
            summaries: Arc::new(summaries),
        }
    }

    /// Returns fixed-size block metadata without materializing card text.
    pub fn summaries(&self) -> &[BinaryDiffBlockSummary] {
        &self.summaries
    }

    /// Returns the number of difference blocks.
    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    /// Returns whether the inputs have no differing rows.
    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }

    /// Materializes a block's text and offsets, or returns `None` past the end.
    pub fn get(&self, index: usize) -> Option<BinaryDiffBlock> {
        let summary = self.summaries.get(index)?;
        let row = self.rows.get(summary.start_row)?;
        Some(BinaryDiffBlock {
            id: index + 1,
            start_row: summary.start_row,
            end_row: summary.end_row,
            left_offset: row.left_offset,
            right_offset: row.right_offset,
            byte_count: summary.byte_count,
            kind_label: summary.class.label().to_owned(),
            kind: summary.kind(),
            preview: format!("{} ≠ {}", summary.left.text(), summary.right.text()),
        })
    }

    /// Iterates over owned blocks without retaining previously yielded strings.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = BinaryDiffBlock> + '_ {
        (0..self.len()).map(|index| self.get(index).expect("index is within block count"))
    }
}

struct BlockBuilder {
    summary: BinaryDiffBlockSummary,
    mismatches: usize,
    deletions: usize,
    insertions: usize,
}

impl BlockBuilder {
    fn new(start_row: usize) -> Self {
        Self {
            summary: BinaryDiffBlockSummary {
                start_row,
                end_row: start_row,
                byte_count: 0,
                class: BlockClass::Mixed,
                left: PreviewBytes::default(),
                right: PreviewBytes::default(),
            },
            mismatches: 0,
            deletions: 0,
            insertions: 0,
        }
    }

    fn observe(&mut self, row: &BinaryDiffRow) {
        self.summary.end_row = row.row_index;
        for (left, right) in row.left_cells.iter().zip(&row.right_cells) {
            match left.kind {
                DiffKind::Mismatch => {
                    self.mismatches += 1;
                    self.summary.left.push(left.byte_val);
                    self.summary.right.push(right.byte_val);
                }
                DiffKind::LeftOnly => {
                    self.deletions += 1;
                    self.summary.left.push(left.byte_val);
                }
                DiffKind::RightOnly => {
                    self.insertions += 1;
                    self.summary.right.push(right.byte_val);
                }
                DiffKind::Match => {}
            }
        }
    }

    fn finish(mut self) -> BinaryDiffBlockSummary {
        self.summary.byte_count = self.mismatches + self.deletions + self.insertions;
        self.summary.class =
            BlockClass::from_counts(self.mismatches, self.deletions, self.insertions);
        self.summary
    }
}
