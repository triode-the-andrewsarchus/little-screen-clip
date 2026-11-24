//! Interactive viewer panels for SPICE netlists (`.cir`, `.sp`, `.spice`),
//! SPICE/LTspice simulation waveforms (`.raw`), and LTspice plot settings (`.plt`).

use super::ViewerState;
use crate::ui_appearance::WorkbenchColors;
use crate::ui_ext::PlotAxis;
use egui::{Align2, Color32, FontId, Rect, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
use little_clip_core::eda::spice::{
    LtspicePlotDocument, RawAnalysisKind, SpiceNetlistDocument, SpicePreviewSample,
    SpiceRawDocument, SpiceRawPlot, TRACE_PALETTE,
};

pub(super) fn render_netlist_details(ui: &mut Ui, net: &SpiceNetlistDocument) {
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
}

pub(super) fn render_connectivity(
    ui: &mut Ui,
    net: &SpiceNetlistDocument,
    state: &mut ViewerState,
) {
    ui.label("Top-level netlist connectivity");
    if net.components.len() < net.component_count {
        ui.weak(format!(
            "{} of {} element records retained",
            net.components.len(),
            net.component_count
        ));
    }
    ui.add(
        egui::TextEdit::singleline(&mut state.filter_query)
            .hint_text("Filter by element, type or node"),
    );
    let filter = state.filter_query.trim().to_ascii_lowercase();
    let rows: Vec<_> = net
        .components
        .iter()
        .filter(|element| {
            filter.is_empty()
                || element.name.to_ascii_lowercase().contains(&filter)
                || element.kind.to_ascii_lowercase().contains(&filter)
                || element
                    .nodes
                    .iter()
                    .any(|node| node.to_ascii_lowercase().contains(&filter))
        })
        .collect();
    let height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    TableBuilder::new(ui)
        .id_salt("netlist connectivity")
        .striped(true)
        .columns(Column::auto().clip(true), 2)
        .column(Column::remainder().clip(true))
        .header(height, |mut row| {
            for title in ["Element", "Type", "Nodes in card order"] {
                row.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(height, rows.len(), |mut row| {
                let element = rows[row.index()];
                let nodes = element.nodes.join(" · ");
                for value in [&element.name, element.kind, &nodes] {
                    row.col(|ui| {
                        ui.add(egui::Label::new(value).truncate())
                            .on_hover_text(value);
                    });
                }
            });
        });
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
    if plot.x_name.is_empty() {
        ui.label(format!("{} signals", plot.total_vars));
    } else {
        ui.label(format!(
            "Points: {} · Vars: {} · Axis: {}",
            plot.total_points, plot.total_vars, plot.x_name
        ));
    }
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
            let val = trace.samples().first().map_or(0.0, |s| s.value);
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
    ui.label("Waveform samples are stored in the .raw file.");
    ui.label(format!(
        "Panes: {} · Traces: {}",
        plt.pane_count(),
        plt.trace_count
    ));
    for section in &plt.sections {
        ui.separator();
        ui.strong(&section.analysis);
        for (idx, pane) in section.panes.iter().enumerate() {
            ui.label(format!(
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
}

pub(in crate::file_manager) fn raw_plot_rect(rect: Rect) -> Rect {
    Rect::from_min_max(rect.min + vec2(28.0, 14.0), rect.max - vec2(14.0, 28.0))
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
    if plot.kind == RawAnalysisKind::DcOperatingPoint {
        paint_op_table(&painter, rect, plot, state, colors);
        return format!("{} · {} signals", plot.kind.label(), plot.traces.len());
    }

    let Some(x_range) = plot.x_range else {
        return format!("{} · No finite axis samples", plot.kind.label());
    };
    let log_x = plot.log_x();
    let [xmin, xmax] = x_range.map(|x| axis_position(x, log_x));
    let (ymin, ymax) = compute_y_bounds(plot, state);
    let plot_rect = raw_plot_rect(rect);
    if plot_rect.width() <= 4.0 || plot_rect.height() <= 4.0 {
        return "Drag to pan · scroll to zoom".into();
    }

    let drawn = Rect::from_center_size(
        plot_rect.center() + state.pan,
        plot_rect.size() * state.zoom,
    );
    let x = PlotAxis::new([xmin, xmax], [drawn.left(), drawn.right()]);
    let y = PlotAxis::new([ymin, ymax], [drawn.bottom(), drawn.top()]);
    paint_scope_grid(&painter, plot_rect);
    paint_trace_curves(&painter, plot_rect, plot, [x, y], state);
    paint_axis_labels(&painter, plot_rect, [x, y], log_x, colors);
    hover_readout(ui, &painter, plot_rect, plot, (x, [xmin, xmax]), state)
}

fn axis_position(x: f64, log_x: bool) -> f64 {
    if log_x { x.log10() } else { x }
}

fn compute_y_bounds(plot: &SpiceRawPlot, state: &ViewerState) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (idx, tr) in plot.traces.iter().enumerate() {
        if state.hidden_traces.contains(&idx) {
            continue;
        }
        for sample in tr.samples() {
            let v = sample.ordinate(state.show_phase && plot.is_complex);
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

pub(in crate::file_manager) fn paint_scope_grid(painter: &egui::Painter, rect: Rect) {
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

fn paint_trace_curves(
    painter: &egui::Painter,
    plot_rect: Rect,
    plot: &SpiceRawPlot,
    [x, y]: [PlotAxis; 2],
    state: &ViewerState,
) {
    let scope_painter = painter.with_clip_rect(plot_rect);
    let phase = state.show_phase && plot.is_complex;
    let log_x = plot.log_x();
    let point = |sample: &SpicePreviewSample| {
        pos2(
            x.screen_at(axis_position(sample.x, log_x)),
            y.screen_at(sample.ordinate(phase)),
        )
    };

    for (idx, tr) in plot.traces.iter().enumerate() {
        if state.hidden_traces.contains(&idx) {
            continue;
        }
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let stroke = Stroke::new(1.7, Color32::from_rgb(r, g, b));
        for run in tr
            .sweeps()
            .flat_map(|sweep| sweep.chunk_by(SpicePreviewSample::shares_run_with))
        {
            if let [sample] = run {
                scope_painter.circle_filled(point(sample), stroke.width * 0.5, stroke.color);
            } else {
                scope_painter.add(egui::Shape::line(run.iter().map(point).collect(), stroke));
            }
        }
    }
}

pub(in crate::file_manager) fn paint_axis_labels(
    painter: &egui::Painter,
    rect: Rect,
    [x, y]: [PlotAxis; 2],
    log_x: bool,
    colors: WorkbenchColors,
) {
    let [x0, x1] = [rect.left(), rect.right()].map(|pixel| {
        let value = x.value_at(pixel);
        if log_x { 10_f64.powf(value) } else { value }
    });
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
        font,
        colors.text_secondary,
    );
    paint_ordinate_labels(painter, rect, y, colors.text_secondary);
}

pub(in crate::file_manager) fn paint_ordinate_labels(
    painter: &egui::Painter,
    rect: Rect,
    y: PlotAxis,
    color: Color32,
) {
    let font = FontId::proportional(10.0);
    painter.text(
        pos2(rect.left() + 4.0, rect.top() + 2.0),
        Align2::LEFT_TOP,
        fmt_num(y.value_at(rect.top())),
        font.clone(),
        color,
    );
    painter.text(
        pos2(rect.left() + 4.0, rect.bottom() - 2.0),
        Align2::LEFT_BOTTOM,
        fmt_num(y.value_at(rect.bottom())),
        font,
        color,
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
    state: &ViewerState,
    colors: WorkbenchColors,
) {
    let mut y = rect.top() + 14.0;
    for (idx, tr) in plot
        .traces
        .iter()
        .enumerate()
        .filter(|(i, _)| !state.hidden_traces.contains(i))
        .take(24)
    {
        let [r, g, b] = TRACE_PALETTE[idx % TRACE_PALETTE.len()];
        let val = tr.samples().first().map_or(0.0, |s| s.value);
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

fn hover_readout(
    ui: &egui::Ui,
    painter: &egui::Painter,
    plot_rect: Rect,
    plot: &SpiceRawPlot,
    (x, [xmin, xmax]): (PlotAxis, [f64; 2]),
    state: &ViewerState,
) -> String {
    let default_hint = "Drag to pan · scroll to zoom · hover waveforms for readout".to_string();
    let Some(pos) = super::super::ui::preview_hovered(ui, plot_rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten()
    else {
        return default_hint;
    };
    let target_x = x.value_at(pos.x);
    if !(xmin..=xmax).contains(&target_x) {
        return default_hint;
    }
    let log_x = plot.log_x();
    let Some(reference) = plot
        .traces
        .iter()
        .enumerate()
        .find(|(idx, tr)| !state.hidden_traces.contains(idx) && !tr.samples().is_empty())
        .map(|(_, tr)| tr.samples())
    else {
        return default_hint;
    };
    let Some(index) = nearest_sample(
        reference.iter().map(|s| axis_position(s.x, log_x)),
        target_x,
    ) else {
        return default_hint;
    };
    let sample = reference[index];
    let x_actual = sample.x;

    let sx = x.screen_at(axis_position(x_actual, log_x));
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
        // Peak reduction can retain different source rows for different
        // traces. A readout compares values from the same row and sweep.
        if let Some(value) = tr.sample_at(sample.source_index) {
            let yv = value.ordinate(state.show_phase && plot.is_complex);
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

fn nearest_sample(values: impl Iterator<Item = f64>, target: f64) -> Option<usize> {
    values
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
        assert_eq!(nearest_sample(values.into_iter(), 2.5), Some(0));
        assert_eq!(nearest_sample(values.into_iter(), 0.4), Some(5));
        assert_eq!(nearest_sample([f64::NAN].into_iter(), 1.0), None);
    }
}
