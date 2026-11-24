use super::{
    Job,
    capture::{Band, Capture, Cutoff, Trace},
    plot, read_job,
};
use crate::{file_manager::signal_view::take_query_result, ui_appearance::WorkbenchColors};
use egui::{Color32, Painter, Rect, Sense, Ui, pos2};
use egui_extras::{Column, TableBuilder};
use little_clip_core::engineering::{
    touchstone::{BandEdge, BandSearch, Index, ThresholdBand},
    waveforms::{display_axis_value, display_value},
};
use std::{fmt::Write, sync::Arc, time::Duration};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Below,
    Above,
    FromPeak,
}

#[derive(Default)]
pub(super) struct View {
    rule: Option<Rule>,
    inputs: Levels,
    region: Option<Band>,
    selected: Option<Band>,
    hovered: Option<Band>,
    at_pointer: Option<Band>,
    answer: Option<(Query, Result<BandSearch, String>)>,
    pending: Option<(Query, Job<BandSearch>)>,
    launch_after: f64,
}

struct Levels {
    below: f64,
    above: f64,
    drop: f64,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            below: -10.0,
            above: -3.0,
            drop: 3.0,
        }
    }
}

pub(super) struct Inputs<'a> {
    pub trace: Trace,
    pub viewport: Band,
    pub markers: Option<Band>,
    pub capture: &'a Arc<Capture>,
    pub index: Option<&'a Arc<Index>>,
}

#[derive(Clone, Copy, PartialEq)]
struct Query {
    trace: Trace,
    region: Option<Band>,
    cutoff: Cutoff,
}

