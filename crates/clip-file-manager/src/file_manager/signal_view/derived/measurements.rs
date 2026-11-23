use super::{Frame, Lane, plot, take_query_result};
use crate::file_manager::preview_job::Task;
use egui::{Color32, Painter, Rect, Sense, Ui, pos2};
use egui_extras::{Column, TableBuilder};
use little_clip_core::{
    BackgroundJobs,
    engineering::{
        analysis::{Dimension, Measurement, MeasurementGrid, Moments},
        waveforms::display_value,
    },
};
use std::{
    fmt::Write,
    ops::Range,
    sync::{Arc, Weak, atomic::AtomicBool},
    time::Duration,
};

#[derive(Default)]
pub(super) struct View {
    open: bool,
    signal: usize,
    condition: Option<usize>,
    hovered: Option<usize>,
    selected: Option<usize>,
    reveal: bool,
    ready: bool,
    pending: Option<(Query, Task<Result<Measurement, String>>)>,
    result: Option<(Query, Result<Measurement, String>)>,
}

struct Query {
    frame: Weak<Frame>,
    span: Range<f64>,
    signal: usize,
    condition: Option<usize>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct SavedMeasurements {
    open: bool,
    signal: usize,
    condition: Option<usize>,
    selected: Option<usize>,
}

impl Query {
    fn matches(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.frame, &other.frame)
            && self.span == other.span
            && self.signal == other.signal
            && self.condition == other.condition
    }
}

impl View {
    pub(super) fn save(&self) -> SavedMeasurements {
        SavedMeasurements {
            open: self.open,
            signal: self.signal,
            condition: self.condition,
            selected: self.selected,
        }
    }

    pub(super) fn restore(&mut self, saved: SavedMeasurements, lanes: usize) {
        self.open = saved.open;
        self.signal = if saved.signal < lanes {
            saved.signal
        } else {
            usize::MAX
        };
        self.condition = saved.condition.filter(|&index| index < lanes);
        self.selected = saved.selected.filter(|&group| group < 3);
    }
    pub(super) fn remap(&mut self, lanes: &[Option<usize>]) {
        let mapped = |index| lanes.get(index).copied().flatten();
        let signal = mapped(self.signal);
        let condition = self.condition.and_then(mapped);
        let selected = self
            .selected
            .filter(|_| signal.is_some() && condition.is_some() == self.condition.is_some());
        *self = Self {
            open: self.open,
            signal: signal.unwrap_or(usize::MAX),
            condition,
            selected,
            ..Self::default()
        };
    }

    pub(super) fn ui(&mut self, ui: &mut Ui, frame: &Arc<Frame>, span: Range<f64>, brushed: bool) {
        self.ready = false;
        self.hovered = None;
        if !self.selectable(frame) {
            return;
        }
        if ui
            .add(egui::Button::new("Measure lanes").selected(self.open))
            .clicked()
        {
            self.open = !self.open;
        }
        if !self.open {
            self.pending = None;
            return;
        }
        self.controls(ui, frame, brushed);
        let current = Query {
            frame: Arc::downgrade(frame),
            span,
            signal: self.signal,
            condition: self.condition,
        };
        self.update(ui, frame, current);
        let Lane::Analog { channel, .. } = &frame.lanes[self.signal] else {
            return;
        };
        let result = self
            .result
            .as_ref()
            .filter(|_| self.ready)
            .map(|(_, result)| result.as_ref().map_err(String::as_str));
        let (hovered, clicked) = table(
            ui,
            result,
            self.condition.is_some(),
            &channel.unit,
            self.selected,
        );
        self.hovered = hovered;
        if let Some(group) = clicked {
            self.selected = (self.selected != Some(group)).then_some(group);
            self.reveal = self.selected.is_some();
        }
    }

    fn selectable(&mut self, frame: &Frame) -> bool {
        if !matches!(frame.lanes.get(self.signal), Some(Lane::Analog { .. })) {
            let Some(signal) = frame
                .lanes
                .iter()
                .position(|lane| matches!(lane, Lane::Analog { .. }))
            else {
                return false;
            };
            self.signal = signal;
        }
        if let Some(index) = self.condition
            && !matches!(frame.lanes.get(index), Some(Lane::Digital { .. }))
        {
            self.condition = None;
        }
        true
    }

