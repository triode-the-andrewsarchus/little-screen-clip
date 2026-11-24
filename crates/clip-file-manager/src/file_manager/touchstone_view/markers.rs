use super::capture::{Band, Capture, Trace};
use super::{Job, marker_value, plot, read_job};
use crate::ui_appearance::WorkbenchColors;
use egui::{Color32, Rect, Ui};
use little_clip_core::engineering::{
    touchstone::{Index, Metadata, Parameter, Point, Summary},
    waveforms::display_value,
};
use std::{sync::Arc, time::Duration};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Target {
    Frequency(f64),
    Row(u64),
}

#[derive(Default)]
struct Marker {
    assigned: Option<Target>,
    exact: Option<Point>,
    task: Option<Job<Point>>,
    failed: bool,
}

impl Marker {
    fn frequency(&self) -> Option<f64> {
        self.exact
            .as_ref()
            .map(|point| point.frequency_hz)
            .or_else(|| match self.assigned? {
                Target::Frequency(hz) => Some(hz),
                Target::Row(_) => None,
            })
    }

    fn request(
        &mut self,
        ctx: &egui::Context,
        summary: &Summary,
        capture: &Arc<Capture>,
        index: Option<&Arc<Index>>,
    ) {
        let Some(target) = self.assigned else { return };
        if self.exact.is_some() || self.task.is_some() || self.failed {
            return;
        }
        if let Some(point) = prefix_point(summary, target) {
            self.exact = Some(point.clone());
            ctx.request_repaint();
        } else if let Some(index) = index {
            let index = index.clone();
            let capture = capture.clone();
            self.task = Some(read_job(
                ctx,
                Duration::from_secs(10),
                move |cancel| match target {
                    Target::Frequency(hz) => capture.marker(&index, hz, cancel),
                    Target::Row(row) => capture.row(&index, row, cancel),
                },
            ));
        }
    }

