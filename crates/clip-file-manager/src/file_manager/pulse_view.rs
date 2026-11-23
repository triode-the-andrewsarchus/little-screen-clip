//! Offline inspection of infrared signal banks and Sub-GHz pulse timing.

pub(super) mod strip;

use super::scope_view::plot as timing;
use egui::{Color32, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use little_clip_core::{
    engineering::{
        pulses::{Capture, Signal},
        waveforms::display_value,
    },
    palette::AppPalette,
};
use std::{fmt::Write, ops::Range, path::Path, sync::atomic::AtomicBool};

const MAX_DRAW_PULSES: usize = 4096;
const MAX_COPY_PULSES: usize = 8192;
const PULSE_COLOR: Color32 = Color32::from_rgb(239, 187, 98);

pub(super) struct Viewer {
    capture: Capture,
    active: usize,
    query: String,
    span: Range<f64>,
    selection: Option<Range<f64>>,
    cursor: Option<f64>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (_, capture) = Capture::load(path, cancel)?;
        let span = 0.0..duration(&capture.signals[0]);
        Ok(Self {
            capture,
            active: 0,
            query: String::new(),
            span,
            selection: None,
            cursor: None,
        })
    }

    pub fn focus_signal(&mut self, ordinal: usize) {
        if ordinal < self.capture.signals.len() {
            self.active = ordinal;
            self.span = 0.0..duration(&self.capture.signals[ordinal]);
            self.selection = None;
            self.cursor = None;
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        let mut shell = crate::shell::StudioShell::new(
            self.capture.format.title(),
            crate::icons::Icon::Zap,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.selector(ui);
        if !self.capture.complete {
            ui.weak("Showing the available prefix of this capture");
        }
        if self.capture.signals[self.active].pulses.is_empty() {
            self.fields(ui);
        } else {
            self.pulse_view(ui);
        }
    }

    fn selector(&mut self, ui: &mut Ui) {
        let mut selected = self.active;
        ui.horizontal_wrapped(|ui| {
            ui.strong(self.capture.format.title());
            if self.capture.signals.len() > 1 {
                egui::ComboBox::from_id_salt("pulse_active_signal")
                    .selected_text(self.capture.signals[self.active].label().as_ref())
                    .show_ui(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.query).hint_text("Find a signal"),
                        );
                        let query = self.query.to_lowercase();
                        egui::ScrollArea::vertical()
                            .max_height(298.0)
                            .show(ui, |ui| {
                                for (index, signal) in self.capture.signals.iter().enumerate() {
                                    if query.is_empty()
                                        || signal.name.to_lowercase().contains(&query)
                                    {
                                        ui.selectable_value(&mut selected, index, &signal.name);
                                    }
                                }
                            });
                    });
            }
            let signal = &self.capture.signals[self.active];
            if let Some(protocol) = &signal.protocol {
                ui.label(protocol);
            }
            if let Some(hz) = signal.carrier_hz {
                ui.label(display_value(hz as f64, "Hz"));
            }
            if let Some(duty) = signal.duty_cycle {
                ui.weak(format!("{:.1}% duty", duty * 100.0));
            }
        });
        if selected != self.active {
            self.focus_signal(selected);
        }
    }

    fn fields(&self, ui: &mut Ui) {
        let signal = &self.capture.signals[self.active];
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("ir_signal_fields")
                .striped(true)
                .show(ui, |ui| {
                    for (key, value) in &signal.fields {
                        ui.strong(key);
                        ui.monospace(value);
                        ui.end_row();
                    }
                });
        });
    }

    fn pulse_view(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let signal = &self.capture.signals[self.active];
            ui.weak(format!("{} pulses", signal.pulses.len()));
            if ui.button("Fit").clicked() {
                self.span = 0.0..duration(signal);
            }
            let span = self.selection.as_ref().map(sample_range);
            let pulses = span.map_or(&[][..], |span| signal.intersecting(span));
            if ui
                .add_enabled(
                    !pulses.is_empty() && pulses.len() <= MAX_COPY_PULSES,
                    egui::Button::new("Copy pulses"),
                )
                .clicked()
            {
                let mut text = String::from("Start (µs)\tDuration (µs)\tLevel\n");
                for pulse in pulses {
                    let _ = writeln!(
                        text,
                        "{}\t{}\t{}",
                        pulse.start_us,
                        pulse.duration_us,
                        level(pulse.high)
                    );
                }
                ui.ctx().copy_text(text);
            }
            if let Some(time) = self.cursor {
                ui.label(display_value(time, "s"));
            }
            if let Some(span) = &self.selection {
                ui.label(format!("Δ {}", display_value(span.end - span.start, "s")));
            }
        });
        let height = (ui.available_height() * 0.57).max(85.0);
        let (outer, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
        let rect = Rect::from_min_max(outer.min + vec2(30.0, 6.0), outer.max - vec2(8.0, 18.0));
        self.navigate(ui, rect, &response);
        let painter = ui.painter_at(outer);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        timing::axes(&painter, rect, &self.span, ui.visuals().weak_text_color());
        let signal = &self.capture.signals[self.active];
        let clipped = painter.with_clip_rect(rect);
        draw_pulses(&clipped, rect.shrink2(vec2(0.0, 12.0)), signal, &self.span);
        if let Some(range) = &self.selection {
            timing::highlight(
                &clipped,
                rect,
                &self.span,
                range.clone(),
                Color32::from_white_alpha(50),
            );
        }
        if let Some(time) = self.cursor {
            let x = timing::x_at(rect, &self.span, time);
            clipped.vline(x, rect.y_range(), Stroke::new(1.0, Color32::WHITE));
        }
        self.table(ui);
    }

    fn navigate(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        if response.hovered()
            && let Some(point) = ui
                .input(|input| input.pointer.hover_pos())
                .filter(|point| rect.contains(*point))
        {
            let scroll = ui.input(|input| input.smooth_scroll_delta);
            if scroll != egui::Vec2::ZERO {
                let domain = duration(&self.capture.signals[self.active]);
                let old = self.span.end - self.span.start;
                let anchor = timing::at(rect, &self.span, point.x);
                let scale = f64::from((-scroll.y * 0.012).exp()).clamp(0.1, 10.0);
                let width = (old * scale).clamp(1e-6_f64.min(domain), domain);
                let offset = f64::from(scroll.x / rect.width().max(1.0)) * old;
                let start = (anchor - (anchor - self.span.start) * width / old - offset)
                    .clamp(0.0, (domain - width).max(0.0));
                self.span = start..start + width;
            }
        }
        let Some(point) = response.interact_pointer_pos() else {
            return;
        };
        let time = timing::at(rect, &self.span, point.x);
        if response.clicked() && rect.contains(point) {
            self.cursor = Some(time);
            self.selection = None;
        }
        if response.dragged()
            && let Some(origin) = ui
                .input(|input| input.pointer.press_origin())
                .filter(|point| rect.contains(*point))
        {
            let first = timing::at(rect, &self.span, origin.x);
            self.selection = Some(first.min(time)..first.max(time));
            self.cursor = Some(time);
        }
    }

    fn table(&mut self, ui: &mut Ui) {
        let signal = &self.capture.signals[self.active];
        let span = self.selection.as_ref().unwrap_or(&self.span);
        let pulses = signal.intersecting(sample_range(span));
        ui.weak("Start · Duration · Level");
        egui::ScrollArea::vertical()
            .max_height(ui.available_height().max(32.0))
            .show_rows(
                ui,
                ui.text_style_height(&egui::TextStyle::Monospace),
                pulses.len(),
                |ui, rows| {
                    for index in rows {
                        let pulse = pulses[index];
                        let start = pulse.start_us as f64 * 1e-6;
                        let end = pulse.end_us() as f64 * 1e-6;
                        let highlighted = self.cursor.is_some_and(|t| start <= t && t < end);
                        let label = format!(
                            "{:>13}   {:>12}   {}",
                            display_value(start, "s"),
                            display_value(f64::from(pulse.duration_us) * 1e-6, "s"),
                            level(pulse.high)
                        );
                        if ui
                            .selectable_label(highlighted, egui::RichText::new(label).monospace())
                            .clicked()
                        {
                            self.cursor = Some(start);
                            self.selection = None;
                        }
                    }
                },
            );
    }
}

