use super::Task;
use crate::{
    file_manager::{
        eda::{
            eda_pan_zoom_canvas,
            raster_task::{REPAINT_INTERVAL, RasterTask, poll_current},
            spice::{paint_axis_labels, paint_ordinate_labels, paint_scope_grid, raw_plot_rect},
        },
        scope_view::plot::labelled_x_axis,
    },
    file_source::FileSource,
    ui_appearance::WorkbenchColors,
    ui_ext::PlotAxis,
};
use egui::{Color32, Rect, Stroke, Ui, Vec2, pos2, vec2};
use little_clip_core::{
    BackgroundJobs,
    eda::spice::{
        RawAnalysisKind, SpicePreviewSample,
        raw_compare::{RawComparison, Selection, Sweep, compatible_axes, residual},
    },
};
use std::{sync::LazyLock, task::Poll};

static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));
const COLORS: [Color32; 3] = [
    Color32::from_rgb(248, 165, 100),
    Color32::from_rgb(109, 216, 251),
    Color32::from_rgb(179, 223, 132),
];

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
enum Mode {
    #[default]
    Overlay,
    A,
    B,
    Residual,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct TraceKey {
    analysis: usize,
    signal: usize,
    sweep: usize,
    phase: bool,
    mode: Mode,
}

#[derive(Default)]
pub(super) struct View {
    pair: Option<[usize; 2]>,
    pending: RasterTask<[usize; 2], RawComparison>,
    data: Option<Result<RawComparison, String>>,
    selection: TraceKey,
    graph: Option<(TraceKey, Result<Graph, &'static str>)>,
    pan: Vec2,
    zoom: f32,
}

impl View {
    pub(super) fn cancel_pending(&mut self) {
        self.pending = None;
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        key: [usize; 2],
        files: [&FileSource; 2],
        names: [&str; 2],
        colors: WorkbenchColors,
    ) {
        self.update(ui.ctx(), key, files);
        let data = match &self.data {
            Some(Ok(data)) => data,
            Some(Err(error)) => {
                ui.label(error);
                return;
            }
            None => {
                ui.label("Reading waveform pair…");
                return;
            }
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(COLORS[0], format!("A  {}", names[0]));
            ui.colored_label(COLORS[1], format!("B  {}", names[1]));
        });
        selection_controls(ui, data, &mut self.selection);
        if self
            .graph
            .as_ref()
            .is_none_or(|(key, _)| *key != self.selection)
        {
            let next = Graph::build(data, self.selection);
            let compatible = match (&self.graph, &next) {
                (Some((old, Ok(previous))), Ok(next)) => {
                    old.analysis == self.selection.analysis
                        && old.signal == self.selection.signal
                        && old.sweep == self.selection.sweep
                        && old.phase == self.selection.phase
                        && previous.bounds == next.bounds
                }
                _ => false,
            };
            if !compatible {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            self.graph = Some((self.selection, next));
        }
        if let Some((_, graph)) = &self.graph {
            match graph {
                Ok(graph) => graph.paint(ui, colors, &mut self.zoom, &mut self.pan),
                Err(reason) => {
                    ui.label(*reason);
                }
            }
        }
    }

