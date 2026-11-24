//! Interactive viewer panels for SPICE netlists (`.cir`, `.sp`, `.spice`),
//! SPICE/LTspice simulation waveforms (`.raw`), and LTspice plot settings (`.plt`).

use super::ViewerState;
use crate::ui_appearance::WorkbenchColors;
use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke, Ui, pos2, vec2};
use little_clip_core::eda::spice::{
    LtspicePlotDocument, RawAnalysisKind, SpiceNetlistDocument, SpiceRawDocument, SpiceRawPlot,
    TRACE_PALETTE,
};

pub(super) fn render_netlist_details(
    ui: &mut Ui,
    net: &SpiceNetlistDocument,
    state: &mut ViewerState,
) {
    ui.strong(&net.title);
    ui.label(format!(
        "Nodes: {} · Components: {}",
        net.node_count, net.component_count
    ));
    ui.label(format!(
        "Subcircuits: {} · Models: {}",
        net.subcircuits.len(),
        net.models.len()
    ));

    if !net.category_counts.is_empty() {
        ui.separator();
        ui.strong("Components by Type");
        for (cat, count) in &net.category_counts {
            ui.label(format!("{cat}: {count}"));
        }
    }
    if !net.analyses.is_empty() {
        ui.separator();
        ui.strong("Analysis Directives");
        for a in &net.analyses {
            ui.add(egui::Label::new(a).wrap());
        }
    }
    if !net.subcircuits.is_empty() {
        ui.separator();
        ui.strong(format!("Subcircuits ({})", net.subcircuits.len()));
        for sub in net.subcircuits.iter().take(64) {
            ui.add(
                egui::Label::new(format!(
                    ".subckt {} ({}) · {} items",
                    sub.name,
                    sub.ports.join(" "),
                    sub.element_count
                ))
                .wrap(),
            );
        }
    }
    if !net.models.is_empty() {
        ui.separator();
        ui.strong(format!("Models ({})", net.models.len()));
        for m in net.models.iter().take(64) {
            ui.label(format!("{} ({})", m.name, m.kind));
        }
    }
    if !net.includes.is_empty() {
        ui.separator();
        ui.strong("External Includes (not opened)");
        for inc in &net.includes {
            ui.add(egui::Label::new(inc).wrap());
        }
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Filter:");
        ui.text_edit_singleline(&mut state.filter_query);
    });
    let q = state.filter_query.trim().to_ascii_lowercase();
    for comp in net.components.iter().take(200) {
        if !q.is_empty()
            && !comp.name.to_ascii_lowercase().contains(&q)
            && !comp.kind.to_ascii_lowercase().contains(&q)
        {
            continue;
        }
        ui.label(format!(
            "{} [{}] ({})",
            comp.name,
            comp.kind,
            comp.nodes.join(", ")
        ));
    }
}

pub(super) fn render_raw_details(ui: &mut Ui, raw: &SpiceRawDocument, state: &mut ViewerState) {
    if raw.plots.len() > 1 {
        ui.strong("Simulation Plots");
        for (idx, p) in raw.plots.iter().enumerate() {
            let selected = state.selected_plot == idx;
            let label = format!("#{} {} ({})", idx + 1, p.kind.label(), p.plot_name);
            if ui.selectable_label(selected, label).clicked() && !selected {
                state.selected_plot = idx;
                state.hidden_traces.clear();
            }
        }
        ui.separator();
    }

    let Some(plot) = raw.plots.get(state.selected_plot).or_else(|| raw.primary()) else {
        ui.label("No plots found in .raw file.");
        return;
    };

    ui.strong(&plot.title);
    ui.label(format!("Analysis: {}", plot.kind.label()));
    if !plot.plot_name.is_empty() {
        ui.add(egui::Label::new(&plot.plot_name).wrap());
    }
    ui.label(format!(
        "Points: {} · Vars: {} · Axis: {}",
        plot.total_points, plot.total_vars, plot.x_name
    ));
    if plot.is_complex {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.show_phase, false, "Magnitude (dB)");
            ui.selectable_value(&mut state.show_phase, true, "Phase (deg)");
        });
    }

    ui.separator();
    ui.strong(format!("Traces ({})", plot.traces.len()));
    for (idx, trace) in plot.traces.iter().enumerate() {
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let mut visible = !state.hidden_traces.contains(&idx);
        let summary = if plot.kind == RawAnalysisKind::DcOperatingPoint {
            let val = trace.values.first().copied().unwrap_or(0.0);
            format!("{} = {}", trace.name, fmt_num(val))
        } else {
            format!(
                "{} [{}..{}]",
                trace.name,
                fmt_num(trace.min),
                fmt_num(trace.max)
            )
        };
        ui.horizontal(|ui| {
            let (swatch, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(swatch, 2.0, Color32::from_rgb(r, g, b));
            if ui.checkbox(&mut visible, summary).changed() {
                if visible {
                    state.hidden_traces.remove(&idx);
                } else {
                    state.hidden_traces.insert(idx);
                }
            }
        });
    }
}

