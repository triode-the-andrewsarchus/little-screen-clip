use crate::ui_appearance::WorkbenchColors;
use egui::{Color32, Painter, Rect, Stroke, pos2};
use little_clip_core::engineering::tags::{Direction, FrameInfo, SampleBin, TraceBin};
use std::ops::Range;

pub(super) fn position(range: &Range<u64>, fraction: f32) -> u64 {
    range.start
        + ((range.end - range.start) as f64 * f64::from(fraction.clamp(0.0, 1.0))).round() as u64
}

pub(super) fn fraction(rect: Rect, x: f32) -> f32 {
    ((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0)
}

pub(super) fn x_at(rect: Rect, range: &Range<u64>, row: u64) -> f32 {
    rect.left()
        + rect.width()
            * (row.saturating_sub(range.start) as f64
                / range.end.saturating_sub(range.start).max(1) as f64) as f32
}

pub(super) fn direction(direction: Direction) -> &'static str {
    match direction {
        Direction::Reader => "Reader",
        Direction::Tag => "Tag",
    }
}

pub(super) fn trace_preview(frames: &[FrameInfo]) -> Vec<TraceBin> {
    frames
        .iter()
        .map(|&frame| TraceBin {
            first: frame,
            last: frame,
            readers: u64::from(frame.direction == Direction::Reader),
            tags: u64::from(frame.direction == Direction::Tag),
        })
        .collect()
}

pub(super) struct TraceAxis {
    rows: u64,
    origin: Option<u32>,
    end: u64,
}

impl TraceAxis {
    pub fn new(bins: &[TraceBin]) -> Self {
        let Some((first, last)) = bins.first().zip(bins.last()) else {
            return Self {
                rows: 0,
                origin: None,
                end: 0,
            };
        };
        let rows = last.last.row + 1;
        let origin = first.first.timestamp_ticks;
        let mut previous = 0;
        for bin in bins {
            let first = bin.first.timestamp_ticks.wrapping_sub(origin);
            let last = bin.last.timestamp_ticks.wrapping_sub(origin);
            if first < previous || last < first {
                return Self {
                    rows,
                    origin: None,
                    end: rows,
                };
            }
            previous = last;
        }
        if previous == 0 {
            return Self {
                rows,
                origin: None,
                end: rows,
            };
        }
        let end = u64::from(previous) + u64::from(last.last.duration_ticks).max(1);
        Self {
            rows,
            origin: Some(origin),
            end,
        }
    }

    fn start(&self, frame: FrameInfo) -> u64 {
        self.origin.map_or(frame.row, |origin| {
            u64::from(frame.timestamp_ticks.wrapping_sub(origin))
        })
    }

    fn stop(&self, frame: FrameInfo) -> u64 {
        self.start(frame)
            + if self.origin.is_some() {
                u64::from(frame.duration_ticks).max(1)
            } else {
                1
            }
    }

    pub fn domain(&self) -> Range<u64> {
        0..self.end
    }

    pub fn label(&self) -> &'static str {
        if self.origin.is_some() {
            "Elapsed capture ticks"
        } else {
            "Frame order"
        }
    }

    pub fn pick(&self, bins: &[TraceBin], fraction: f32) -> u64 {
        let offset = position(&self.domain(), fraction);
        if self.origin.is_none() {
            return offset.min(self.rows.saturating_sub(1));
        }
        let slot = bins
            .partition_point(|bin| self.start(bin.first) <= offset)
            .saturating_sub(1);
        let Some(bin) = bins.get(slot) else { return 0 };
        let lo = self.start(bin.first);
        let hi = self.start(bin.last);
        let fraction =
            (offset.saturating_sub(lo) as f32 / hi.saturating_sub(lo).max(1) as f32).min(1.0);
        position(&(bin.first.row..bin.last.row), fraction)
    }

    pub fn for_row(&self, bins: &[TraceBin], row: u64, frame: Option<FrameInfo>) -> u64 {
        if self.origin.is_none() {
            return row;
        }
        if let Some(frame) = frame {
            return self.start(frame);
        }
        let slot = bins.partition_point(|bin| bin.last.row < row);
        let Some(bin) = bins.get(slot) else {
            return self.end;
        };
        let fraction =
            row.saturating_sub(bin.first.row) as f32 / (bin.last.row - bin.first.row).max(1) as f32;
        position(&(self.start(bin.first)..self.start(bin.last)), fraction)
    }
}

