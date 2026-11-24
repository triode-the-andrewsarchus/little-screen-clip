//! Offline inspection of infrared signal banks and Sub-GHz pulse timing.

pub(super) mod strip;

use super::scope_view::plot as timing;
use egui::{Color32, Painter, Rect, Sense, Stroke, Ui, pos2, vec2};
use egui_extras::{Column, TableBuilder};
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SignalColumn {
    Name,
    Device,
    Protocol,
}

impl SignalColumn {
    fn value(self, signal: &Signal) -> &str {
        match self {
            Self::Name => &signal.name,
            Self::Device => signal.model.as_deref().unwrap_or_default(),
            Self::Protocol => signal_protocol(signal),
        }
    }
}

fn signal_protocol(signal: &Signal) -> &str {
    signal
        .protocol
        .as_deref()
        .unwrap_or(if signal.pulses.is_empty() { "" } else { "Raw" })
}

fn signal_cells(row: &mut egui_extras::TableRow<'_, '_>, signal: &Signal, show_codes: bool) {
    for column in [
        SignalColumn::Name,
        SignalColumn::Device,
        SignalColumn::Protocol,
    ] {
        row.col(|ui| {
            let value = column.value(signal);
            ui.add(egui::Label::new(if value.is_empty() { "—" } else { value }).truncate());
        });
    }
    if show_codes {
        for field in ["address", "command"] {
            row.col(|ui| {
                let value = signal
                    .fields
                    .iter()
                    .find_map(|(key, value)| (key == field).then_some(value.as_str()));
                ui.add(egui::Label::new(value.unwrap_or("—")).truncate());
            });
        }
    }
}