pub(super) fn render_plt_details(ui: &mut Ui, plt: &LtspicePlotDocument) {
    ui.strong("LTspice Plot Settings");
    ui.label("Waveform samples are stored in the .raw file.");
    ui.label(format!("Analysis: {}", plt.analysis));
    ui.label(format!(
        "Panes: {} · Traces: {}",
        plt.panes.len(),
        plt.trace_count
    ));
    for (idx, pane) in plt.panes.iter().enumerate() {
        ui.separator();
        ui.strong(format!(
            "Pane {} (log X: {}, log Y: {})",
            idx + 1,
            pane.log_x,
            pane.log_y
        ));
        for tr in &pane.traces {
            ui.label(tr);
        }
    }
}

pub(super) fn paint_raw_center(
    ui: &mut Ui,
    rect: Rect,
    raw: &SpiceRawDocument,
    state: &ViewerState,
    colors: WorkbenchColors,
) -> String {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, Color32::from_rgb(16, 20, 26));

    let Some(plot) = raw.plots.get(state.selected_plot).or_else(|| raw.primary()) else {
        return "Empty SPICE rawfile".into();
    };
    if plot.kind == RawAnalysisKind::DcOperatingPoint || plot.x_values.len() <= 1 {
        paint_op_table(&painter, rect, plot, colors);
        return format!("{} · {} signals", plot.kind.label(), plot.traces.len());
    }

    let (x_mapped, xmin, xmax, log_x) = compute_x_mapping(plot);
    let (ymin, ymax) = compute_y_bounds(plot, state);
    let pad = 28.0_f32;
    let plot_rect = Rect::from_min_max(rect.min + vec2(pad, 14.0), rect.max - vec2(14.0, pad));
    if plot_rect.width() <= 4.0 || plot_rect.height() <= 4.0 {
        return "Drag to pan · scroll to zoom".into();
    }

    paint_scope_grid(&painter, plot_rect);
    paint_trace_curves(
        &painter, plot_rect, plot, &x_mapped, xmin, xmax, ymin, ymax, state,
    );
    let range = |a, b, offset, size| {
        let middle = (a + b) * 0.5 - f64::from(offset / (size * state.zoom)) * (b - a);
        let half = (b - a) * 0.5 / f64::from(state.zoom);
        (middle - half, middle + half)
    };
    let (left, right) = range(xmin, xmax, state.pan.x, plot_rect.width());
    let (bottom, top) = range(ymin, ymax, -state.pan.y, plot_rect.height());
    paint_axis_labels(&painter, plot_rect, left, right, bottom, top, log_x, colors);
    hover_readout(ui, &painter, plot_rect, plot, &x_mapped, xmin, xmax, state)
}