    fn update(&mut self, ctx: &egui::Context, key: [usize; 2], files: [&FileSource; 2]) {
        if self.pair != Some(key) {
            self.pair = Some(key);
            self.data = None;
            self.graph = None;
            self.selection = TraceKey::default();
        }
        if self.data.is_some() {
            return;
        }
        match poll_current(ctx, &mut self.pending, &key) {
            Poll::Ready(Some(data)) => self.data = Some(data),
            Poll::Pending => {}
            Poll::Ready(None) => {
                let files = files.map(FileSource::clone);
                self.pending = Some((
                    key,
                    Task::start_unbounded(&JOBS, move |cancel| {
                        RawComparison::load(files.each_ref(), cancel)
                    }),
                ));
                ctx.request_repaint_after(REPAINT_INTERVAL);
            }
        }
    }
}

fn selection_label(selection: &Selection) -> String {
    let name = &selection.name;
    match selection.indices {
        [Some(_), Some(_)] => name.clone(),
        [Some(_), None] => format!("{name} (A only)"),
        _ => format!("{name} (B only)"),
    }
}

fn pick(ui: &mut Ui, label: &str, selected: &mut usize, choices: impl Iterator<Item = String>) {
    let choices: Vec<_> = choices.collect();
    egui::ComboBox::from_label(label)
        .selected_text(choices.get(*selected).map_or("—", String::as_str))
        .show_ui(ui, |ui| {
            for (index, label) in choices.into_iter().enumerate() {
                ui.selectable_value(selected, index, label);
            }
        });
}

fn selection_controls(ui: &mut Ui, data: &RawComparison, key: &mut TraceKey) {
    ui.horizontal_wrapped(|ui| {
        let analysis = key.analysis;
        pick(
            ui,
            "Analysis",
            &mut key.analysis,
            data.analyses.iter().map(|a| selection_label(&a.selection)),
        );
        if key.analysis != analysis {
            key.signal = 0;
            key.sweep = 0;
            key.phase = false;
        }
        let Some(analysis) = data.analyses.get(key.analysis) else {
            return;
        };
        let signal = key.signal;
        pick(
            ui,
            "Signal",
            &mut key.signal,
            analysis.signals.iter().map(selection_label),
        );
        if key.signal != signal {
            key.sweep = 0;
        }
        let count = traces(data, *key)
            .iter()
            .flatten()
            .map(|(_, trace)| trace.sweeps().count())
            .max()
            .unwrap_or(0);
        if count > 1 {
            pick(
                ui,
                "Sweep",
                &mut key.sweep,
                (1..=count).map(|n| format!("Sweep {n}")),
            );
        }
    });
    if key.sweep > 0
        || traces(data, *key)
            .iter()
            .flatten()
            .any(|(_, t)| t.sweeps().nth(1).is_some())
    {
        ui.weak("Sweeps match by ordinal. Step parameter values are unavailable.");
    }
    ui.horizontal_wrapped(|ui| {
        for (mode, name) in [
            (Mode::Overlay, "Overlay"),
            (Mode::A, "A"),
            (Mode::B, "B"),
            (Mode::Residual, "B − A"),
        ] {
            clip_ui::buttons::choice(ui, &mut key.mode, mode, name);
        }
        if traces(data, *key)
            .iter()
            .flatten()
            .any(|(plot, _)| plot.is_complex)
        {
            ui.checkbox(&mut key.phase, "Phase");
        }
    });
}

fn traces(
    data: &RawComparison,
    key: TraceKey,
) -> [Option<(
    &little_clip_core::eda::spice::SpiceRawPlot,
    &little_clip_core::eda::spice::SpiceRawTrace,
)>; 2] {
    [0, 1].map(|side| {
        let analysis = data.analyses.get(key.analysis)?;
        let plot = &data.documents[side].plots[analysis.selection.indices[side]?];
        let signal = analysis.signals.get(key.signal)?.indices[side]?;
        Some((plot, &plot.traces[signal]))
    })
}

struct Graph {
    curves: Vec<(Color32, Vec<[f64; 2]>)>,
    bounds: [[f64; 2]; 2],
    log_x: bool,
    x_unit: String,
    caption: String,
    reduction: bool,
    missing_side: bool,
}

impl Graph {
    fn build(data: &RawComparison, key: TraceKey) -> Result<Self, &'static str> {
        let all = traces(data, key).map(|selection| {
            let (plot, trace) = selection?;
            let samples = trace.sweeps().nth(key.sweep)?;
            (!samples.is_empty()).then_some(Sweep {
                plot,
                trace,
                samples,
            })
        });
        let pair = match key.mode {
            Mode::A => [all[0], None],
            Mode::B => [None, all[1]],
            _ => all,
        };
        let first = pair[0]
            .or(pair[1])
            .ok_or("This side has no samples for the selected sweep")?;
        let comparable = matches!(all, [Some(a), Some(b)] if compatible_axes(a, b));
        let scale_from = if comparable { all } else { pair };
        let log_x = key.mode != Mode::Residual
            && first.plot.kind == RawAnalysisKind::AcSweep
            && scale_from
                .iter()
                .flatten()
                .all(|sweep| sweep.samples.iter().all(|point| point.x > 0.0));
        let reduction = pair.iter().flatten().any(|sweep| sweep.partial());
        let y_unit = match (first.plot.is_complex, key.phase) {
            (true, true) => "°",
            (true, false) => "dB",
            _ => first.trace.unit_type.as_str(),
        };
        let axis = format!("{} ({}) · {}", first.plot.x_name, first.plot.x_unit, y_unit);
        let (mut curves, caption) = if key.mode == Mode::Residual {
            let [Some(a), Some(b)] = pair else {
                return Err("B − A requires the same sweep on both sides");
            };
            (
                vec![(COLORS[2], residual(a, b)?)],
                format!("B − A · {axis} · linear interpolation in the shared domain"),
            )
        } else {
            if key.mode == Mode::Overlay && !comparable && all.iter().all(Option::is_some) {
                return Err(
                    "The axes or signal units differ. Select A or B to inspect each waveform.",
                );
            }
            (curves(scale_from, key.phase, log_x), axis)
        };
        let bounds =
            bounds(&curves).ok_or("The selected waveform exceeds the plot's numeric range")?;
        match key.mode {
            Mode::A => curves.retain(|(color, _)| *color == COLORS[0]),
            Mode::B => curves.retain(|(color, _)| *color == COLORS[1]),
            _ => {}
        }
        Ok(Self {
            curves,
            bounds,
            log_x,
            x_unit: match first.plot.x_unit.as_str() {
                "time" => "s",
                "frequency" => "Hz",
                "voltage" => "V",
                "current" => "A",
                unit => unit,
            }
            .into(),
            caption,
            reduction,
            missing_side: key.mode == Mode::Overlay && pair.iter().any(Option::is_none),
        })
    }