    fn controls(&mut self, ui: &mut Ui, frame: &Frame, brushed: bool) {
        let before = (self.signal, self.condition);
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("measure-signal")
                .selected_text(frame.lanes[self.signal].name())
                .show_ui(ui, |ui| {
                    for (index, lane) in frame.lanes.iter().enumerate() {
                        if matches!(lane, Lane::Analog { .. }) {
                            ui.selectable_value(&mut self.signal, index, lane.name());
                        }
                    }
                });
            ui.label("Condition");
            egui::ComboBox::from_id_salt("measure-condition")
                .selected_text(
                    self.condition
                        .map_or("All samples", |index| frame.lanes[index].name()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.condition, None, "All samples");
                    for (index, lane) in frame.lanes.iter().enumerate() {
                        if matches!(lane, Lane::Digital { .. }) {
                            ui.selectable_value(&mut self.condition, Some(index), lane.name());
                        }
                    }
                });
            ui.weak(if brushed {
                "Selected interval"
            } else {
                "Visible interval"
            });
        });
        if before != (self.signal, self.condition) {
            self.selected = None;
        }
    }

    fn update(&mut self, ui: &Ui, frame: &Arc<Frame>, current: Query) {
        if let Some(result) = take_query_result(&mut self.pending) {
            self.result = Some(result);
        }
        if self
            .result
            .as_ref()
            .is_some_and(|(query, _)| query.matches(&current))
        {
            self.ready = true;
            return;
        }
        if self.pending.is_none() {
            let frame = Arc::clone(frame);
            let span = current.span.clone();
            let signal = current.signal;
            let condition = current.condition;
            let ctx = ui.ctx().clone();
            let work = move |cancel: &AtomicBool| {
                let result = measure(&frame, signal, condition, span, cancel);
                ctx.request_repaint();
                result
            };
            let task = Task::start(BackgroundJobs::global(), Duration::from_secs(10), work);
            self.pending = Some((current, task));
        }
        ui.ctx().request_repaint_after(Duration::from_millis(32));
    }

    pub(super) fn paint(&self, painter: &Painter, rect: Rect, visible: &Range<f64>) {
        let Some(group) = self
            .hovered
            .or(self.selected)
            .filter(|_| self.open && self.ready)
        else {
            return;
        };
        let Some((_, Ok(result))) = &self.result else {
            return;
        };
        let width = (result.span.end - result.span.start) / result.bands.len() as f64;
        let color = group_color(group);
        let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        let mut mesh = egui::Mesh::default();
        for (index, bucket) in result.bands.iter().enumerate() {
            let fraction = bucket[group] as f32;
            if fraction <= 0.0 {
                continue;
            }
            let start = (index as f64)
                .mul_add(width, result.span.start)
                .max(visible.start);
            let end = ((index + 1) as f64)
                .mul_add(width, result.span.start)
                .min(visible.end);
            if start < end {
                mesh.add_colored_rect(
                    Rect::from_min_max(
                        pos2(plot::x_at(rect, visible, start), rect.top()),
                        pos2(plot::x_at(rect, visible, end), rect.bottom()),
                    ),
                    color.gamma_multiply(0.23 * fraction),
                );
            }
        }
        painter.add(mesh);
    }

    pub(super) fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal)
    }
}

fn measure(
    frame: &Frame,
    signal: usize,
    condition: Option<usize>,
    span: Range<f64>,
    cancel: &AtomicBool,
) -> Result<Measurement, String> {
    let Some(Lane::Analog { raw, .. }) = frame.lanes.get(signal) else {
        return Err("Select a numerical lane to measure.".into());
    };
    let condition = if let Some(index) = condition {
        match frame.lanes.get(index) {
            Some(Lane::Digital { raw: mask, .. })
                if mask.start == raw.start && mask.step == raw.step =>
            {
                Some(mask.samples.as_slice())
            }
            _ => return Err("Condition and signal need the same sample grid.".into()),
        }
    } else {
        None
    };
    MeasurementGrid {
        start: raw.start,
        step: raw.step,
        signal: &raw.samples,
        condition,
    }
    .measure(span, cancel)
}