fn compute_x_mapping(plot: &SpiceRawPlot) -> (Vec<f64>, f64, f64, bool) {
    let log_x = plot.kind == RawAnalysisKind::AcSweep && plot.x_values.iter().all(|&x| x > 0.0);
    let mapped: Vec<f64> = plot
        .x_values
        .iter()
        .map(|&x| if log_x { x.log10() } else { x })
        .collect();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &x in &mapped {
        if x.is_finite() {
            lo = lo.min(x);
            hi = hi.max(x);
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        (mapped, 0.0, 1.0, log_x)
    } else {
        (mapped, lo, hi, log_x)
    }
}

fn trace_y_slice(trace: &little_clip_core::eda::spice::SpiceRawTrace, phase: bool) -> &[f64] {
    if phase && let Some(ph) = &trace.phase_deg {
        return ph;
    }
    &trace.values
}

fn compute_y_bounds(plot: &SpiceRawPlot, state: &ViewerState) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (idx, tr) in plot.traces.iter().enumerate() {
        if state.hidden_traces.contains(&idx) {
            continue;
        }
        for &v in trace_y_slice(tr, state.show_phase) {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (-1.0, 1.0);
    }
    if (hi - lo).abs() < 1e-9 {
        (lo - 1.0, hi + 1.0)
    } else {
        (lo, hi)
    }
}

fn map_point(plot_rect: Rect, u: f64, v: f64, zoom: f32, pan: egui::Vec2) -> Pos2 {
    let center = plot_rect.center() + pan;
    let x = center.x + ((u as f32 - 0.5) * plot_rect.width() * zoom);
    let y = center.y + ((0.5 - v as f32) * plot_rect.height() * zoom);
    pos2(x, y)
}

fn paint_scope_grid(painter: &egui::Painter, rect: Rect) {
    let grid = Stroke::new(1.0, Color32::from_rgb(38, 46, 58));
    painter.rect_stroke(rect, 0.0, grid, egui::StrokeKind::Inside);
    for i in 1..4 {
        let f = i as f32 / 4.0;
        let x = rect.left() + rect.width() * f;
        let y = rect.top() + rect.height() * f;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], grid);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], grid);
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_trace_curves(
    painter: &egui::Painter,
    plot_rect: Rect,
    plot: &SpiceRawPlot,
    x_mapped: &[f64],
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
    state: &ViewerState,
) {
    let xspan = (xmax - xmin).max(1e-12);
    let yspan = (ymax - ymin).max(1e-12);
    let scope_painter = painter.with_clip_rect(plot_rect);

    for (idx, tr) in plot.traces.iter().enumerate() {
        if state.hidden_traces.contains(&idx) {
            continue;
        }
        let ys = trace_y_slice(tr, state.show_phase);
        let n = x_mapped.len().min(ys.len());
        if n < 2 {
            continue;
        }
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let stroke = Stroke::new(1.7, Color32::from_rgb(r, g, b));
        let pts: Vec<Pos2> = (0..n)
            .filter(|&i| x_mapped[i].is_finite() && ys[i].is_finite())
            .map(|i| {
                let u = (x_mapped[i] - xmin) / xspan;
                let v = (ys[i] - ymin) / yspan;
                map_point(plot_rect, u, v, state.zoom, state.pan)
            })
            .collect();
        for seg in pts.windows(2) {
            scope_painter.line_segment([seg[0], seg[1]], stroke);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_axis_labels(
    painter: &egui::Painter,
    rect: Rect,
    xmin: f64,
    xmax: f64,
    ymin: f64,
    ymax: f64,
    log_x: bool,
    colors: WorkbenchColors,
) {
    let x0 = if log_x { 10_f64.powf(xmin) } else { xmin };
    let x1 = if log_x { 10_f64.powf(xmax) } else { xmax };
    let font = FontId::proportional(10.0);
    painter.text(
        pos2(rect.left(), rect.bottom() + 4.0),
        Align2::LEFT_TOP,
        format!("{x0:.3e}"),
        font.clone(),
        colors.text_secondary,
    );
    painter.text(
        pos2(rect.right(), rect.bottom() + 4.0),
        Align2::RIGHT_TOP,
        format!("{x1:.3e}"),
        font.clone(),
        colors.text_secondary,
    );
    painter.text(
        pos2(rect.left() + 4.0, rect.top() + 2.0),
        Align2::LEFT_TOP,
        fmt_num(ymax),
        font.clone(),
        colors.text_secondary,
    );
    painter.text(
        pos2(rect.left() + 4.0, rect.bottom() - 2.0),
        Align2::LEFT_BOTTOM,
        fmt_num(ymin),
        font,
        colors.text_secondary,
    );
}

fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    if v != 0.0 && (v.abs() >= 1e4 || v.abs() < 1e-3) {
        format!("{v:.3e}")
    } else {
        format!("{v:.4}")
    }
}

fn paint_op_table(
    painter: &egui::Painter,
    rect: Rect,
    plot: &SpiceRawPlot,
    colors: WorkbenchColors,
) {
    let mut y = rect.top() + 14.0;
    for (idx, tr) in plot.traces.iter().take(24).enumerate() {
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let val = tr.values.first().copied().unwrap_or(0.0);
        painter.text(
            pos2(rect.left() + 16.0, y),
            Align2::LEFT_TOP,
            format!("{:<24} = {} {}", tr.name, fmt_num(val), tr.unit_type),
            FontId::monospace(12.0),
            Color32::from_rgb(r, g, b),
        );
        y += 20.0;
        if y + 16.0 > rect.bottom() {
            break;
        }
    }
    painter.text(
        pos2(rect.left() + 16.0, rect.bottom() - 16.0),
        Align2::LEFT_BOTTOM,
        "DC Operating Point",
        FontId::proportional(11.0),
        colors.text_secondary,
    );
}

#[allow(clippy::too_many_arguments)]
fn hover_readout(
    ui: &egui::Ui,
    painter: &egui::Painter,
    plot_rect: Rect,
    plot: &SpiceRawPlot,
    x_mapped: &[f64],
    xmin: f64,
    xmax: f64,
    state: &ViewerState,
) -> String {
    let default_hint = "Drag to pan · scroll to zoom · hover waveforms for readout".to_string();
    let Some(pos) = super::super::ui::preview_hovered(ui, plot_rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten()
    else {
        return default_hint;
    };
    let center = plot_rect.center() + state.pan;
    let u = (pos.x - center.x) / (plot_rect.width() * state.zoom) + 0.5;
    if !(0.0..=1.0).contains(&u) {
        return default_hint;
    }
    let target_x = xmin + f64::from(u) * (xmax - xmin).max(1e-12);
    let Some(idx) = nearest_sample(x_mapped, target_x) else {
        return default_hint;
    };
    let x_actual = plot.x_values[idx];

    let sx = map_point(
        plot_rect,
        (x_mapped[idx] - xmin) / (xmax - xmin).max(1e-12),
        0.5,
        state.zoom,
        state.pan,
    )
    .x;
    if sx >= plot_rect.left() && sx <= plot_rect.right() {
        painter.line_segment(
            [pos2(sx, plot_rect.top()), pos2(sx, plot_rect.bottom())],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(200, 215, 235, 110)),
        );
    }

    let mut samples = Vec::new();
    for (t_idx, tr) in plot.traces.iter().enumerate() {
        if state.hidden_traces.contains(&t_idx) {
            continue;
        }
        let ys = trace_y_slice(tr, state.show_phase);
        if let Some(&yv) = ys.get(idx) {
            let unit = if plot.is_complex {
                if state.show_phase { "°" } else { " dB" }
            } else {
                ""
            };
            samples.push(format!("{}={}{unit}", tr.name, fmt_num(yv)));
        }
        if samples.len() >= 4 {
            break;
        }
    }
    if samples.is_empty() {
        format!("{}={x_actual:.4e}", plot.x_name)
    } else {
        format!("{}={x_actual:.4e} · {}", plot.x_name, samples.join("  "))
    }
}

fn nearest_sample(values: &[f64], target: f64) -> Option<usize> {
    values
        .iter()
        .enumerate()
        .filter(|(_, value)| value.is_finite())
        .min_by(|(_, a), (_, b)| (*a - target).abs().total_cmp(&(*b - target).abs()))
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_selects_nearest_sample_across_descending_and_restarted_sweeps() {
        let values = [3.0, 1.0, f64::NAN, -4.0, 4.0, 0.0];
        assert_eq!(nearest_sample(&values, 2.5), Some(0));
        assert_eq!(nearest_sample(&values, 0.4), Some(5));
        assert_eq!(nearest_sample(&[f64::NAN], 1.0), None);
    }
}