    fn paint(&self, ui: &mut Ui, colors: WorkbenchColors, zoom: &mut f32, pan: &mut Vec2) {
        ui.label(&self.caption);
        if self.reduction {
            ui.weak("Preview uses retained samples. Some source samples were reduced or missing.");
        }
        if self.missing_side {
            ui.weak("No corresponding sweep samples on the other side.");
        }
        let (rect, response) = ui.allocate_exact_size(
            vec2(
                ui.available_width(),
                (ui.available_height() - 24.0).max(48.0),
            ),
            egui::Sense::click_and_drag(),
        );
        let plot = raw_plot_rect(rect);
        if !plot.is_positive() {
            return;
        }
        eda_pan_zoom_canvas(ui, plot, plot.center(), &response, zoom, pan);
        let drawn = Rect::from_center_size(plot.center() + *pan, plot.size() * *zoom);
        let [x, y] = [
            PlotAxis::new(self.bounds[0], [drawn.left(), drawn.right()]),
            PlotAxis::new(self.bounds[1], [drawn.bottom(), drawn.top()]),
        ];
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 3.0, Color32::from_rgb(16, 20, 26));
        paint_scope_grid(&painter, plot);
        if self.log_x {
            paint_axis_labels(&painter, plot, [x, y], true, colors);
        } else {
            let domain = x.value_at(plot.left())..x.value_at(plot.right());
            labelled_x_axis(&painter, plot, &domain, &self.x_unit, colors.text_secondary);
            paint_ordinate_labels(&painter, plot, y, colors.text_secondary);
        }
        let painter = painter.with_clip_rect(plot);
        for (color, points) in &self.curves {
            let screen = |&[a, b]: &[f64; 2]| pos2(x.screen_at(a), y.screen_at(b));
            if let [point] = points.as_slice() {
                painter.circle_filled(screen(point), 3.0, *color);
            } else {
                painter.add(egui::Shape::line(
                    points.iter().map(screen).collect(),
                    Stroke::new(1.6, *color),
                ));
            }
        }
        if let Some(pointer) = response
            .hover_pos()
            .filter(|pointer| plot.contains(*pointer))
        {
            painter.vline(pointer.x, plot.y_range(), Stroke::new(1.0, Color32::GRAY));
            let x = x.value_at(pointer.x);
            let x = if self.log_x { 10_f64.powf(x) } else { x };
            ui.label(format!("X {x:.6e}"));
        }
    }
}

fn curves(pair: [Option<Sweep<'_>>; 2], phase: bool, log_x: bool) -> Vec<(Color32, Vec<[f64; 2]>)> {
    let mut curves = Vec::new();
    for (sweep, color) in pair.into_iter().zip(COLORS) {
        let Some(sweep) = sweep else {
            continue;
        };
        for run in sweep.samples.chunk_by(SpicePreviewSample::shares_run_with) {
            curves.push((
                color,
                run.iter()
                    .map(|point| {
                        [
                            if log_x { point.x.log10() } else { point.x },
                            point.ordinate(phase && sweep.plot.is_complex),
                        ]
                    })
                    .collect(),
            ));
        }
    }
    curves
}

fn bounds(curves: &[(Color32, Vec<[f64; 2]>)]) -> Option<[[f64; 2]; 2]> {
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for (axis, span) in bounds.iter_mut().enumerate() {
        for point in curves.iter().flat_map(|(_, points)| points) {
            span[0] = span[0].min(point[axis]);
            span[1] = span[1].max(point[axis]);
        }
        if span[0] == span[1] {
            let pad = (span[0].abs() * 0.02).max(1e-12);
            *span = [span[0] - pad, span[1] + pad];
        }
        if !(span[1] - span[0]).is_finite() {
            return None;
        }
    }
    Some(bounds)
}
