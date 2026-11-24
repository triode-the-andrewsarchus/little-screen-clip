use super::{CursorSnap, ScopeTrace, Viewer, display_value, plot, time_span};
use egui::{Color32, Painter, Rect, Ui, pos2};
use little_clip_core::engineering::waveforms::Peak;
use std::ops::Range;

pub(super) fn instrument_channel(name: &str) -> bool {
    matches!(name, "Current range" | "Current LSB" | "Voltage LSB")
}

impl Viewer {
    pub(super) fn channel_controls(&mut self, ui: &mut Ui) {
        if self.recording.power_recording()
            && self
                .overview
                .channels
                .iter()
                .any(|channel| instrument_channel(&channel.name))
        {
            self.power_channel_controls(ui);
            return;
        }
        let spacing = ui.spacing().item_spacing.x;
        let name_width = 141.0;
        let offset_width = if self.stacked { 0.0 } else { 189.0 };
        let group_width = name_width + 52.0 + offset_width + 2.0 * spacing + 7.0;
        let columns = ((ui.available_width() + spacing) / group_width)
            .floor()
            .max(1.0) as usize;
        let count = self.overview.channels.len();
        let rows = count.div_ceil(columns);
        if count <= 8 {
            self.channel_rows(ui, 0..rows, columns, name_width);
        } else {
            let height = ui.spacing().interact_size.y;
            egui::ScrollArea::vertical()
                .id_salt("scope-channels")
                .max_height(2.0 * height + ui.spacing().item_spacing.y)
                .auto_shrink([false, true])
                .show_rows(ui, height, rows, |ui, rows| {
                    self.channel_rows(ui, rows, columns, name_width);
                });
        }
    }

    fn power_channel_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for index in 0..self.overview.channels.len() {
                if !instrument_channel(&self.overview.channels[index].name) {
                    self.channel_buttons(ui, index, 104.0);
                }
            }
            ui.menu_button("Instrument channels", |ui| {
                for index in 0..self.overview.channels.len() {
                    let name = &self.overview.channels[index].name;
                    if instrument_channel(name)
                        && ui
                            .add(egui::Button::selectable(self.visible[index], name))
                            .clicked()
                    {
                        self.toggle_channel_visibility(index);
                    }
                }
            });
        });
    }

    fn channel_rows(&mut self, ui: &mut Ui, rows: Range<usize>, columns: usize, name_width: f32) {
        let count = self.overview.channels.len();
        for row in rows {
            ui.horizontal(|ui| {
                for index in row * columns..((row + 1) * columns).min(count) {
                    self.channel_buttons(ui, index, name_width);
                }
            });
        }
    }

    fn channel_buttons(&mut self, ui: &mut Ui, index: usize, name_width: f32) {
        ui.horizontal(|ui| {
            let selected = self.active == index;
            let text = egui::RichText::new(self.channel_label(index));
            let text = if selected {
                text
            } else {
                text.color(plot::color(index))
            };
            let height = ui.spacing().interact_size.y;
            if ui
                .add_sized(
                    [name_width, height],
                    egui::Button::new(text).selected(selected).truncate(),
                )
                .on_hover_text("Use for cursors, measurements and sample export")
                .clicked()
                && !selected
            {
                self.activate_channel(index);
            }
            let action = if self.visible[index] { "Hide" } else { "Show" };
            if ui
                .add_sized([52.0, height], egui::Button::new(action))
                .on_hover_text(format!("{action} {}", self.overview.channels[index].name))
                .clicked()
            {
                self.toggle_channel_visibility(index);
            }
            if !self.stacked {
                let trace = ScopeTrace::Recorded(index);
                if !self.trace_fft.contains_key(&trace) {
                    if self.trace_fft_toggle(ui, trace) {
                        self.toggle_trace_fft(trace);
                    }
                    self.offset_control(ui, index, self.channel_bounds(index));
                }
            }
        });
        ui.add_space(7.0);
    }

    fn toggle_channel_visibility(&mut self, index: usize) {
        self.visible[index] = !self.visible[index];
        if !self.visible[index]
            && let Some(spectrum) = self.trace_fft.get_mut(&ScopeTrace::Recorded(index))
        {
            spectrum.suspend();
        }
    }

    pub(super) fn offset_control(&mut self, ui: &mut Ui, index: usize, bounds: Peak) {
        super::lanes::offset_control(
            ui,
            &self.overview.channels[index].unit,
            bounds,
            &mut self.display_offsets[index],
        );
    }

    pub(super) fn channel_label(&self, index: usize) -> String {
        let channel = &self.overview.channels[index];
        if self.recording.power_recording() {
            return channel.name.clone();
        }
        let frequency = self
            .measurements
            .as_ref()
            .and_then(|measured| measured.frequencies.get(index).copied().flatten());
        format!(
            "{} · {}",
            channel.name,
            display_value(frequency.unwrap_or(f64::NAN), "Hz")
        )
    }

    pub(super) fn fit_active(&mut self) {
        self.span = self.active_extent();
    }

    pub(super) fn active_extent(&self) -> Range<f64> {
        time_span(std::slice::from_ref(&self.overview.channels[self.active]))
    }

    pub(super) fn activate_channel(&mut self, index: usize) {
        let previous = self.active;
        if previous != index {
            self.spectra.clear_result();
        }
        if self.recording.triggered_segments() {
            if previous != index {
                self.archive_mark();
            }
            if self.visible[previous] && self.visible.iter().filter(|&&shown| shown).count() == 1 {
                self.visible[previous] = false;
            }
            self.active = index;
            self.visible[index] = true;
            self.fit_active();
        } else if previous != index {
            self.active = index;
            self.snaps.iter_mut().for_each(CursorSnap::clear);
        }
    }

    pub(super) fn paint_segment_rail(&self, painter: &Painter, rect: Rect) {
        let area = rect.shrink2(egui::vec2(0.0, 4.0));
        let painter = painter.with_clip_rect(rect);
        for (index, channel) in self.overview.channels.iter().enumerate() {
            let left = plot::x_at(area, &self.domain, channel.start_seconds);
            let right = plot::x_at(area, &self.domain, channel.time_at(channel.sample_count));
            let marker = Rect::from_min_max(
                pos2(left, area.top()),
                pos2(right.max(left + 2.0), area.bottom()),
            );
            let color = if index == self.active {
                Color32::GOLD
            } else {
                plot::color(0).gamma_multiply(0.60)
            };
            painter.rect_filled(marker, 1.0, color);
        }
    }

    pub(super) fn open_segment_near(&mut self, time: f64) {
        let active = self
            .overview
            .channels
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let distance = |channel: &little_clip_core::engineering::scope::Channel| {
                    let end = channel.time_at(channel.sample_count);
                    (time - time.clamp(channel.start_seconds, end)).abs()
                };
                distance(a).total_cmp(&distance(b))
            })
            .map(|(index, _)| index);
        if let Some(index) = active {
            self.activate_channel(index);
        }
    }
}
