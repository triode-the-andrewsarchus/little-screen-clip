//! Shared power, spectrum, and sample-coordinate drawing for I/Q views.
use super::spectrum::strongest_bin;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, pos2};
use little_clip_core::engineering::{
    iq::{PowerOverview, Spectrum},
    waveforms::{display_axis_value, display_value},
};
use std::ops::Range;

const FONT: FontId = FontId::monospace(10.0);

pub(super) const SIGNAL: Color32 = Color32::from_rgb(113, 218, 230);

pub(super) fn sample_at(rect: Rect, span: &Range<u64>, x: f32) -> u64 {
    sample_fraction(span, ((x - rect.left()) / rect.width()).clamp(0.0, 1.0))
}

pub(super) fn sample_fraction(span: &Range<u64>, fraction: f32) -> u64 {
    span.start
        + (f64::from(fraction.clamp(0.0, 1.0)) * (span.end - span.start) as f64).round() as u64
}

pub(super) fn x_at(rect: Rect, span: &Range<u64>, sample: u64) -> f32 {
    let fraction = sample.saturating_sub(span.start).min(span.end - span.start) as f64
        / (span.end - span.start).max(1) as f64;
    egui::lerp(rect.x_range(), fraction as f32)
}

pub(super) fn sample_label(sample: u64, rate: Option<f64>) -> String {
    rate.map_or_else(
        || format!("{sample} samples"),
        |rate| display_value(sample as f64 / rate, "s"),
    )
}

pub(super) fn power_scale(overview: &PowerOverview) -> Range<f32> {
    let peak = overview
        .bins
        .iter()
        .filter_map(|bin| bin.peak_dbfs)
        .fold(-100.0_f32, f32::max);
    let low = overview
        .bins
        .iter()
        .filter_map(|bin| bin.mean_dbfs)
        .fold(peak, f32::min);
    let high = peak + 3.0;
    low.min(high - 18.0).max(high - 95.0)..high
}

fn y_at(rect: Rect, power: &Range<f32>, dbfs: f32) -> f32 {
    let fraction = ((dbfs - power.start) / (power.end - power.start)).clamp(0.0, 1.0);
    egui::lerp(rect.y_range(), 1.0 - fraction)
}

pub(super) fn draw_activity(
    painter: &Painter,
    rect: Rect,
    span: &Range<u64>,
    overview: &PowerOverview,
    scale: &Range<f32>,
    threshold: Option<f32>,
) {
    let paint = painter.with_clip_rect(rect);
    let mut mean = Vec::with_capacity(overview.bins.len());
    for bin in &overview.bins {
        if bin.samples.end <= span.start || bin.samples.start >= span.end {
            continue;
        }
        let sample = bin.samples.start + (bin.samples.end - bin.samples.start) / 2;
        let x = x_at(rect, span, sample);
        if let (Some(minimum), Some(peak)) = (bin.minimum_dbfs, bin.peak_dbfs) {
            let alpha = if bin.exact() { 0.42 } else { 0.25 };
            paint.line_segment(
                [
                    pos2(x, y_at(rect, scale, minimum)),
                    pos2(x, y_at(rect, scale, peak)),
                ],
                Stroke::new(1.0, SIGNAL.gamma_multiply(alpha)),
            );
        }
        if let Some(value) = bin.mean_dbfs {
            mean.push(pos2(x, y_at(rect, scale, value)));
        } else if mean.len() > 1 {
            paint.add(egui::Shape::line(
                std::mem::take(&mut mean),
                Stroke::new(1.0, SIGNAL),
            ));
        }
    }
    if mean.len() > 1 {
        paint.add(egui::Shape::line(mean, Stroke::new(1.0, SIGNAL)));
    }
    if let Some(level) = threshold.filter(|level| scale.contains(level)) {
        paint.hline(
            rect.x_range(),
            y_at(rect, scale, level),
            Stroke::new(0.75, Color32::from_rgb(237, 153, 82)),
        );
    }
}

pub(super) fn selection(
    painter: &Painter,
    rect: Rect,
    span: &Range<u64>,
    range: &Range<u64>,
    color: Color32,
) {
    if range.end <= span.start || range.start >= span.end {
        return;
    }
    let left = x_at(rect, span, range.start);
    let right = x_at(rect, span, range.end)
        .max(left + 1.0)
        .min(rect.right());
    let overlay = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom()));
    painter.rect_filled(overlay, 0.0, color.gamma_multiply(0.18));
    for x in [left, right] {
        painter.vline(x, rect.y_range(), Stroke::new(1.0, color));
    }
}

pub(super) fn time_ticks(
    painter: &Painter,
    rect: Rect,
    span: &Range<u64>,
    rate: Option<f64>,
    color: Color32,
) {
    for (fraction, align) in [
        (0.0, Align2::LEFT_TOP),
        (0.5, Align2::CENTER_TOP),
        (1.0, Align2::RIGHT_TOP),
    ] {
        let at = sample_fraction(span, fraction);
        painter.text(
            pos2(egui::lerp(rect.x_range(), fraction), rect.bottom() + 2.0),
            align,
            sample_label(at, rate),
            FONT,
            color,
        );
    }
}