fn table(
    ui: &mut Ui,
    result: Option<Result<&Measurement, &str>>,
    gated: bool,
    unit: &str,
    selected: Option<usize>,
) -> (Option<usize>, Option<usize>) {
    let integral_unit = Dimension::from_unit(unit)
        .and_then(Dimension::time_integral)
        .map_or_else(|| format!("{unit}·s"), |dimension| dimension.to_string());
    let groups = if gated {
        &[(1, "High"), (0, "Low")][..]
    } else {
        &[(1, "All")][..]
    };
    let measurement = result.and_then(Result::ok);
    let mut hovered = None;
    let mut picked = None;
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 5.0;
    TableBuilder::new(ui)
        .id_salt("measured signal groups")
        .striped(true)
        .vscroll(false)
        .auto_shrink([false, true])
        .columns(Column::remainder().clip(true), 6)
        .sense(Sense::click())
        .header(row_height, |mut row| {
            for title in [
                "State",
                "Observed time",
                "Mean",
                "RMS",
                "Min – max",
                "Integral",
            ] {
                row.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|mut body| {
            for &(group, title) in groups {
                body.row(row_height, |mut row| {
                    let selected = selected == Some(group);
                    row.set_selected(selected);
                    row.col(|ui| {
                        let text = egui::RichText::new(title);
                        let text = if selected {
                            text
                        } else {
                            text.color(group_color(group))
                        };
                        ui.add(egui::Label::new(text).selectable(false));
                    });
                    let cells = measurement.map_or_else(
                        || std::array::from_fn(|_| "—".to_owned()),
                        |result| cells(result.groups[group], unit, &integral_unit),
                    );
                    for cell in cells {
                        row.col(|ui| {
                            ui.add(egui::Label::new(&cell).truncate().selectable(false))
                                .on_hover_text(cell);
                        });
                    }
                    let response = row.response();
                    if measurement.is_some() && response.hovered() {
                        hovered = Some(group);
                    }
                    if measurement.is_some() && response.clicked() {
                        picked = Some(group);
                    }
                });
            }
        });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                measurement.is_some(),
                egui::Button::new("Copy statistics").small(),
            )
            .clicked()
            && let Some(result) = measurement
        {
            ui.ctx()
                .copy_text(tsv(result, groups, unit, &integral_unit));
        }
        match result {
            None => {
                ui.weak("Measuring…");
            }
            Some(Err(error)) => {
                let text = egui::RichText::new(error).color(ui.visuals().warn_fg_color);
                ui.add(egui::Label::new(text).truncate())
                    .on_hover_text(error);
            }
            Some(Ok(result)) => coverage(ui, result),
        }
    });
    (hovered, picked)
}

fn coverage(ui: &mut Ui, result: &Measurement) {
    let parts = [
        (result.unknown_condition_seconds, "Condition unknown for"),
        (result.unrecorded_seconds, "Outside the sample grid for"),
    ];
    let text = parts
        .into_iter()
        .filter(|(seconds, _)| *seconds > 0.0)
        .map(|(seconds, label)| format!("{label} {}", display_value(seconds, "s")))
        .collect::<Vec<_>>()
        .join(" · ");
    if !text.is_empty() {
        ui.add(egui::Label::new(egui::RichText::new(&text).weak()).truncate())
            .on_hover_text(text);
    }
}

fn cells(data: Moments, unit: &str, integral_unit: &str) -> [String; 5] {
    let time = display_value(data.measured_seconds, "s");
    let time = if data.measured_seconds < data.state_seconds {
        format!("{time} of {}", display_value(data.state_seconds, "s"))
    } else {
        time
    };
    let value = |number: Option<f64>, unit| {
        number.map_or_else(|| "—".into(), |number| display_value(number, unit))
    };
    let extent = data.extent().map_or_else(
        || "—".into(),
        |range| {
            format!(
                "{} – {}",
                display_value(range.start, unit),
                display_value(range.end, unit)
            )
        },
    );
    [
        time,
        value(data.mean(), unit),
        value(data.rms(), unit),
        extent,
        value(data.integral(), integral_unit),
    ]
}

fn tsv(result: &Measurement, groups: &[(usize, &str)], unit: &str, integral_unit: &str) -> String {
    let mut text = format!(
        "state\tstart_s\tend_s\tstate_s\tobserved_s\tmean ({unit})\trms ({unit})\tmin ({unit})\tmax ({unit})\tintegral ({integral_unit})\n"
    );
    let number = |value: Option<f64>| value.map_or_else(String::new, |value| value.to_string());
    for &(group, name) in groups {
        let data = result.groups[group];
        let _ = writeln!(
            text,
            "{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            result.span.start,
            result.span.end,
            data.state_seconds,
            data.measured_seconds,
            number(data.mean()),
            number(data.rms()),
            number(data.extent().map(|range| range.start)),
            number(data.extent().map(|range| range.end)),
            number(data.integral()),
        );
    }
    text
}

fn group_color(group: usize) -> Color32 {
    plot::color(1 - group)
}