pub(super) fn trace(
    painter: &Painter,
    rect: Rect,
    bins: &[TraceBin],
    axis: &TraceAxis,
    colors: WorkbenchColors,
) {
    painter.rect_filled(rect, 3.0, colors.recessed);
    let range = axis.domain();
    let middle = rect.center().y;
    let scale = bins
        .iter()
        .map(|bin| bin.readers.max(bin.tags))
        .max()
        .unwrap_or(1);
    let cap = rect.height() * 0.42;
    let clipped = painter.with_clip_rect(rect);
    let mut mesh = egui::Mesh::default();
    for bin in bins {
        let left = x_at(rect, &range, axis.start(bin.first));
        let right = x_at(rect, &range, axis.stop(bin.last)).max(left + 1.0);
        if bin.readers > 0 {
            let height = ((bin.readers as f64).ln_1p() / (scale as f64).ln_1p()) as f32 * cap;
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(left, middle - height), pos2(right, middle - 0.75)),
                colors.accent,
            );
        }
        if bin.tags > 0 {
            let height = ((bin.tags as f64).ln_1p() / (scale as f64).ln_1p()) as f32 * cap;
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(left, middle + 0.75), pos2(right, middle + height)),
                colors.warning,
            );
        }
    }
    clipped.add(mesh);
}

pub(super) fn sample_peaks(samples: &[i16], stride: usize) -> Vec<SampleBin> {
    samples
        .chunks(stride.max(1))
        .filter_map(|samples| {
            Some(SampleBin {
                min: *samples.iter().min()?,
                max: *samples.iter().max()?,
            })
        })
        .collect()
}

pub(super) fn samples(
    painter: &Painter,
    rect: Rect,
    range: &Range<u64>,
    start: u64,
    step: u64,
    bins: &[SampleBin],
    colors: WorkbenchColors,
) {
    painter.rect_filled(rect, 3.0, colors.recessed);
    let first = range
        .start
        .saturating_sub(start)
        .div_euclid(step)
        .min(bins.len() as u64) as usize;
    let end = range
        .end
        .saturating_sub(start)
        .div_ceil(step)
        .min(bins.len() as u64) as usize;
    let visible = &bins[first..end.max(first)];
    let [lo, hi] = visible.iter().fold([0, 0], |[lo, hi], peak| {
        [lo.min(peak.min), hi.max(peak.max)]
    });
    let span = f32::from(hi - lo).max(1.0);
    let y = |value| rect.bottom() - 2.0 - (rect.height() - 4.0) * f32::from(value - lo) / span;
    let clipped = painter.with_clip_rect(rect);
    let baseline = y(0);
    clipped.line_segment(
        [pos2(rect.left(), baseline), pos2(rect.right(), baseline)],
        Stroke::new(0.75, colors.border),
    );
    let mut path = Vec::with_capacity(visible.len());
    let mut mesh = egui::Mesh::default();
    for (slot, peak) in visible.iter().enumerate() {
        let row = start + (first + slot) as u64 * step;
        let x = x_at(rect, range, row);
        let top = y(peak.max);
        let bottom = y(peak.min);
        if step > 1 || peak.min != peak.max {
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(x - 0.45, top), pos2(x + 0.45, bottom.max(top + 0.6))),
                colors.accent,
            );
        }
        path.push(pos2(x, (top + bottom) * 0.5));
    }
    clipped.add(mesh);
    if path.len() > 1 {
        clipped.add(egui::Shape::line(path, Stroke::new(1.1, colors.accent)));
    } else if let Some(point) = path.first() {
        clipped.circle_filled(*point, 1.75, colors.accent);
    }
}

pub(super) fn range(
    painter: &Painter,
    rect: Rect,
    domain: &Range<u64>,
    selected: &Range<u64>,
    color: Color32,
) {
    let left = x_at(rect, domain, selected.start).clamp(rect.left(), rect.right());
    let right = x_at(rect, domain, selected.end).clamp(rect.left(), rect.right());
    painter.rect_filled(
        Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
        0.0,
        color.gamma_multiply(0.30),
    );
}

pub(super) fn marker(painter: &Painter, rect: Rect, domain: &Range<u64>, row: u64, color: Color32) {
    if domain.contains(&row) {
        let x = x_at(rect, domain, row);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.45, color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_trace_ticks_follow_source_rows_across_a_clock_wrap() {
        let frames = [u32::MAX - 10, 7, 30].map(|timestamp_ticks| FrameInfo {
            row: 0,
            timestamp_ticks,
            duration_ticks: 1,
            direction: Direction::Reader,
            data_len: 1,
        });
        let frames: Vec<_> = frames
            .into_iter()
            .enumerate()
            .map(|(row, frame)| FrameInfo {
                row: row as u64,
                ..frame
            })
            .collect();
        let bins = trace_preview(&frames);
        let axis = TraceAxis::new(&bins);
        assert_eq!(axis.domain(), 0..42);
        assert_eq!(axis.pick(&bins, 0.5), 1);
        assert_eq!(axis.for_row(&bins, 1, Some(frames[1])), 18);
    }
}