impl View {
    pub(super) fn content(&mut self, ui: &mut Ui, inputs: Inputs<'_>) -> Option<Band> {
        self.hovered = None;
        if self.rule.is_none() {
            self.rule = Some(if inputs.trace[0] == inputs.trace[1] {
                Rule::Below
            } else {
                Rule::FromPeak
            });
        }
        self.controls(ui, &inputs);
        let query = self.query(inputs.trace);
        self.update(ui, &inputs, query);
        match self
            .answer
            .as_ref()
            .filter(|(key, _)| *key == query)
            .map(|(_, result)| result)
        {
            Some(Ok(search)) => {
                let (hovered, chosen) = results(ui, search, self.at_pointer.or(self.selected));
                self.hovered = hovered;
                if let Some(band) = chosen {
                    self.selected = Some(band);
                    let pad = (band[1] - band[0]) * 0.18;
                    return Some([band[0] - pad, band[1] + pad]);
                }
            }
            Some(Err(error)) => {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            None => {
                ui.weak(if inputs.index.is_some() {
                    "Searching the saved sweep…"
                } else {
                    "Waiting for the sweep index…"
                });
            }
        }
        None
    }

    fn query(&self, trace: Trace) -> Query {
        let cutoff = match self.rule.unwrap_or(Rule::Below) {
            Rule::Below => Cutoff::Below(self.inputs.below),
            Rule::Above => Cutoff::Above(self.inputs.above),
            Rule::FromPeak => Cutoff::PeakDrop(self.inputs.drop),
        };
        Query {
            trace,
            region: self.region,
            cutoff,
        }
    }

    fn controls(&mut self, ui: &mut Ui, inputs: &Inputs<'_>) {
        let previous = self.query(inputs.trace);
        ui.horizontal_wrapped(|ui| {
            let rule = self.rule.get_or_insert(Rule::Below);
            for (value, label) in [
                (Rule::Below, "At most"),
                (Rule::Above, "At least"),
                (Rule::FromPeak, "Below peak"),
            ] {
                clip_ui::buttons::choice(ui, rule, value, label);
            }
            let level = match *rule {
                Rule::Below => &mut self.inputs.below,
                Rule::Above => &mut self.inputs.above,
                Rule::FromPeak => &mut self.inputs.drop,
            };
            let range = if *rule == Rule::FromPeak {
                0.1..=200.0
            } else {
                -240.0..=240.0
            };
            ui.add(
                egui::DragValue::new(level)
                    .range(range)
                    .speed(0.1)
                    .suffix(" dB"),
            )
            .on_hover_text("The crossing is interpolated between adjacent measured points");
            ui.separator();
            if ui.button("Whole sweep").clicked() {
                self.region = None;
            }
            if ui.button("Use visible band").clicked() {
                self.region = Some(inputs.viewport);
            }
            if ui
                .add_enabled(inputs.markers.is_some(), egui::Button::new("Use markers"))
                .clicked()
            {
                self.region = inputs.markers;
            }
            if let Some([a, b]) = self.region {
                let spacing = (b - a).abs() * 0.001;
                ui.weak(format!(
                    "{} – {}",
                    display_axis_value(a, spacing, "Hz"),
                    display_axis_value(b, spacing, "Hz")
                ));
            }
        });
        if previous != self.query(inputs.trace) {
            self.selected = None;
            self.launch_after = ui.input(|input| input.time) + 0.08;
        }
    }

    fn update(&mut self, ui: &Ui, source: &Inputs<'_>, query: Query) {
        if self.pending.as_ref().is_some_and(|(key, _)| *key != query) {
            self.pending = None;
        }
        if let Some(result) = take_query_result(&mut self.pending) {
            self.answer = Some(result);
        }
        if self.pending.is_some() || self.answer.as_ref().is_some_and(|(key, _)| *key == query) {
            return;
        }
        let now = ui.input(|input| input.time);
        if now < self.launch_after {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.launch_after - now));
            return;
        }
        let Some(index) = source.index.cloned() else {
            return;
        };
        let capture = Arc::clone(source.capture);
        let task = read_job(ui.ctx(), Duration::from_secs(100), move |cancel| {
            capture.search_bands(&index, query.region, query.trace, query.cutoff, cancel)
        });
        self.pending = Some((query, task));
    }

    pub(super) fn suspend(&mut self) {
        self.pending = None;
        self.hovered = None;
        self.at_pointer = None;
    }

    pub(super) fn invalidate_trace(&mut self) {
        self.suspend();
        self.answer = None;
        self.selected = None;
    }

    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn observe_frequency(&mut self, trace: Trace, hz: Option<f64>) {
        self.at_pointer = None;
        let query = self.query(trace);
        if let Some(hz) = hz
            && let Some((_, Ok(search))) = self.answer.as_ref().filter(|(key, _)| *key == query)
        {
            self.at_pointer = search.bands.iter().find_map(|band| {
                let [low, high] = band.band_hz();
                (low..=high).contains(&hz).then_some([low, high])
            });
        }
    }

    pub(super) fn paint_bands(
        &self,
        painter: &Painter,
        rect: Rect,
        viewport: Band,
        trace: Trace,
        colors: WorkbenchColors,
    ) {
        let Some((_, Ok(result))) = self
            .answer
            .as_ref()
            .filter(|(key, _)| *key == self.query(trace))
        else {
            return;
        };
        let mut mesh = egui::Mesh::default();
        for band in &result.bands {
            let [lo, hi] = band.band_hz();
            if hi < viewport[0] || lo > viewport[1] {
                continue;
            }
            let left = plot::x_at(rect, viewport, lo.max(viewport[0]));
            let right = plot::x_at(rect, viewport, hi.min(viewport[1])).max(left + 1.0);
            let color = if self.at_pointer.or(self.hovered).or(self.selected) == Some([lo, hi]) {
                colors.focus.gamma_multiply(0.22)
            } else {
                colors.accent.gamma_multiply(0.09)
            };
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
                color,
            );
        }
        painter
            .with_clip_rect(rect.intersect(painter.clip_rect()))
            .add(mesh);
    }

    fn threshold(&self, query: Query) -> Option<f64> {
        match query.cutoff {
            Cutoff::Below(db) | Cutoff::Above(db) => Some(db),
            Cutoff::PeakDrop(drop) => {
                let (old, result) = self.answer.as_ref()?;
                let Cutoff::PeakDrop(old_drop) = old.cutoff else {
                    return None;
                };
                if old.region != query.region || old.trace != query.trace {
                    return None;
                }
                Some(result.as_ref().ok()?.threshold_db + old_drop - drop)
            }
        }
    }

    pub(super) fn cutoff_line(
        &mut self,
        ui: &Ui,
        painter: &Painter,
        rect: Rect,
        values: Band,
        trace: Trace,
        color: Color32,
    ) {
        let query = self.query(trace);
        let Some(db) = self.threshold(query) else {
            return;
        };
        let id = ui.make_persistent_id("touchstone-cutoff-drag");
        if let Some(wanted) = plot::cutoff_handle(ui, painter, rect, values, db, color, id) {
            match query.cutoff {
                Cutoff::Below(_) => self.inputs.below = wanted.clamp(-240.0, 240.0),
                Cutoff::Above(_) => self.inputs.above = wanted.clamp(-240.0, 240.0),
                Cutoff::PeakDrop(drop) => self.inputs.drop = (db + drop - wanted).clamp(0.1, 200.0),
            }
            self.selected = None;
            self.launch_after = ui.input(|input| input.time) + 0.10;
        }
    }
}