pub(super) fn spectrum(
    painter: &Painter,
    rect: Rect,
    frame: &Spectrum,
    frequency_cursor: Option<f32>,
    crossed: Option<Pos2>,
    ink: Color32,
    density: bool,
) {
    let shift = if density {
        frame
            .noise_bandwidth_hz
            .map_or(0.0, |bandwidth| (-10.0 * bandwidth.log10()) as f32)
    } else {
        0.0
    };
    let unit = if density { "dBFS/Hz" } else { "dBFS" };
    let finite = frame
        .bins_dbfs
        .iter()
        .copied()
        .filter(|value| value.is_finite());
    let peak = finite.clone().fold(-180.0_f32, f32::max) + shift;
    let scale = (peak - 85.0)..(peak + 4.0);
    for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let y = egui::lerp(rect.y_range(), fraction);
        painter.hline(
            rect.x_range(),
            y,
            Stroke::new(0.5, ink.gamma_multiply(0.22)),
        );
    }
    let paint = painter.with_clip_rect(rect);
    let bins = &frame.bins_dbfs;
    let columns = column_count(rect, bins.len());
    let plot = (0..columns)
        .filter_map(|column| {
            let bin = column_peak(bins, columns, column)?;
            let x = egui::lerp(rect.x_range(), bin_fraction(column, columns));
            Some(pos2(x, y_at(rect, &scale, bins[bin] + shift)))
        })
        .collect::<Vec<_>>();
    if plot.len() > 1 {
        paint.add(egui::Shape::line(plot, Stroke::new(1.25, SIGNAL)));
    }
    if let Some(cursor) = frequency_cursor {
        paint.vline(
            egui::lerp(
                rect.x_range(),
                plotted_bin_fraction(rect, bin_at_fraction(cursor, bins.len()), bins.len()),
            ),
            rect.y_range(),
            Stroke::new(1.0, Color32::GOLD),
        );
    }
    if let Some(hover) = crossed {
        paint.vline(hover.x, rect.y_range(), Stroke::new(0.6, ink));
    }
    frequency_ticks(painter, rect, frame, true, ink);
    painter.text(
        rect.left_top() + egui::vec2(3.0, 3.0),
        Align2::LEFT_TOP,
        format!("{:.0} {unit}", scale.end),
        FONT,
        ink,
    );
}

fn column_count(rect: Rect, bins: usize) -> usize {
    bins.min(rect.width().floor().max(1.0) as usize)
}

fn column_peak(bins: &[f32], columns: usize, column: usize) -> Option<usize> {
    let first = column * bins.len() / columns;
    let end = (column + 1) * bins.len() / columns;
    strongest_bin(bins, first..end)
}

pub(super) fn bin_at_fraction(fraction: f32, bins: usize) -> usize {
    ((fraction * bins as f32) as usize).min(bins.saturating_sub(1))
}

pub(super) fn bin_fraction(bin: usize, bins: usize) -> f32 {
    (bin as f32 + 0.5) / bins.max(1) as f32
}

pub(super) fn displayed_bin(rect: Rect, bins: &[f32], fraction: f32) -> Option<usize> {
    let columns = column_count(rect, bins.len());
    if columns == 0 {
        return None;
    }
    column_peak(bins, columns, bin_at_fraction(fraction, columns))
}

pub(super) fn plotted_bin_fraction(rect: Rect, bin: usize, count: usize) -> f32 {
    let columns = column_count(rect, count);
    if columns == 0 {
        return 0.0;
    }
    // Locate the displayed column that owns this FFT bin.
    let column = ((bin + 1) * columns - 1) / count;
    bin_fraction(column, columns)
}

pub(super) fn frequency_ticks(
    painter: &Painter,
    rect: Rect,
    frame: &Spectrum,
    absolute: bool,
    ink: Color32,
) {
    for (fraction, align) in [
        (0.0, Align2::LEFT_TOP),
        (0.5, Align2::CENTER_TOP),
        (1.0, Align2::RIGHT_TOP),
    ] {
        if fraction == 0.5 && rect.width() < 255.0 {
            continue;
        }
        let bin = bin_at_fraction(fraction, frame.bins_dbfs.len());
        painter.text(
            pos2(egui::lerp(rect.x_range(), fraction), rect.bottom() + 2.0),
            align,
            frequency_label(frame, bin, absolute),
            FONT,
            ink,
        );
    }
}

pub(super) fn frequency_label(spectrum: &Spectrum, bin: usize, absolute: bool) -> String {
    let size = spectrum.bins_dbfs.len().max(1) as f64;
    let hz = absolute
        .then(|| spectrum.frequency_hz_at(bin))
        .flatten()
        .or_else(|| spectrum.relative_hz_at(bin));
    match (hz, spectrum.sample_rate_hz) {
        (Some(hz), Some(rate)) => display_axis_value(hz, rate / size, "Hz"),
        _ => {
            let normalized = bin as f64 / size - 0.5;
            format!("{normalized:+.5} cy/smp")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_selects_the_peak_rendered_in_each_decimated_column() {
        let bins = [-12.0, -6.0, -90.0, -72.0, -40.0, -28.0, -34.0];
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(3.0, 40.0));
        for bin in 0..bins.len() {
            let expected = if bin < 2 {
                1
            } else if bin < 4 {
                3
            } else {
                5
            };
            assert_eq!(
                displayed_bin(rect, &bins, plotted_bin_fraction(rect, bin, bins.len())),
                Some(expected)
            );
        }
        assert_eq!(displayed_bin(rect, &bins, 1.0), Some(5));
    }

    #[test]
    fn absolute_labels_distinguish_adjacent_bins_at_a_large_center_frequency() {
        let frame = Spectrum {
            samples: 0..512,
            sample_rate_hz: Some(32_768.0),
            center_hz: Some(915_000_000.0),
            crosses_tuning: false,
            finite_samples: 512,
            noise_bandwidth_hz: None,
            bins_dbfs: vec![0.0; 512],
        };
        assert_eq!(frequency_label(&frame, 320, true), "915.0041 MHz");
        assert_ne!(
            frequency_label(&frame, 320, true),
            frequency_label(&frame, 321, true)
        );
        assert_eq!(frequency_label(&frame, 320, false), "4.096 kHz");
    }
}