fn level(high: bool) -> &'static str {
    if high { "High" } else { "Low" }
}

fn sample_range(span: &Range<f64>) -> Range<u64> {
    (span.start * 1e6).floor() as u64..(span.end * 1e6).ceil() as u64
}

fn duration(signal: &Signal) -> f64 {
    (signal.duration_us() as f64 * 1e-6).max(1e-6)
}

fn draw_pulses(painter: &Painter, rect: Rect, signal: &Signal, span: &Range<f64>) {
    let pulses = signal.intersecting(sample_range(span));
    let high = rect.top() + rect.height() * 0.22;
    let low = rect.bottom() - rect.height() * 0.22;
    if pulses.len() <= MAX_DRAW_PULSES {
        let mut points = Vec::with_capacity(pulses.len() * 2);
        for pulse in pulses {
            let y = if pulse.high { high } else { low };
            let start = timing::x_at(rect, span, pulse.start_us as f64 * 1e-6);
            let end = timing::x_at(rect, span, pulse.end_us() as f64 * 1e-6);
            points.push(pos2(start, y));
            points.push(pos2(end, y));
        }
        if !points.is_empty() {
            painter.add(egui::Shape::line(points, Stroke::new(1.25, PULSE_COLOR)));
        }
    } else {
        let stride = pulses.len().div_ceil(512);
        let mut mesh = egui::Mesh::default();
        for slice in pulses.chunks(stride) {
            let first = slice[0];
            let last = slice[slice.len() - 1];
            let left = timing::x_at(rect, span, first.start_us as f64 * 1e-6);
            let right = timing::x_at(rect, span, last.end_us() as f64 * 1e-6);
            let both = slice.iter().any(|pulse| pulse.high != first.high);
            let [top, bottom] = match (both, first.high) {
                (true, _) => [high, low],
                (false, true) => [high - 0.5, high + 0.5],
                (false, false) => [low - 0.5, low + 0.5],
            };
            mesh.add_colored_rect(
                Rect::from_min_max(pos2(left, top), pos2(right, bottom)),
                PULSE_COLOR.gamma_multiply(0.7),
            );
        }
        painter.add(mesh);
    }
}