fn results(
    ui: &mut Ui,
    search: &BandSearch,
    selected: Option<Band>,
) -> (Option<Band>, Option<Band>) {
    ui.horizontal_wrapped(|ui| {
        let count = search.bands.len();
        if ui
            .add_enabled(count != 0, egui::Button::new("Copy bands").small())
            .clicked()
        {
            ui.ctx().copy_text(copy_bands(search));
        }
        ui.weak(if search.found == count {
            format!("{count} bands")
        } else {
            format!("Widest {count} of {} bands", search.found)
        });
        ui.weak(format!("Cutoff {:.3} dB", search.threshold_db));
    });
    if search.bands.is_empty() {
        ui.weak("No observed frequencies meet the cutoff.");
        return (None, None);
    }
    let mut hover = None;
    let mut chosen = None;
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 5.0;
    TableBuilder::new(ui)
        .id_salt("touchstone-frequency-bands")
        .striped(true)
        .auto_shrink([false, false])
        .min_scrolled_height(0.0)
        .columns(Column::remainder().clip(true), 5)
        .sense(Sense::click())
        .header(row_height, |mut row| {
            for header in [
                "Lower edge",
                "Upper edge",
                "Width",
                "Strongest match",
                "Level",
            ] {
                row.col(|ui| {
                    ui.strong(header);
                });
            }
        })
        .body(|body| {
            body.rows(row_height, search.bands.len(), |mut row| {
                let item = search.bands[row.index()];
                let band = item.band_hz();
                row.set_selected(selected == Some(band));
                for (text, hint) in band_cells(item) {
                    row.col(|ui| {
                        let response = ui.add(egui::Label::new(text).truncate().selectable(false));
                        if let Some(hint) = hint {
                            response.on_hover_text(hint);
                        }
                    });
                }
                let response = row.response();
                if response.clicked() {
                    chosen = Some(band);
                }
                if response.hovered() {
                    hover = Some(band);
                }
            });
        });
    (hover, chosen)
}

fn band_cells(band: ThresholdBand) -> [(String, Option<&'static str>); 5] {
    let spacing = band.width_hz() * 0.0001;
    let edge = |edge: BandEdge| {
        (
            display_axis_value(edge.frequency_hz, spacing, "Hz"),
            Some(if edge.limited {
                "End of observed data"
            } else {
                "Interpolated crossing"
            }),
        )
    };
    let width = display_value(band.width_hz(), "Hz");
    let width = if band.edges.iter().any(|edge| edge.limited) {
        format!("≥ {width}")
    } else {
        width
    };
    [
        edge(band.edges[0]),
        edge(band.edges[1]),
        (width, None),
        (
            display_axis_value(band.extreme.frequency_hz, spacing, "Hz"),
            None,
        ),
        (
            (if band.extreme_db == f64::NEG_INFINITY {
                "−∞ dB".into()
            } else {
                format!("{:.3} dB", band.extreme_db)
            }),
            None,
        ),
    ]
}

fn copy_bands(search: &BandSearch) -> String {
    let mut text = String::from(
        "low_hz\thigh_hz\twidth_hz\tlow_limited\thigh_limited\tpeak_hz\tpeak_db\tcutoff_db\n",
    );
    for band in &search.bands {
        let [low, high] = band.edges;
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            low.frequency_hz,
            high.frequency_hz,
            band.width_hz(),
            low.limited,
            high.limited,
            band.extreme.frequency_hz,
            band.extreme_db,
            search.threshold_db
        );
    }
    text
}
