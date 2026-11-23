//! Shared power, spectrum, and sample-coordinate drawing for I/Q views.
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, pos2};
use little_clip_core::engineering::{
    iq::{PowerOverview, Spectrum},
    waveforms::display_value,
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
    let size = frame.bins_dbfs.len().max(1);
    let points = rect.width().ceil().clamp(1.0, size as f32) as usize;
    let plot = (0..points)
        .filter_map(|index| {
            let left = index * size / points;
            let right = (index + 1) * size / points;
            let power = frame.bins_dbfs[left..right]
                .iter()
                .filter(|value| value.is_finite())
                .fold(None, |peak: Option<f32>, &value| {
                    Some(peak.map_or(value, |peak| peak.max(value)))
                })?;
            let x = egui::lerp(
                rect.x_range(),
                index as f32 / points.saturating_sub(1).max(1) as f32,
            );
            Some(pos2(x, y_at(rect, &scale, power + shift)))
        })
        .collect::<Vec<_>>();
    if plot.len() > 1 {
        paint.add(egui::Shape::line(plot, Stroke::new(1.25, SIGNAL)));
    }
    if let Some(cursor) = frequency_cursor {
        paint.vline(
            egui::lerp(rect.x_range(), cursor),
            rect.y_range(),
            Stroke::new(1.0, Color32::GOLD),
        );
    }
    if let Some(hover) = crossed {
        paint.vline(hover.x, rect.y_range(), Stroke::new(0.6, ink));
    }
    for (fraction, align) in [
        (0.0, Align2::LEFT_TOP),
        (0.5, Align2::CENTER_TOP),
        (1.0, Align2::RIGHT_TOP),
    ] {
        let bin = ((frame.bins_dbfs.len() - 1) as f32 * fraction).round() as usize;
        painter.text(
            pos2(egui::lerp(rect.x_range(), fraction), rect.bottom() + 2.0),
            align,
            frequency_label(frame, bin),
            FONT,
            ink,
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(3.0, 3.0),
        Align2::LEFT_TOP,
        format!("{:.0} {unit}", scale.end),
        FONT,
        ink,
    );
}

pub(super) fn frequency_label(spectrum: &Spectrum, bin: usize) -> String {
    spectrum
        .frequency_hz_at(bin)
        .or_else(|| spectrum.relative_hz_at(bin))
        .map_or_else(
            || {
                let normalized = bin as f64 / spectrum.bins_dbfs.len().max(1) as f64 - 0.5;
                format!("{normalized:+.3} cy/smp")
            },
            |hz| display_value(hz, "Hz"),
        )
}
