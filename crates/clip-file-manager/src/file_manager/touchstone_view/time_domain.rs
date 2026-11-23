use super::{Band, Capture, Job, Trace, read_job};
use crate::{
    file_manager::{
        preview_copy::Copies,
        scope_view::plot,
        signal_view::{
            frequency_plot::{amplitude_ticks, choose_window},
            take_query_result, timeline,
        },
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Color32, Context, PointerButton, Rect, Sense, Stroke, Ui, vec2};
use little_clip_core::engineering::{
    analysis::SpectrumWindow,
    touchstone::{GateMode, ImpulseResponse, Index, TracePoint},
    waveforms::{Peak, display_value},
};
use std::{ops::Range, sync::Arc, time::Duration};

pub(super) const GATE_COLOR: Color32 = Color32::from_rgb(231, 168, 88);

pub(super) struct TimeDomain {
    band: Option<Band>,
    window: SpectrumWindow,
    result: Option<(Sweep, Result<Arc<Response>, String>)>,
    read: Option<(Sweep, Job<Arc<Response>>)>,
    view: Option<Range<f64>>,
    anchor: Option<f64>,
    draft: Option<Range<f64>>,
    gate: Option<Range<f64>>,
    gate_mode: GateMode,
    gate_read: Option<(Gate, Job<Arc<Vec<TracePoint>>>)>,
    gated: Option<(Gate, Result<Arc<Vec<TracePoint>>, String>)>,
}

#[derive(Clone, PartialEq)]
struct Gate {
    range: Range<f64>,
    mode: GateMode,
}

#[derive(Clone, PartialEq)]
struct Sweep {
    trace: Trace,
    band: Band,
    window: SpectrumWindow,
}

struct Response {
    impulse: ImpulseResponse,
    magnitudes: Vec<f32>,
    bounds: Peak,
}

impl Default for TimeDomain {
    fn default() -> Self {
        Self {
            band: None,
            window: SpectrumWindow::Rectangular,
            result: None,
            read: None,
            view: None,
            anchor: None,
            draft: None,
            gate: None,
            gate_mode: GateMode::Keep,
            gate_read: None,
            gated: None,
        }
    }
}

impl TimeDomain {
    pub(super) fn gated(&self) -> Option<&[TracePoint]> {
        self.gated_points().map(|points| points.as_slice())
    }

    pub(super) fn gated_points(&self) -> Option<&Arc<Vec<TracePoint>>> {
        let (gate, result) = self.gated.as_ref()?;
        (self.gate.as_ref() == Some(&gate.range) && self.gate_mode == gate.mode)
            .then(|| result.as_ref().ok())
            .flatten()
    }

    pub(super) fn invalidate(&mut self) {
        self.suspend();
        self.result = None;
        self.view = None;
        self.gate = None;
        self.gated = None;
    }

    pub(super) fn suspend(&mut self) {
        self.read = None;
        self.gate_read = None;
        self.anchor = None;
        self.draft = None;
    }

    pub(super) fn pending(&self) -> bool {
        self.read.is_some() || self.gate_read.is_some()
    }

    pub(super) fn content(
        &mut self,
        ui: &mut Ui,
        trace: Trace,
        viewport: Band,
        full: Band,
        capture: &Arc<Capture>,
        index: Option<&Arc<Index>>,
        colors: WorkbenchColors,
        copies: &mut Copies,
    ) {
        self.controls(ui, viewport, copies);
        let Some(index) = index else {
            ui.weak("Indexing the measurement sweep…");
            return;
        };
        let key = Sweep {
            trace,
            band: self.band.unwrap_or(full),
            window: self.window,
        };
        self.update(ui.ctx(), &key, capture, index);
        let Some((_, result)) = self.result.as_ref().filter(|(ready, _)| ready == &key) else {
            ui.weak("Transforming the measured band…");
            return;
        };
        let response = match result {
            Ok(response) => response.clone(),
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error);
                return;
            }
        };
        self.plot(ui, &response, colors);
        self.update_gate(ui.ctx(), &response);
    }

    fn controls(&mut self, ui: &mut Ui, viewport: Band, copies: &mut Copies) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Band-pass impulse");
            choose_window(ui, &mut self.window);
            if ui.button("Use visible band").clicked() {
                self.band = Some(viewport);
            }
            if ui.button("Whole sweep").clicked() {
                self.band = None;
            }
            if ui.button("Fit delay").clicked() {
                self.view = None;
            }
            ui.add_enabled_ui(self.gate.is_some(), |ui| {
                egui::ComboBox::from_id_salt("delay gate mode")
                    .selected_text(match self.gate_mode {
                        GateMode::Keep => "Keep interval",
                        GateMode::Reject => "Reject interval",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.gate_mode, GateMode::Keep, "Keep interval");
                        ui.selectable_value(
                            &mut self.gate_mode,
                            GateMode::Reject,
                            "Reject interval",
                        );
                    });
            });
            if ui
                .add_enabled(self.gate.is_some(), egui::Button::new("Clear gate"))
                .clicked()
            {
                self.gate = None;
                ui.ctx().request_repaint();
            }
            if ui
                .add_enabled(
                    self.gated().is_some() && !copies.busy(),
                    egui::Button::new("Copy gated CSV"),
                )
                .clicked()
                && let Some((_, Ok(points))) = &self.gated
            {
                let points = points.clone();
                copies.start(ui.ctx(), move |_| copy_trace(&points));
            }
        });
    }

    fn update(&mut self, ctx: &Context, key: &Sweep, capture: &Arc<Capture>, index: &Arc<Index>) {
        if let Some(result) = take_query_result(&mut self.read) {
            self.result = Some(result);
        }
        if self.result.as_ref().is_some_and(|(ready, _)| ready == key)
            || self
                .read
                .as_ref()
                .is_some_and(|(loading, _)| loading == key)
        {
            return;
        }
        self.gate = None;
        self.gated = None;
        self.gate_read = None;
        self.view = None;
        let requested = key.clone();
        let capture = capture.clone();
        let index = index.clone();
        self.read = Some((
            key.clone(),
            read_job(ctx, Duration::from_secs(20), move |cancel| {
                let input =
                    capture.exact_band(&index, requested.band, requested.trace, 0, cancel)?;
                let impulse = ImpulseResponse::from_trace(&input, requested.window)
                    .map_err(|err| err.to_string())?;
                Ok(Arc::new(Response::new(impulse)))
            }),
        ));
    }

    fn update_gate(&mut self, ctx: &Context, response: &Arc<Response>) {
        if let Some(result) = take_query_result(&mut self.gate_read) {
            self.gated = Some(result);
            ctx.request_repaint();
        }
        let Some(range) = self.gate.clone() else {
            self.gate_read = None;
            self.gated = None;
            return;
        };
        let key = Gate {
            range,
            mode: self.gate_mode,
        };
        if self.gated.as_ref().is_some_and(|(ready, _)| ready == &key)
            || self
                .gate_read
                .as_ref()
                .is_some_and(|(loading, _)| loading == &key)
        {
            return;
        }
        let response = response.clone();
        let requested = key.clone();
        self.gate_read = Some((
            key,
            read_job(ctx, Duration::from_secs(10), move |_| {
                response
                    .impulse
                    .apply_gate(requested.range, requested.mode)
                    .map(Arc::new)
                    .map_err(|err| err.to_string())
            }),
        ));
    }

    fn plot(&mut self, ui: &mut Ui, response: &Response, colors: WorkbenchColors) {
        let height = (ui.available_height() - 38.0).max(88.0);
        let (area, hit) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let rect = Rect::from_min_max(area.min + vec2(64.0, 2.0), area.max - vec2(9.0, 19.0));
        let start = response.impulse.start_seconds();
        let step = response.impulse.step_seconds();
        let domain = start..start + response.magnitudes.len() as f64 * step;
        let mut span = self.view.clone().unwrap_or_else(|| domain.clone());
        if let Some(requested) = timeline::navigate(ui.ctx(), rect, &hit, &span, false) {
            span = timeline::fit(&domain, requested, step);
            self.view = Some(span.clone());
        }
        self.brush(ui, rect, &hit, &span);
        let painter = ui.painter_at(area);
        painter.rect_filled(rect, 3.0, colors.recessed);
        plot::axes(&painter, rect, &span, colors.text_muted);
        amplitude_ticks(&painter, rect, response.bounds, "|h|", colors.text_muted);
        let clipped = painter.with_clip_rect(rect);
        if let Some(gate) = self.draft.as_ref().or(self.gate.as_ref()) {
            plot::highlight(
                &clipped,
                rect,
                &span,
                gate.clone(),
                GATE_COLOR.gamma_multiply(0.25),
            );
        }
        plot::draw_raw(
            &clipped,
            rect,
            &span,
            start,
            step,
            &response.magnitudes,
            response.bounds,
            colors.accent,
        );
        let hovered = hit
            .hover_pos()
            .filter(|pointer| rect.contains(*pointer))
            .map(|pointer| {
                let time = plot::at(rect, &span, pointer.x);
                let row = ((time - start) / step)
                    .round()
                    .clamp(0.0, response.magnitudes.len().saturating_sub(1) as f64)
                    as usize;
                let x = plot::x_at(rect, &span, (row as f64).mul_add(step, start));
                clipped.vline(x, rect.y_range(), Stroke::new(1.1, colors.text_primary));
                row
            });
        self.readout(ui, response, hovered);
        hit.on_hover_text(
            "Drag to select a delay interval · Right-click to clear · Scroll to zoom",
        );
    }

    fn brush(&mut self, ui: &Ui, rect: Rect, hit: &egui::Response, span: &Range<f64>) {
        if hit.secondary_clicked() {
            self.gate = None;
            self.draft = None;
            self.anchor = None;
            ui.ctx().request_repaint();
        }
        if hit.drag_started_by(PointerButton::Primary) {
            self.draft = None;
            self.anchor = ui
                .input(|input| input.pointer.press_origin())
                .filter(|point| rect.contains(*point))
                .map(|point| plot::at(rect, span, point.x));
        }
        if let Some(anchor) = self.anchor
            && let Some(pointer) = hit.interact_pointer_pos()
        {
            let time = plot::at(rect, span, pointer.x);
            self.draft = Some(anchor.min(time)..anchor.max(time));
        }
        if hit.drag_stopped_by(PointerButton::Primary) {
            self.anchor = None;
            self.gate = self.draft.take().filter(|range| !range.is_empty());
        }
    }

    fn readout(&self, ui: &mut Ui, response: &Response, hovered: Option<usize>) {
        ui.horizontal_wrapped(|ui| {
            let dt = response.impulse.step_seconds();
            let start = response.impulse.start_seconds();
            if let Some(row) = hovered {
                ui.monospace(format!(
                    "{} · |h| {:.5}",
                    display_value((row as f64).mul_add(dt, start), "s"),
                    response.magnitudes[row]
                ));
            } else if let Some(gate) = &self.gate {
                ui.monospace(format!(
                    "{} {} to {}",
                    match self.gate_mode {
                        GateMode::Keep => "Keep",
                        GateMode::Reject => "Reject",
                    },
                    display_value(gate.start, "s"),
                    display_value(gate.end, "s")
                ));
            } else {
                let [low, high] = response.impulse.band_hz();
                ui.weak(format!(
                    "{} to {} · Δt {}",
                    display_value(low, "Hz"),
                    display_value(high, "Hz"),
                    display_value(dt, "s")
                ));
            }
            if response.impulse.interpolated() {
                ui.weak("Uniform frequency grid interpolated");
            }
            if self.gated().is_some() {
                ui.colored_label(GATE_COLOR, "Gated frequency trace");
            }
            if let Some((gate, Err(error))) = &self.gated
                && self.gate.as_ref() == Some(&gate.range)
                && self.gate_mode == gate.mode
            {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    }
}

impl Response {
    fn new(impulse: ImpulseResponse) -> Self {
        let magnitudes = impulse
            .samples()
            .iter()
            .map(|value| value.norm() as f32)
            .collect::<Vec<_>>();
        let mut bounds = Peak::default();
        bounds.add(&magnitudes);
        Self {
            impulse,
            magnitudes,
            bounds,
        }
    }
}

fn copy_trace(points: &[TracePoint]) -> Result<String, String> {
    use std::fmt::Write;
    let mut csv = String::from("frequency_hz,gated_real,gated_imaginary\n");
    for point in points {
        let _ = writeln!(
            csv,
            "{},{},{}",
            point.frequency_hz, point.value.re, point.value.im
        );
    }
    little_clip_core::copy_text_to_system_clipboard(&csv).map_err(|err| err.to_string())?;
    Ok(format!("Copied {} gated frequency points", points.len()))
}