    fn poll(&mut self) -> Option<String> {
        let result = self.task.as_ref()?.poll()?;
        self.task = None;
        match result
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        {
            Ok(point) => {
                self.exact = Some(point);
                None
            }
            Err(error) => {
                self.failed = true;
                Some(error)
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Markers {
    pub active: usize,
    markers: [Marker; 2],
    jump: String,
    invalid_jump: bool,
    follow: Option<usize>,
}

impl Markers {
    pub fn label(index: usize) -> &'static str {
        ["A", "B"][index]
    }

    pub fn color(colors: WorkbenchColors, index: usize) -> Color32 {
        [colors.focus, colors.warning][index]
    }

    pub fn set(&mut self, index: usize, target: Target) {
        let marker = &mut self.markers[index];
        if marker.assigned != Some(target) || marker.failed {
            *marker = Marker {
                assigned: Some(target),
                ..Default::default()
            };
        }
    }

    pub fn near(&self, rect: Rect, band: Band, x: f32) -> Option<usize> {
        self.positions(None)
            .map(|(index, hz)| (index, (plot::x_at(rect, band, hz) - x).abs()))
            .filter(|(_, distance)| *distance < 12.0)
            .min_by(|(_, a), (_, b)| a.total_cmp(b))
            .map(|(index, _)| index)
    }

    pub fn positions(
        &self,
        draft: Option<(usize, f64)>,
    ) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.markers
            .iter()
            .enumerate()
            .filter_map(move |(index, marker)| {
                let hz = draft
                    .filter(|(dragged, _)| *dragged == index)
                    .map(|(_, hz)| hz)
                    .or_else(|| marker.frequency())?;
                Some((index, hz))
            })
    }

    pub fn exact_points(&self) -> impl Iterator<Item = (usize, &Point)> {
        self.markers
            .iter()
            .enumerate()
            .filter_map(|(index, marker)| Some((index, marker.exact.as_ref()?)))
    }

    pub fn exact_rows(&self) -> Option<[u64; 2]> {
        let [a, b] = &self.markers;
        Some([a.exact.as_ref()?.index, b.exact.as_ref()?.index])
    }

    pub fn band(&self, draft: Option<(usize, f64)>) -> Option<Band> {
        let mut markers = self.positions(draft);
        let (_, a) = markers.next()?;
        let (_, b) = markers.next()?;
        Some([a.min(b), a.max(b)])
    }

    pub fn request(
        &mut self,
        ctx: &egui::Context,
        summary: &Summary,
        capture: &Arc<Capture>,
        index: Option<&Arc<Index>>,
    ) {
        for marker in &mut self.markers {
            marker.request(ctx, summary, capture, index);
        }
    }

    pub fn poll(&mut self) -> Option<String> {
        let [a, b] = &mut self.markers;
        let a = a.poll();
        let b = b.poll();
        a.or(b)
    }

    pub fn pending(&self) -> bool {
        self.markers.iter().any(|marker| marker.task.is_some())
    }

    pub fn controls(
        &mut self,
        ui: &mut Ui,
        trace: Trace,
        metadata: &Metadata,
        colors: WorkbenchColors,
    ) -> (bool, Option<f64>) {
        let mut inspect = false;
        let mut center = None;
        ui.horizontal_wrapped(|ui| {
            for (index, marker) in self.markers.iter().enumerate() {
                let label = Self::label(index);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new(label)
                                .strong()
                                .color(Self::color(colors, index)),
                        )
                        .selected(self.active == index),
                    )
                    .on_hover_text(format!("Place marker {label}"))
                    .clicked()
                {
                    self.active = index;
                }
                match marker.exact.as_ref() {
                    Some(point) => {
                        if ui
                            .button(readout(point, trace, metadata))
                            .on_hover_text(format!("Open measured row {}", point.index + 1))
                            .clicked()
                        {
                            self.active = index;
                            inspect = true;
                            center = Some(point.frequency_hz);
                        }
                    }
                    None => {
                        ui.weak(marker.frequency().map_or_else(
                            || "—".to_owned(),
                            |hz| format!("{} …", display_value(hz, "Hz")),
                        ));
                    }
                }
                ui.separator();
            }
            if let Some([start, end]) = self.band(None) {
                ui.monospace(format!("Δf {}", display_value(end - start, "Hz")));
            }
            if ui.small_button("Clear").clicked() {
                self.markers = Default::default();
                self.follow = None;
            }
        });
        (inspect, center)
    }

