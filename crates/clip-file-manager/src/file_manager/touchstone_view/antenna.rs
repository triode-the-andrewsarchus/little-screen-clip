//! Linked representations of a saved reflection measurement.

use super::{
    capture::Band,
    plot::{self, Projection, Smith},
};
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, Color32, FontId, PointerButton, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::engineering::{
    touchstone::{Complex64, TracePoint},
    waveforms::{display_axis_value, display_value},
};

pub(super) struct Sweep<'a> {
    pub points: &'a [TracePoint],
    pub reference: f64,
    pub color: Color32,
}

pub(super) struct Marker<'a> {
    pub hz: f64,
    pub value: Complex64,
    pub reference: f64,
    pub label: &'a str,
    pub color: Color32,
    pub exact: bool,
}

pub(super) type Guide<'a> = (f64, &'a str, Color32);

pub(super) struct PlotInteraction {
    pub smith_pick: Option<TracePoint>,
    pub frequency: Rect,
    pub frequency_hit: Response,
}

pub(super) fn primary(projection: Projection) -> bool {
    matches!(projection, Projection::Decibels | Projection::Vswr)
}

/// Returns whether the user opened the linked layout.
pub(super) fn controls(
    ui: &mut Ui,
    enabled: &mut bool,
    projection: &mut Projection,
    available: bool,
) -> bool {
    if ui
        .add(egui::Button::new("VSWR").selected(*projection == Projection::Vswr))
        .clicked()
    {
        *projection = Projection::Vswr;
    }
    ui.separator();
    let open = *enabled && available && primary(*projection);
    if !ui
        .add_enabled(
            available,
            egui::Button::new("Antenna analysis").selected(open),
        )
        .clicked()
    {
        return false;
    }
    *enabled = !open;
    if *enabled && !primary(*projection) {
        *projection = Projection::Decibels;
    }
    *enabled
}

pub(super) fn impedance(value: Complex64, reference: f64) -> Option<Complex64> {
    let denominator = 1.0 - value;
    if denominator.norm() <= 1e-12 {
        return None;
    }
    let result = reference * (1.0 + value) / denominator;
    (result.re.is_finite() && result.im.is_finite()).then_some(result)
}

pub(super) fn frequency(hz: f64) -> String {
    display_axis_value(hz, 1.0, "Hz")
}

pub(super) fn readout(hz: f64, value: Complex64, reference: f64) -> String {
    let db = plot::decibels(value);
    let db = if db <= -160.0 {
        "≤ −160 dB".into()
    } else {
        format!("{db:.3} dB")
    };
    let vswr = Projection::Vswr.coordinate(value);
    let vswr = if vswr.is_finite() {
        format!("{vswr:.3}")
    } else {
        "—".into()
    };
    let z = impedance(value, reference).map_or_else(
        || "Z —".to_owned(),
        |z| format!("{:.3} {:+.3}j Ω", z.re, z.im),
    );
    format!("{} · {db} · VSWR {vswr} · {z}", frequency(hz))
}

pub(super) fn best_button(ui: &mut Ui, point: TracePoint) -> Response {
    let vswr = Projection::Vswr.coordinate(point.value);
    ui.small_button(format!(
        "{} · VSWR {vswr:.3}",
        frequency(point.frequency_hz)
    ))
    .on_hover_text(format!(
        "Select measured row {} at {:.6} Hz",
        point.index + 1,
        point.frequency_hz
    ))
}

pub(super) struct Window {
    pub visible: Band,
    pub selected: Option<Band>,
}

