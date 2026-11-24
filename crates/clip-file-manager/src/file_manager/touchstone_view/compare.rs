//! Compare two measurements on a shared frequency axis with exact record links.

mod band;
mod data;
mod panels;

use super::{
    antenna,
    capture::{Band, Trace},
    marker_value,
    plot::{self, Projection},
};
use crate::{file_source::ReadCheck, ui_appearance::WorkbenchColors};
use band::MatchBand;
use data::{Element, Reads, Request, Source, TraceKeys};
use egui::{Button, Color32, PointerButton, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::{
    engineering::touchstone::{Parameter, TracePoint},
    palette::AppPalette,
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

fn sweep_colors(ui: &Ui) -> [Color32; 2] {
    if ui.visuals().dark_mode {
        [
            Color32::from_rgb(92, 193, 247),
            Color32::from_rgb(243, 170, 102),
        ]
    } else {
        [
            Color32::from_rgb(4, 105, 161),
            Color32::from_rgb(149, 78, 14),
        ]
    }
}

pub(in crate::file_manager) struct Viewer {
    sources: [Source; 2],
    order: [usize; 2],
    selected: Element,
    matrix_side: usize,
    matrix: Trace,
    traces: TraceKeys,
    prefix: [Vec<TracePoint>; 2],
    reads: Reads,
    tools: panels::Tools,
    full: Band,
    viewport: Band,
    marker: f64,
    projection: Projection,
    antenna: bool,
    match_band: MatchBand,
}

impl Viewer {
    pub(in crate::file_manager) fn load(
        paths: Vec<PathBuf>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let [a, b]: [PathBuf; 2] = paths
            .try_into()
            .map_err(|_| "Select exactly two RF sweeps")?;
        let sources = [Source::load(a, cancel)?, Source::load(b, cancel)?];
        let selected =
            Element::at(&sources[0], [0, 0]).ok_or("The first sweep has no response axes")?;
        let traces = sources.each_ref().map(|source| selected.trace(source));
        let bands = std::array::from_fn(|side| traces[side].map(|_| sources[side].band()));
        let full = extent(bands);
        let marker = overlap(bands).map_or(full[0], |band| band[0]);
        let prefix = data::prefix(&sources, traces);
        let antenna = sources.iter().all(|source| source.metadata().ports == 1)
            && sources
                .iter()
                .any(|source| marker_value(source.metadata(), [0, 0]).is_some());
        Ok(Self {
            sources,
            order: [0, 1],
            selected,
            matrix_side: 0,
            matrix: [0, 0],
            traces,
            prefix,
            reads: Reads::default(),
            tools: panels::Tools::new(full),
            full,
            viewport: full,
            marker,
            projection: preferred(selected.parameter),
            antenna,
            match_band: MatchBand::default(),
        })
    }

    pub(in crate::file_manager) fn set_check(&mut self, check: ReadCheck) {
        for source in &mut self.sources {
            if let Some(capture) = Arc::get_mut(&mut source.capture) {
                capture.check = check.clone();
            }
        }
    }

    pub(in crate::file_manager) fn ui(
        &mut self,
        ui: &mut Ui,
        close: &mut bool,
        palette: &AppPalette,
    ) {
        let colors = WorkbenchColors::from_palette(palette);
        let mut shell = crate::shell::StudioShell::new(
            "RF sweep comparison",
            crate::icons::Icon::Crosshair,
            palette,
            close,
        )
        .with_session_title();
        shell.render_header(ui, |_| {});
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 24.0;
            ui.spacing_mut().button_padding.y = 2.0;
            self.workspace(ui, colors);
        });
    }

    fn workspace(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        self.source_controls(ui);
        self.trace_controls(ui);
        let measured = self.indexed_bands();
        let available = self.available_bands(measured);
        self.update_extent(extent(available));
        let common = overlap(measured);
        let compatible = self.compatible();
        self.frequency_controls(ui, common, compatible);
        self.match_readout(ui, measured);
        let enough = compatible && common.is_some_and(|band| band[1] > band[0]);
        if enough || !self.antenna_open() || self.tools.active() {
            self.tools.controls(
                ui,
                enough && self.selected.parameter == Parameter::S,
                enough,
            );
        }
        let available_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt("RF comparison")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let linked = self.antenna_open();
                if linked {
                    let top = ui.cursor().min.y;
                    self.details(ui, common, compatible);
                    let readout_height = ui.cursor().min.y - top;
                    let charts = (available_height - readout_height).max(244.0);
                    self.graphs(ui, (charts * 0.52).min(360.0), common, compatible, colors);
                    self.antenna_companions(ui, (charts * 0.46).min(290.0), colors);
                } else {
                    let (ratio, ceiling) = if self.tools.active() {
                        (0.42, 248.0)
                    } else {
                        (0.51, 330.0)
                    };
                    self.graphs(
                        ui,
                        (available_height * ratio).clamp(116.0, ceiling),
                        common,
                        compatible,
                        colors,
                    );
                }
                self.match_band.finish_frame(ui);
                if !linked {
                    self.details(ui, common, compatible);
                }
            });
    }

    fn source_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for (slot, source) in self.order.into_iter().enumerate() {
                let side = &self.sources[source];
                ui.colored_label(
                    sweep_colors(ui)[slot],
                    format!("{}  {}", panels::letter(slot), side.name().display()),
                )
                .on_hover_text(side.path.to_string_lossy());
            }
            if ui.button("Swap A/B").clicked() {
                self.order.swap(0, 1);
            }
            let mut mhz = self.marker * 1e-6;
            if ui
                .add(
                    egui::DragValue::new(&mut mhz)
                        .range(self.full[0] * 1e-6..=self.full[1] * 1e-6)
                        .max_decimals(9)
                        .speed(0.5)
                        .prefix("Marker ")
                        .suffix(" MHz")
                        .update_while_editing(false),
                )
                .changed()
            {
                self.jump(mhz * 1e6);
            }
        });
    }

    fn trace_controls(&mut self, ui: &mut Ui) {
        if self.sources.iter().all(|source| {
            source.metadata().ports == 1 && Element::at(source, [0, 0]) == Some(self.selected)
        }) {
            return;
        }
        let mut element = self.selected;
        if self
            .sources
            .iter()
            .all(|source| source.metadata().ports <= 4)
        {
            ui.horizontal_wrapped(|ui| {
                let mut seen = Vec::new();
                for source in self.order {
                    let metadata = self.sources[source].metadata();
                    for response in 0..metadata.ports {
                        for stimulus in 0..metadata.ports {
                            let trace = [response, stimulus];
                            let Some(candidate) = Element::at(&self.sources[source], trace) else {
                                continue;
                            };
                            if seen.contains(&candidate) {
                                continue;
                            }
                            seen.push(candidate);
                            let label = metadata
                                .parameter_label(response, stimulus)
                                .unwrap_or_default();
                            let label = self.label_availability(candidate, label);
                            if ui
                                .add(Button::new(label).selected(candidate == self.selected))
                                .clicked()
                            {
                                element = candidate;
                            }
                        }
                    }
                }
            });
        } else {
            element = self.matrix_controls(ui);
        }
        self.select(element);
    }

    fn label_availability(&self, element: Element, label: String) -> String {
        let [a, b] = self
            .order
            .map(|side| element.trace(&self.sources[side]).is_some());
        match [a, b] {
            [true, false] => format!("{label} · A only"),
            [false, true] => format!("{label} · B only"),
            _ => label,
        }
    }

    fn matrix_controls(&mut self, ui: &mut Ui) -> Element {
        ui.horizontal_wrapped(|ui| {
            for (slot, side) in self.order.into_iter().enumerate() {
                if ui
                    .add(
                        Button::new(format!("{} matrix", panels::letter(slot)))
                            .selected(self.matrix_side == side),
                    )
                    .clicked()
                {
                    self.matrix_side = side;
                    self.matrix = [0, 0];
                }
            }
            let source = &self.sources[self.matrix_side];
            let metadata = source.metadata();
            for (label, axis) in ["Response", "Stimulus"].into_iter().zip(&mut self.matrix) {
                let mut port = *axis + 1;
                ui.label(label);
                ui.add(egui::DragValue::new(&mut port).range(1..=metadata.ports));
                *axis = port - 1;
            }
            let [response, stimulus] = self.matrix;
            if let Some(label) = metadata.parameter_label(response, stimulus)
                && let Some(candidate) = Element::at(source, self.matrix)
            {
                ui.strong(self.label_availability(candidate, label));
            }
        });
        Element::at(&self.sources[self.matrix_side], self.matrix).unwrap_or(self.selected)
    }

    fn select(&mut self, next: Element) {
        if self.selected != next {
            if self.selected.parameter != next.parameter {
                self.projection = preferred(next.parameter);
            }
            self.selected = next;
            self.traces = self.sources.each_ref().map(|source| next.trace(source));
            if self.projection == Projection::Vswr && !self.has_antenna() {
                self.projection = preferred(next.parameter);
            }
            self.prefix = data::prefix(&self.sources, self.traces);
        }
    }

    fn indexed_bands(&self) -> [Option<Band>; 2] {
        std::array::from_fn(|side| {
            self.reads
                .indexed(self.traces, side)?
                .ok()
                .map(|data| data.index.band_hz())
        })
    }

    fn available_bands(&self, indexed: [Option<Band>; 2]) -> [Option<Band>; 2] {
        std::array::from_fn(|side| {
            indexed[side].or_else(|| self.traces[side].map(|_| self.sources[side].band()))
        })
    }

    fn update_extent(&mut self, new: Band) {
        if self.full != new {
            self.viewport = if self.viewport == self.full {
                new
            } else {
                plot::fit_band(self.viewport, new)
            };
            self.full = new;
            self.marker = self.marker.clamp(new[0], new[1]);
        }
    }

    fn compatible(&self) -> bool {
        self.traces.iter().all(Option::is_some)
            && self
                .selected
                .axes
                .same_basis(self.sources[0].metadata(), self.sources[1].metadata())
    }

    fn frequency_controls(&mut self, ui: &mut Ui, common: Option<Band>, compatible: bool) {
        let has_antenna = self.has_antenna();
        let db_label = self.decibel_label(has_antenna);
        ui.horizontal_wrapped(|ui| {
            if self.selected.parameter == Parameter::S {
                ui.selectable_value(&mut self.projection, Projection::Decibels, db_label);
            }
            for projection in [Projection::Magnitude, Projection::Phase] {
                ui.selectable_value(&mut self.projection, projection, projection.title());
            }
            if has_antenna {
                antenna::controls(ui, &mut self.antenna, &mut self.projection, true);
            }
            if ui.button("Fit sweeps").clicked() {
                self.viewport = self.full;
            }
            if ui
                .add_enabled(common.is_some(), Button::new("Fit overlap"))
                .clicked()
                && let Some(band) = common
            {
                self.viewport = plot::fit_band(plot::pad_single(band), self.full);
            }
        });
        self.coverage(ui, common, compatible);
        if self.antenna_open() {
            self.match_band.controls(ui, &mut self.viewport, self.full);
        }
    }

    fn decibel_label(&self, has_antenna: bool) -> String {
        if has_antenna {
            for side in self.order {
                if let Some([response, stimulus]) = self.traces[side]
                    && let Some(parameter) = self.sources[side]
                        .metadata()
                        .parameter_label(response, stimulus)
                {
                    return format!("{parameter} dB");
                }
            }
        }
        Projection::Decibels.title().to_owned()
    }

    fn coverage(&self, ui: &mut Ui, common: Option<Band>, compatible: bool) {
        ui.horizontal_wrapped(|ui| {
            for (slot, side) in self.order.into_iter().enumerate() {
                let letter = panels::letter(slot);
                let Some(_) = self.traces[side] else {
                    ui.weak(format!("{letter} has no matching trace"));
                    continue;
                };
                match self.reads.indexed(self.traces, side) {
                    Some(Ok(indexed)) => {
                        let [lo, hi] = indexed.index.band_hz();
                        let reference = self
                            .reference(side)
                            .map_or_else(String::new, |reference| format!(" · {reference} Ω ref"));
                        ui.weak(format!(
                            "{letter}  {}–{}{reference}",
                            antenna::frequency(lo),
                            antenna::frequency(hi)
                        ));
                    }
                    Some(Err(error)) => {
                        ui.colored_label(ui.visuals().error_fg_color, format!("{letter}  {error}"));
                    }
                    None => {
                        ui.weak(format!("Indexing {letter}…"));
                    }
                }
            }
            if let Some([lo, hi]) = common {
                ui.label(format!(
                    "Overlap  {}–{}",
                    antenna::frequency(lo),
                    antenna::frequency(hi)
                ));
            } else if self.indexed_bands().iter().all(Option::is_some) {
                ui.weak("No shared frequency coverage");
            }
            if !compatible && self.traces.iter().all(Option::is_some) {
                ui.weak("Measurement bases differ · Deltas unavailable");
            }
        });
    }

    fn match_readout(&mut self, ui: &mut Ui, measured: [Option<Band>; 2]) {
        let linked = self.antenna_open();
        let band = self.match_band.selected.unwrap_or(self.viewport);
        self.reads
            .update_best(ui.ctx(), &self.sources, self.traces, linked.then_some(band));
        if linked
            && let Some(hz) = panels::best_match(
                ui,
                self.order,
                self.reads.best(self.traces, band),
                band,
                self.match_band.selected.is_some(),
                measured,
            )
        {
            self.jump(hz);
        }
    }

    fn details(&mut self, ui: &mut Ui, common: Option<Band>, compatible: bool) {
        let matched = common.filter(|_| compatible);
        let linked = self.antenna_open();
        let threshold = self.tools.threshold(
            matched.filter(|_| self.selected.parameter == Parameter::S),
            self.viewport,
            self.marker,
        );
        let aperture = self.tools.aperture(matched, self.marker);
        self.reads.update(
            ui.ctx(),
            &self.sources,
            Request {
                traces: self.traces,
                viewport: self.viewport,
                marker: self.marker,
                thresholds: threshold,
                aperture,
            },
        );
        let metric_jump = matched.and_then(|_| {
            self.tools.metrics(
                ui,
                panels::Metrics {
                    reads: &self.reads,
                    traces: self.traces,
                    order: self.order,
                    threshold,
                    aperture,
                    marker: self.marker,
                },
            )
        });
        if self.tools.active() {
            ui.separator();
        }
        let samples = self.reads.marker(self.traces, self.marker);
        let marker_jump = panels::marker(
            ui,
            &self.sources,
            self.order,
            self.traces,
            samples,
            compatible,
            self.projection,
            linked,
        );
        if let Some(hz) = metric_jump.or(marker_jump) {
            self.jump(hz);
            ui.ctx().request_repaint();
        }
    }

    fn reference(&self, side: usize) -> Option<f64> {
        marker_value(self.sources[side].metadata(), self.traces[side]?)
    }

    fn has_antenna(&self) -> bool {
        self.traces.iter().any(Option::is_some)
            && (0..2).all(|side| self.traces[side].is_none() || self.reference(side).is_some())
    }

    fn antenna_open(&self) -> bool {
        self.antenna && antenna::primary(self.projection) && self.has_antenna()
    }

    fn antenna_companions(&mut self, ui: &mut Ui, height: f32, colors: WorkbenchColors) {
        let ink = sweep_colors(ui);
        let sweeps: Vec<_> = self
            .order
            .into_iter()
            .enumerate()
            .filter_map(|(slot, side)| {
                Some(antenna::Sweep {
                    points: self.trace_data(side),
                    reference: self.reference(side)?,
                    color: ink[slot],
                })
            })
            .collect();
        let mut markers = Vec::with_capacity(2);
        if let Some(Ok(samples)) = self.reads.marker(self.traces, self.marker) {
            for (slot, side) in self.order.into_iter().enumerate() {
                if let (Some(Ok(Some(sample))), Some(reference)) =
                    (&samples[side], self.reference(side))
                {
                    markers.push(antenna::Marker {
                        hz: sample.frequency_hz,
                        value: sample.value,
                        reference,
                        label: panels::letter(slot),
                        color: ink[slot],
                        exact: sample.exact(),
                    });
                }
            }
        }
        let interaction = antenna::companions(
            ui,
            antenna::Window {
                visible: self.viewport,
                selected: self.match_band.visible_selection(),
            },
            height,
            &sweeps,
            &markers,
            &[(self.marker, "", colors.text_primary)],
            colors,
        );
        if let Some(picked) = interaction.smith_pick {
            self.jump(picked.frequency_hz);
        }
        self.navigate(ui, interaction.frequency, &interaction.frequency_hit);
    }

    fn trace_data(&self, source: usize) -> &[TracePoint] {
        self.reads
            .data(self.traces, self.viewport, source)
            .unwrap_or(&self.prefix[source])
    }

    fn graphs(
        &mut self,
        ui: &mut Ui,
        height: f32,
        common: Option<Band>,
        compatible: bool,
        colors: WorkbenchColors,
    ) {
        if self.traces.iter().all(Option::is_some) && !compatible && !self.antenna_open() {
            for (slot, side) in self.order.into_iter().enumerate() {
                ui.colored_label(sweep_colors(ui)[slot], self.axis_title(side, true));
                self.graph(ui, (height * 0.56).max(112.0), common, colors, Some(slot));
            }
        } else {
            let side = self
                .order
                .into_iter()
                .find(|&side| self.traces[side].is_some());
            if let Some(side) = side
                && self.projection == Projection::Magnitude
                && self.selected.parameter != Parameter::S
            {
                ui.weak(self.axis_title(side, false));
            }
            self.graph(ui, height, common, colors, None);
        }
    }

    fn axis_title(&self, side: usize, include_source: bool) -> String {
        let source = &self.sources[side];
        let unit = self.traces[side].map_or("", |trace| source.magnitude_unit(trace));
        let title = self.projection.title();
        let scale = if self.projection == Projection::Magnitude && !unit.is_empty() {
            format!("{title} ({unit})")
        } else {
            title.to_owned()
        };
        if include_source {
            let slot = usize::from(self.order[1] == side);
            format!("{}  {scale}", panels::letter(slot))
        } else {
            scale
        }
    }

    fn graph(
        &mut self,
        ui: &mut Ui,
        height: f32,
        common: Option<Band>,
        colors: WorkbenchColors,
        only: Option<usize>,
    ) {
        let (outer, frame) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let rect = Rect::from_min_max(outer.min + vec2(58.0, 8.0), outer.max - vec2(10.0, 24.0));
        if !rect.is_positive() {
            return;
        }
        let response = ui.interact(
            rect,
            frame.id.with("paired frequency trace"),
            Sense::click_and_drag(),
        );
        self.navigate(ui, rect, &response);
        let painter = ui.painter_at(outer);
        painter.rect_filled(rect, 3.0, colors.recessed);
        if let Some(band) = common
            && band != self.full
        {
            plot::highlight(
                &painter,
                rect,
                self.viewport,
                band,
                colors.text_muted.gamma_multiply(0.37),
            );
        }
        if self.antenna_open()
            && let Some(band) = self.match_band.visible_selection()
        {
            plot::highlight(&painter, rect, self.viewport, band, colors.accent);
        }
        let matched = common.filter(|_| only.is_none() && self.compatible());
        if let Some(aperture) = self.tools.aperture(matched, self.marker) {
            plot::highlight(&painter, rect, self.viewport, aperture, colors.warning);
        }
        plot::axes(&painter, rect, self.viewport, colors.text_muted);
        let values = if let Some(slot) = only {
            plot::trace(
                &painter,
                rect,
                self.viewport,
                self.trace_data(self.order[slot]),
                self.projection,
                sweep_colors(ui)[slot],
                true,
            )
        } else {
            let traces = self.order.map(|source| self.trace_data(source));
            plot::compare_traces(
                &painter,
                rect,
                self.viewport,
                traces,
                self.projection,
                sweep_colors(ui),
            )
        };
        if matched.is_some()
            && self.projection == Projection::Decibels
            && let Some(db) = self.tools.cutoff_db()
            && let Some(wanted) = plot::cutoff_handle(
                ui,
                &painter,
                rect,
                values,
                db,
                colors.warning,
                response.id.with("shared cutoff"),
            )
        {
            self.tools.set_cutoff(wanted);
        }
        plot::marker(
            &painter,
            rect,
            self.viewport,
            self.marker,
            "",
            colors.text_primary,
        );
        self.paint_marker_dots(&painter, rect, values, only, sweep_colors(ui));
        response.on_hover_text(if self.antenna_open() {
            "Shift-drag to select a match band · Click or drag the shared marker · Scroll to zoom · Middle-drag to pan"
        } else {
            "Click or drag the shared marker · Scroll to zoom · Middle-drag to pan"
        });
    }

    fn paint_marker_dots(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        values: Band,
        only: Option<usize>,
        colors: [Color32; 2],
    ) {
        let Some(Ok(samples)) = self.reads.marker(self.traces, self.marker) else {
            return;
        };
        let painter = painter.with_clip_rect(rect.expand(4.0).intersect(painter.clip_rect()));
        let x = plot::x_at(rect, self.viewport, self.marker);
        for (slot, side) in self.order.into_iter().enumerate() {
            if only.is_some_and(|only| only != slot) {
                continue;
            }
            let Some(Ok(Some(sample))) = samples[side] else {
                continue;
            };
            let value = if self.antenna_open() {
                self.projection.coordinate(sample.value)
            } else {
                self.projection.marker_value(sample)
            };
            let y = plot::fraction(values, value);
            if !(0.0..=1.0).contains(&y) || x < rect.left() || x > rect.right() {
                continue;
            }
            let position = pos2(x, rect.bottom() - y * rect.height());
            if sample.exact() {
                painter.circle_filled(position, 3.6, colors[slot]);
            } else {
                painter.circle_stroke(position, 4.0, Stroke::new(1.4, colors[slot]));
            }
        }
    }

    fn navigate(&mut self, ui: &Ui, rect: Rect, response: &Response) {
        let viewport = plot::navigate(ui, rect, response, self.viewport, self.full, true);
        if self.viewport != viewport {
            self.viewport = viewport;
            ui.ctx().request_repaint();
        }
        if self.antenna_open() && self.match_band.drag(ui, rect, response, self.viewport) {
            return;
        }
        if (response.clicked_by(PointerButton::Primary)
            || response.dragged_by(PointerButton::Primary))
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.marker = plot::hz_at(rect, self.viewport, pos.x);
        }
    }

    fn jump(&mut self, hz: f64) {
        self.marker = hz.clamp(self.full[0], self.full[1]);
        if !(self.viewport[0]..=self.viewport[1]).contains(&self.marker) {
            let half = (self.viewport[1] - self.viewport[0]) * 0.5;
            self.viewport = plot::fit_band([self.marker - half, self.marker + half], self.full);
        }
    }
}

fn preferred(parameter: Parameter) -> Projection {
    if parameter == Parameter::S {
        Projection::Decibels
    } else {
        Projection::Magnitude
    }
}

fn extent(bands: [Option<Band>; 2]) -> Band {
    let [lo, hi] = bands.into_iter().flatten().fold(
        [f64::INFINITY, f64::NEG_INFINITY],
        |[lo, hi], [start, end]| [lo.min(start), hi.max(end)],
    );
    plot::pad_single(if lo.is_finite() && hi.is_finite() {
        [lo, hi]
    } else {
        [0.0, 1.0]
    })
}

fn overlap(bands: [Option<Band>; 2]) -> Option<Band> {
    let [Some(a), Some(b)] = bands else {
        return None;
    };
    let band = [a[0].max(b[0]), a[1].min(b[1])];
    (band[0] <= band[1]).then_some(band)
}
