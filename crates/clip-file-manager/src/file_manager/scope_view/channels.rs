use super::{CursorSnap, Viewer, display_value, plot, time_span};
use egui::{Color32, Painter, Rect, Ui, pos2};
use std::ops::Range;

impl Viewer {
    pub(super) fn channel_picker(&mut self, ui: &mut Ui) {
        let mut active = self.active;
        if self.recording.triggered_segments() || self.overview.channels.len() > 8 {
            egui::ComboBox::from_id_salt("scope-channel")
                .selected_text(self.channel_label(active))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for index in 0..self.overview.channels.len() {
                        ui.selectable_value(&mut active, index, self.channel_label(index));
                    }
                });
        } else {
            for index in 0..self.overview.channels.len() {
                let selected = index == active;
                let text = egui::RichText::new(self.channel_label(index));
                let text = if selected {
                    text
                } else {
                    text.color(plot::color(index))
                };
                if ui.selectable_label(selected, text).clicked() {
                    active = index;
                }
            }
        }
        if active != self.active {
            self.activate_channel(active);
        }
    }

    fn channel_label(&self, index: usize) -> String {
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

    fn activate_channel(&mut self, index: usize) {
        let previous = self.active;
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
