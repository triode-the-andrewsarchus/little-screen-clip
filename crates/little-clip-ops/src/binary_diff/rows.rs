//! Source-backed rows: retain alignment spans, materialize only requested cells.

use super::{AlignedCell, BinaryDiffMode, BinaryDiffRow, DiffKind};
use std::{ops::Range, sync::Arc, time::Instant};

// Myers allocates its frontier arrays before consulting the deadline. Limit its
// input workspace as well as elapsed search time; larger changed spans retain
// all bytes with a coarser, explicitly labelled alignment.
const MAX_ALIGNMENT_MIDDLE_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
struct AlignmentRun {
    start: usize,
    len: usize,
    left: Option<usize>,
    right: Option<usize>,
}

/// Source-backed binary rows that materialize cells only on request.
///
/// Clones share both input buffers and alignment spans. A requested row owns its
/// cells and remains valid independently of this collection.
#[derive(Debug, Clone)]
pub struct BinaryDiffRows {
    left: Arc<Vec<u8>>,
    right: Arc<Vec<u8>>,
    runs: Arc<Vec<AlignmentRun>>,
    columns: usize,
    bytes_per_row: usize,
}

impl BinaryDiffRows {
    pub(super) fn new(
        left: Arc<Vec<u8>>,
        right: Arc<Vec<u8>>,
        mode: BinaryDiffMode,
        bytes_per_row: usize,
        deadline: Instant,
    ) -> (Self, bool) {
        let mut rows = Self {
            left,
            right,
            runs: Arc::new(Vec::new()),
            columns: 0,
            bytes_per_row,
        };
        let limited = match mode {
            BinaryDiffMode::Raw => {
                rows.push_ranges(0..rows.left.len(), 0..rows.right.len());
                false
            }
            BinaryDiffMode::Aligned => rows.align(deadline),
        };
        (rows, limited)
    }

    fn align(&mut self, deadline: Instant) -> bool {
        // Keep ordinary comparisons byte-for-byte compatible with the library's
        // compaction choices. Trim only when needed to avoid a large workspace.
        let (prefix, suffix) = if self.left.len() + self.right.len() > MAX_ALIGNMENT_MIDDLE_BYTES {
            let prefix = self
                .left
                .iter()
                .zip(self.right.iter())
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = self.left[prefix..]
                .iter()
                .rev()
                .zip(self.right[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            (prefix, suffix)
        } else {
            (0, 0)
        };
        self.push_ranges(0..prefix, 0..prefix);
        let left_end = self.left.len() - suffix;
        let right_end = self.right.len() - suffix;
        let middle_bytes = left_end - prefix + right_end - prefix;
        let limited = if left_end == prefix || right_end == prefix {
            // Identical inputs and pure insertion/deletion need no search, even
            // when their size would exceed the search workspace budget.
            self.push_ranges(prefix..left_end, prefix..right_end);
            false
        } else if middle_bytes > MAX_ALIGNMENT_MIDDLE_BYTES {
            self.push_ranges(prefix..left_end, prefix..right_end);
            true
        } else {
            let ops = similar::capture_diff_slices_deadline(
                similar::Algorithm::Myers,
                &self.left[prefix..left_end],
                &self.right[prefix..right_end],
                Some(deadline),
            );
            let limited = Instant::now() >= deadline;
            for op in ops {
                let left = op.old_range();
                let right = op.new_range();
                self.push_ranges(
                    left.start + prefix..left.end + prefix,
                    right.start + prefix..right.end + prefix,
                );
            }
            limited
        };
        self.push_ranges(left_end..self.left.len(), right_end..self.right.len());
        limited
    }

    fn push_ranges(&mut self, left: Range<usize>, right: Range<usize>) {
        let common = left.len().min(right.len());
        self.push_run(common, Some(left.start), Some(right.start));
        self.push_run(left.len() - common, Some(left.start + common), None);
        self.push_run(right.len() - common, None, Some(right.start + common));
    }

    fn push_run(&mut self, len: usize, left: Option<usize>, right: Option<usize>) {
        if len != 0 {
            Arc::get_mut(&mut self.runs)
                .expect("alignment is not shared during construction")
                .push(AlignmentRun {
                    start: self.columns,
                    len,
                    left,
                    right,
                });
            self.columns += len;
        }
    }

    /// Returns the number of rows, including any partially filled final row.
    pub fn len(&self) -> usize {
        self.columns.div_ceil(self.bytes_per_row)
    }

    /// Returns whether both sources are empty.
    pub fn is_empty(&self) -> bool {
        self.columns == 0
    }

    /// Materializes one row, or returns `None` for an out-of-range index.
    pub fn get(&self, index: usize) -> Option<BinaryDiffRow> {
        if index >= self.len() {
            return None;
        }
        let mut row = BinaryDiffRow::default();
        self.write_row(index, &mut row);
        Some(row)
    }

    /// Materializes the first row, or returns `None` for empty inputs.
    pub fn first(&self) -> Option<BinaryDiffRow> {
        self.get(0)
    }

    /// Iterates over owned rows without retaining previously yielded rows.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = BinaryDiffRow> + '_ {
        (0..self.len()).map(|index| self.get(index).expect("index is within row count"))
    }

    pub(super) fn write_row(&self, index: usize, row: &mut BinaryDiffRow) {
        row.row_index = index;
        row.left_offset = None;
        row.right_offset = None;
        row.left_cells.clear();
        row.right_cells.clear();
        let mut sub = false;
        let mut ins = false;
        let mut del = false;
        let start = index * self.bytes_per_row;
        let mut run_index = self
            .runs
            .partition_point(|run| run.start <= start)
            .saturating_sub(1);
        for column in start..(start + self.bytes_per_row).min(self.columns) {
            if column >= self.runs[run_index].start + self.runs[run_index].len {
                run_index += 1;
            }
            let run = &self.runs[run_index];
            let delta = column - run.start;
            let left_offset = run.left.map(|offset| offset + delta);
            let right_offset = run.right.map(|offset| offset + delta);
            let left = left_offset.map(|offset| self.left[offset]);
            let right = right_offset.map(|offset| self.right[offset]);
            let kind = match (left, right) {
                (Some(a), Some(b)) if a == b => DiffKind::Match,
                (Some(_), Some(_)) => {
                    sub = true;
                    DiffKind::Mismatch
                }
                (Some(_), None) => {
                    del = true;
                    DiffKind::LeftOnly
                }
                (None, Some(_)) => {
                    ins = true;
                    DiffKind::RightOnly
                }
                (None, None) => unreachable!("an alignment span always has a source"),
            };
            row.left_offset = row.left_offset.or(left_offset);
            row.right_offset = row.right_offset.or(right_offset);
            row.left_cells.push(AlignedCell {
                byte_val: left,
                offset: left_offset,
                kind,
            });
            row.right_cells.push(AlignedCell {
                byte_val: right,
                offset: right_offset,
                kind,
            });
        }
        let padding = AlignedCell {
            byte_val: None,
            offset: None,
            kind: DiffKind::Match,
        };
        row.left_cells.resize(self.bytes_per_row, padding);
        row.right_cells.resize(self.bytes_per_row, padding);
        row.has_diff = sub || ins || del;
        row.row_glyph = match (sub, ins, del) {
            (false, false, false) => "│",
            (true, false, false) => "≠",
            (_, true, false) => "+",
            (_, false, true) => "-",
            _ => "~",
        };
    }
}