pub(super) struct Viewer {
    capture: Capture,
    active: usize,
    query: String,
    matches: Vec<usize>,
    order: Option<(SignalColumn, bool)>,
    reveal_signal: bool,
    reveal_pulse: bool,
    span: Range<f64>,
    selection: Option<Range<f64>>,
    cursor: Option<f64>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let (_, capture) = Capture::load(path, cancel)?;
        let span = 0.0..duration(&capture.signals[0]);
        let matches = (0..capture.signals.len()).collect();
        Ok(Self {
            capture,
            active: 0,
            query: String::new(),
            matches,
            order: None,
            reveal_signal: false,
            reveal_pulse: true,
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
            self.reveal_signal = true;
            self.reveal_pulse = true;
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
        if !self.capture.complete {
            ui.weak("Showing the available prefix of this capture");
        }
        if self.capture.signals.len() > 1 {
            let width = (ui.available_width() * 0.56).clamp(120.0, 640.0);
            egui::Panel::left("ir_signal_bank")
                .exact_size(width)
                .show(ui, |ui| self.signal_list(ui));
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 8,
                    top: 6,
                    bottom: 0,
                })
                .show(ui, |ui| self.signal_detail(ui));
        } else {
            self.signal_detail(ui);
        }
    }

    fn signal_detail(&mut self, ui: &mut Ui) {
        self.signal_summary(ui);
        if self.capture.signals[self.active].pulses.is_empty() {
            self.fields(ui);
        } else {
            self.pulse_view(ui);
        }
    }

    fn signal_list(&mut self, ui: &mut Ui) {
        let search_changed = ui
            .add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Find a signal")
                    .desired_width(ui.available_width()),
            )
            .changed();
        if search_changed {
            self.filter_signals();
        }
        if self.matches.is_empty() {
            ui.weak("No matching signals");
            return;
        }
        ui.spacing_mut().item_spacing.y = 1.0;
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 3.0;
        let header_height = row_height.max(ui.spacing().interact_size.y) + 4.0;
        let width = ui.available_width();
        let show_codes = width >= 480.0;
        let headings = [
            ("Signal", Some(SignalColumn::Name)),
            ("Device", Some(SignalColumn::Device)),
            ("Protocol", Some(SignalColumn::Protocol)),
            ("Address", None),
            ("Command", None),
        ];
        let mut table = TableBuilder::new(ui)
            .id_salt("ir_signal_list")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .column(Column::exact((width * 0.22).clamp(62.0, 126.0)).clip(true));
        table = if show_codes {
            table
                .column(Column::exact(width * 0.26).clip(true))
                .column(Column::exact(width * 0.13).clip(true))
                .column(Column::exact(width * 0.17).clip(true))
                .column(Column::remainder().clip(true))
        } else {
            table
                .column(Column::remainder().clip(true))
                .column(Column::exact((width * 0.22).clamp(68.0, 105.0)).clip(true))
        };
        if search_changed {
            table = table.vertical_scroll_offset(0.0);
        } else if std::mem::take(&mut self.reveal_signal)
            && let Some(index) = self.matches.iter().position(|&index| index == self.active)
        {
            table = table.scroll_to_row(index, None);
        }
        let mut selected = self.active;
        let mut order = self.order;
        table
            .header(header_height, |mut row| {
                for (title, column) in headings.into_iter().take(if show_codes { 5 } else { 3 }) {
                    row.col(|ui| {
                        let Some(column) = column else {
                            ui.strong(title);
                            return;
                        };
                        let direction = order
                            .filter(|(sorted, _)| *sorted == column)
                            .map(|(_, descending)| descending);
                        let caption = match direction {
                            Some(true) => format!("{title} ↓"),
                            Some(false) => format!("{title} ↑"),
                            None => title.to_owned(),
                        };
                        if ui
                            .add(egui::Button::new(caption).frame(false).truncate())
                            .on_hover_text("Sort the matching signals")
                            .clicked()
                        {
                            order = Some((column, direction == Some(false)));
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.matches.len(), |mut row| {
                    let index = self.matches[row.index()];
                    row.set_selected(index == self.active);
                    signal_cells(&mut row, &self.capture.signals[index], show_codes);
                    if row.response().clicked() {
                        selected = index;
                    }
                });
            });
        if selected != self.active {
            self.focus_signal(selected);
            self.reveal_signal = false;
        }
        if order != self.order {
            self.order = order;
            self.sort_signals();
            self.reveal_signal = true;
            ui.ctx().request_repaint();
        }
    }

    fn filter_signals(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.matches = self
            .capture
            .signals
            .iter()
            .enumerate()
            .filter(|(_, signal)| {
                query.is_empty()
                    || [signal.name.as_str(), SignalColumn::Device.value(signal)]
                        .iter()
                        .any(|value| value.to_lowercase().contains(&query))
            })
            .map(|(index, _)| index)
            .collect();
        self.sort_signals();
    }

    fn sort_signals(&mut self) {
        let Some((column, descending)) = self.order else {
            return;
        };
        if descending {
            self.matches.sort_by_cached_key(|&index| {
                let value = column.value(&self.capture.signals[index]).to_lowercase();
                (value.is_empty(), std::cmp::Reverse(value), index)
            });
        } else {
            self.matches.sort_by_cached_key(|&index| {
                let value = column.value(&self.capture.signals[index]).to_lowercase();
                (value.is_empty(), value, index)
            });
        }
    }

    fn signal_summary(&self, ui: &mut Ui) {
        let signal = &self.capture.signals[self.active];
        ui.horizontal_wrapped(|ui| {
            if signal.name != self.capture.format.title() {
                ui.strong(signal.label().as_ref());
            }
            if let Some(protocol) = &signal.protocol
                && protocol != &signal.name
            {
                ui.label(protocol);
            }
            if let Some(hz) = signal.carrier_hz {
                ui.label(display_value(hz as f64, "Hz"));
            }
            if let Some(duty) = signal.duty_cycle {
                ui.weak(format!("{:.1}% duty", duty * 100.0));
            }
        });
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
                self.reveal_pulse = true;
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
        let height = (ui.available_height() * 0.36).clamp(70.0, 208.0);
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
                self.reveal_pulse = true;
            }
        }
        let Some(point) = response.interact_pointer_pos() else {
            return;
        };
        let time = timing::at(rect, &self.span, point.x);
        if response.clicked() && rect.contains(point) {
            self.cursor = Some(time);
            self.selection = None;
            self.reveal_pulse = true;
        }
        if response.dragged()
            && let Some(origin) = ui
                .input(|input| input.pointer.press_origin())
                .filter(|point| rect.contains(*point))
        {
            let first = timing::at(rect, &self.span, origin.x);
            self.selection = Some(first.min(time)..first.max(time));
            self.cursor = Some(time);
            self.reveal_pulse = true;
        }
    }

    fn table(&mut self, ui: &mut Ui) {
        let signal = &self.capture.signals[self.active];
        let span = self.selection.as_ref().unwrap_or(&self.span);
        let pulses = signal.intersecting(sample_range(span));
        ui.spacing_mut().item_spacing.y = 1.0;
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 3.0;
        let time_width = (ui.available_width() * 0.35).clamp(40.0, 102.0);
        let mut table = TableBuilder::new(ui)
            .id_salt("pulse_samples")
            .striped(true)
            .sense(Sense::click())
            .auto_shrink([false, false])
            .columns(Column::exact(time_width).clip(true), 2)
            .column(Column::remainder().clip(true));
        if std::mem::take(&mut self.reveal_pulse) {
            let target = self
                .cursor
                .filter(|cursor| span.contains(cursor))
                .map_or(0, |cursor| {
                    pulses.partition_point(|pulse| pulse.end_us() as f64 * 1e-6 <= cursor)
                });
            table = table.scroll_to_row(target, None);
        }
        table
            .header(row_height, |mut row| {
                for title in ["Start", "Duration", "Level"] {
                    row.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, pulses.len(), |mut row| {
                    let pulse = pulses[row.index()];
                    let start = pulse.start_us as f64 * 1e-6;
                    let end = pulse.end_us() as f64 * 1e-6;
                    row.set_selected(self.cursor.is_some_and(|t| start <= t && t < end));
                    row.col(|ui| {
                        ui.monospace(display_value(start, "s"));
                    });
                    row.col(|ui| {
                        ui.monospace(display_value(f64::from(pulse.duration_us) * 1e-6, "s"));
                    });
                    row.col(|ui| {
                        ui.monospace(level(pulse.high));
                    });
                    if row.response().clicked() {
                        self.cursor = Some(start);
                        if self.selection.take().is_some() {
                            self.reveal_pulse = true;
                        }
                    }
                });
            });
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