    pub fn inspect(
        &mut self,
        ui: &mut Ui,
        metadata: &Metadata,
        rows: Option<u64>,
        trace: Trace,
    ) -> Option<Trace> {
        self.inspect_navigation(ui, rows);
        let point = self.markers[self.active].exact.as_ref()?;
        let ports = metadata.ports;
        let mut selected = None;
        egui::ScrollArea::vertical()
            .id_salt("touchstone-source-matrix")
            .max_height(166.0)
            .show_rows(
                ui,
                ui.text_style_height(&egui::TextStyle::Monospace) + 6.0,
                ports * ports,
                |ui, range| {
                    for cell in range {
                        let [response, stimulus] = [cell / ports, cell % ports];
                        let Some(value) = point.value(response, stimulus) else {
                            continue;
                        };
                        let label = metadata
                            .parameter_label(response, stimulus)
                            .unwrap_or_default();
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(trace == [response, stimulus], &label)
                                .on_hover_text("Show this matrix element in the plot")
                                .clicked()
                            {
                                selected = Some([response, stimulus]);
                            }
                            ui.monospace(format!("{:.9e} {:+.9e}j", value.re, value.im));
                            ui.weak(format!("{:.3}°", value.arg().to_degrees()));
                            if ui
                                .small_button("Copy")
                                .on_hover_text("Copy exact complex value")
                                .clicked()
                            {
                                ui.ctx().copy_text(format!("{} {:+}j", value.re, value.im));
                            }
                        });
                    }
                },
            );
        selected
    }

    fn inspect_navigation(&mut self, ui: &mut Ui, rows: Option<u64>) {
        let marker = &self.markers[self.active];
        let row = marker.exact.as_ref().map(|point| point.index);
        let mut target = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Marker {} source", Self::label(self.active)));
            if let Some(row) = row {
                ui.monospace(format!("Record {}", row + 1));
                if ui
                    .add_enabled(row > 0, egui::Button::new("Previous"))
                    .clicked()
                {
                    target = Some(Target::Row(row - 1));
                }
                if ui
                    .add_enabled(
                        rows.is_none_or(|rows| row + 1 < rows),
                        egui::Button::new("Next"),
                    )
                    .clicked()
                {
                    target = Some(Target::Row(row + 1));
                }
            }
            let edited = ui.add(
                egui::TextEdit::singleline(&mut self.jump)
                    .id_salt("touchstone-marker-frequency")
                    .hint_text("Frequency in Hz")
                    .desired_width(125.0)
                    .char_limit(32),
            );
            if edited.changed() {
                self.invalid_jump = false;
            }
            if ui.button("Go").clicked()
                || (edited.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)))
            {
                let hz = self
                    .jump
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|hz| hz.is_finite() && *hz >= 0.0);
                self.invalid_jump = hz.is_none();
                if let Some(hz) = hz {
                    target = Some(Target::Frequency(hz));
                }
            }
            if self.invalid_jump {
                ui.colored_label(ui.visuals().error_fg_color, "Enter a frequency in Hz");
            }
        });
        if let Some(target) = target {
            self.set(self.active, target);
            self.follow = Some(self.active);
        }
    }

    pub fn take_follow(&mut self) -> Option<f64> {
        let index = self.follow?;
        let hz = self.markers[index].exact.as_ref()?.frequency_hz;
        self.follow = None;
        Some(hz)
    }
}

fn prefix_point(summary: &Summary, target: Target) -> Option<&Point> {
    let points = &summary.points;
    match target {
        Target::Row(row) => points.get(usize::try_from(row).ok()?),
        Target::Frequency(hz) => {
            let next = points.partition_point(|point| point.frequency_hz < hz);
            match (
                next.checked_sub(1).and_then(|last| points.get(last)),
                points.get(next),
            ) {
                (Some(left), Some(right)) if hz - left.frequency_hz <= right.frequency_hz - hz => {
                    Some(left)
                }
                (_, Some(right)) => Some(right),
                (left, None) if summary.complete => left,
                _ => None,
            }
        }
    }
}

fn readout(point: &Point, trace: Trace, metadata: &Metadata) -> String {
    let hz = display_value(point.frequency_hz, "Hz");
    let Some(value) = point.value(trace[0], trace[1]) else {
        return hz;
    };
    if let Some(reference) = marker_value(metadata, trace) {
        return super::antenna::readout(point.frequency_hz, value, reference);
    }
    let phase = value.arg().to_degrees();
    let amplitude = if metadata.parameter == Parameter::S {
        format!("{:.3} dB", plot::decibels(value))
    } else {
        format!("{:.6}", value.norm())
    };
    format!("{hz} · {amplitude} · {phase:.2}°")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::Path, sync::atomic::AtomicBool};

    #[test]
    fn marker_beyond_preview_waits_for_index_instead_of_snapping_to_preview_edge() {
        let data = b"# Hz S RI\n10 1 0\n20 .5 0\n30 .1 0\n";
        let summary = little_clip_core::engineering::touchstone::summary(
            &data[..],
            Path::new("partial.s1p"),
            2,
            &AtomicBool::new(false),
        )
        .unwrap();
        let local = prefix_point(&summary, Target::Frequency(17.0)).unwrap();
        assert_eq!(local.frequency_hz, 20.0);
        assert!(prefix_point(&summary, Target::Frequency(21.0)).is_none());
    }
}