pub(super) fn companions(
    ui: &mut Ui,
    window: Window,
    height: f32,
    sweeps: &[Sweep<'_>],
    markers: &[Marker<'_>],
    guides: &[Guide<'_>],
    colors: WorkbenchColors,
) -> PlotInteraction {
    let (outer, frame) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let split = outer.left() + outer.width() * 0.33;
    let smith_rect = Rect::from_min_max(
        outer.min + vec2(0.0, 22.0),
        pos2(split - 5.0, outer.bottom() - 2.0),
    );
    let impedance_rect = Rect::from_min_max(
        pos2(split + 59.0, outer.top() + 22.0),
        outer.max - vec2(10.0, 22.0),
    );
    let painter = ui.painter_at(outer);
    for (at, title) in [
        (outer.left() + 4.0, "Smith · Z/Z₀"),
        (split + 8.0, "Impedance (Ω) · R bright, X muted"),
    ] {
        painter.text(
            pos2(at, outer.top() + 2.0),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(13.0),
            colors.text_secondary,
        );
    }
    let smith_hit = ui.interact(smith_rect, frame.id.with("Smith"), Sense::click_and_drag());
    let frequency_hit = ui.interact(
        impedance_rect,
        frame.id.with("Impedance"),
        Sense::click_and_drag(),
    );
    let smith_pick = smith_chart(
        &painter,
        smith_rect,
        smith_hit,
        window.visible,
        sweeps,
        markers,
        colors,
    );
    impedance_plot(
        &painter,
        impedance_rect,
        window,
        sweeps,
        markers,
        guides,
        colors,
    );
    PlotInteraction {
        smith_pick,
        frequency: impedance_rect,
        frequency_hit,
    }
}

fn smith_chart(
    painter: &egui::Painter,
    rect: Rect,
    hit: Response,
    band: Band,
    sweeps: &[Sweep<'_>],
    markers: &[Marker<'_>],
    colors: WorkbenchColors,
) -> Option<TracePoint> {
    painter.rect_filled(rect, 3.0, colors.recessed);
    let painter = painter.with_clip_rect(rect);
    let chart = Smith::interactive(rect, &hit);
    chart.grid(&painter, colors.text_muted);
    for sweep in sweeps {
        chart.trace(
            &painter,
            plot::visible(sweep.points, band, false),
            sweep.color,
        );
    }
    for marker in markers
        .iter()
        .filter(|marker| (band[0]..=band[1]).contains(&marker.hz))
    {
        chart.mark_sample(
            &painter,
            marker.value,
            marker.label,
            marker.color,
            marker.exact,
            marker.label == "A",
        );
    }
    let pointer = hit.hover_pos()?;
    let points = sweeps
        .iter()
        .flat_map(|sweep| plot::visible(sweep.points, band, false));
    let picked = chart.nearest(pointer, points.copied())?;
    if !hit.clicked_by(PointerButton::Primary) || hit.double_clicked_by(PointerButton::Primary) {
        hit.on_hover_text(format!(
            "Select source point · {:.6} MHz\nScroll to zoom · Middle-drag to pan · Double-click to reset",
            picked.frequency_hz * 1e-6
        ));
        return None;
    }
    Some(picked)
}

fn impedance_plot(
    painter: &egui::Painter,
    rect: Rect,
    Window {
        visible: band,
        selected,
    }: Window,
    sweeps: &[Sweep<'_>],
    markers: &[Marker<'_>],
    guides: &[Guide<'_>],
    colors: WorkbenchColors,
) {
    painter.rect_filled(rect, 3.0, colors.recessed);
    if let Some(selected) = selected {
        plot::highlight(painter, rect, band, selected, colors.accent);
    }
    plot::axes(painter, rect, band, colors.text_muted);
    let curves: Vec<_> = sweeps
        .iter()
        .map(|sweep| {
            let data = plot::visible(sweep.points, band, true);
            let mut parts = [
                Vec::with_capacity(data.len()),
                Vec::with_capacity(data.len()),
            ];
            for point in data {
                let z = impedance(point.value, sweep.reference);
                parts[0].push((point.frequency_hz, z.map_or(f64::NAN, |z| z.re)));
                parts[1].push((point.frequency_hz, z.map_or(f64::NAN, |z| z.im)));
            }
            (parts, sweep.color)
        })
        .collect();
    let values = impedance_range(band, &curves);
    plot::scale(painter, rect, values, |value| display_value(value, "Ω"));
    let inside = painter.with_clip_rect(rect);
    for ([r, x], color) in &curves {
        plot::paint_curve(
            &inside,
            rect,
            band,
            r.iter().copied(),
            values,
            false,
            *color,
        );
        plot::paint_curve(
            &inside,
            rect,
            band,
            x.iter().copied(),
            values,
            false,
            color.gamma_multiply(0.68),
        );
    }
    for &(hz, label, color) in guides {
        plot::marker(painter, rect, band, hz, label, color);
    }
    for marker in markers
        .iter()
        .filter(|marker| (band[0]..=band[1]).contains(&marker.hz))
    {
        let Some(z) = impedance(marker.value, marker.reference) else {
            continue;
        };
        for value in [z.re, z.im] {
            let y = plot::fraction(values, value);
            if (0.0..=1.0).contains(&y) {
                let at = pos2(
                    plot::x_at(rect, band, marker.hz),
                    rect.bottom() - y * rect.height(),
                );
                if marker.exact {
                    inside.circle_filled(at, 3.0, marker.color);
                } else {
                    inside.circle_stroke(at, 3.4, Stroke::new(1.3, marker.color));
                }
            }
        }
    }
}

type ImpedanceCurves = [Vec<(f64, f64)>; 2];

fn impedance_range(band: Band, curves: &[(ImpedanceCurves, Color32)]) -> Band {
    let [lo, hi] = curves
        .iter()
        .flat_map(|(parts, _)| parts.iter().flatten())
        .filter(|(hz, value)| (band[0]..=band[1]).contains(hz) && value.is_finite())
        .fold([0.0_f64, 0.0_f64], |[lo, hi], &(_, value)| {
            [lo.min(value), hi.max(value)]
        });
    let pad = ((hi - lo) * 0.05).max(0.1);
    [lo - pad, hi + pad]
}
