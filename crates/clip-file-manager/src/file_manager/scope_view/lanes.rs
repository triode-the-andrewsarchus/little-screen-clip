use super::{Viewer, display_value, plot};
use egui::{
    Align, Align2, Color32, FontId, Layout, Painter, Rect, Stroke, Ui, UiBuilder, pos2, vec2,
};
use little_clip_core::engineering::scope::Channel;
use little_clip_core::engineering::waveforms::Peak;

const HEADER_HEIGHT: f32 = 24.0;
pub(super) const MIN_HEIGHT: f32 = 68.0;

#[derive(Clone, Copy)]
pub(super) struct Lane {
    pub plot: Rect,
    header: Rect,
    pub frame: Rect,
}

impl Lane {
    pub fn new(area: Rect, slot: usize, count: usize) -> Self {
        let plot_area = plot::lane(area, slot, count);
        let frame = Rect::from_min_max(
            pos2(plot_area.left() - 78.0, plot_area.top() + 3.0),
            pos2(plot_area.right(), plot_area.bottom() - 3.0),
        );
        let header = Rect::from_min_size(frame.min, vec2(frame.width(), HEADER_HEIGHT));
        let plot = Rect::from_min_max(
            pos2(plot_area.left(), header.bottom() + 6.0),
            frame.right_bottom() - vec2(0.0, 6.0),
        );
        Self {
            plot,
            header,
            frame,
        }
    }

    pub fn background(self, ui: &Ui, painter: &Painter) {
        painter.rect_filled(self.frame, 3.0, ui.visuals().faint_bg_color);
        painter.rect_filled(self.plot, 0.0, ui.visuals().extreme_bg_color);
    }

    pub fn label(self, painter: &Painter, name: &str, color: Color32) {
        painter.text(
            self.header.left_center() + vec2(7.0, 0.0),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(13.0),
            color,
        );
    }

    pub fn scale(self, painter: &Painter, bounds: Peak, unit: &str, color: Color32) {
        if !bounds.valid() {
            return;
        }
        let tick = |y, value| {
            painter.text(
                pos2(self.plot.left() - 6.0, y),
                Align2::RIGHT_CENTER,
                display_value(f64::from(value), unit),
                FontId::monospace(11.0),
                color,
            );
        };
        if bounds.min == bounds.max {
            tick(self.plot.center().y, bounds.min);
        } else {
            tick(self.plot.top(), bounds.max);
            tick(self.plot.bottom(), bounds.min);
        }
        if bounds.min <= 0.0 && bounds.max >= 0.0 {
            painter.hline(
                self.plot.x_range(),
                plot::y_at(self.plot, bounds, 0.0),
                Stroke::new(1.0, color.gamma_multiply(0.30)),
            );
        }
    }

    pub fn trace_heading(
        self,
        ui: &mut Ui,
        channel: &Channel,
        index: usize,
        offset: &mut f32,
        bounds: Peak,
    ) {
        let unit = &channel.unit;
        let range = (f64::from(bounds.max) - f64::from(bounds.min)).abs();
        let header = self.header.shrink2(vec2(7.0, 0.0));
        let mut ui = ui.new_child(
            UiBuilder::new()
                .id_salt(("scope-trace-heading", index))
                .max_rect(header)
                .layout(Layout::left_to_right(Align::Center)),
        );
        ui.set_clip_rect(ui.clip_rect().intersect(self.header));
        ui.spacing_mut().interact_size.y = 22.0;
        ui.spacing_mut().button_padding.y = 2.0;
        ui.colored_label(plot::color(index), &channel.name);
        ui.add_space(6.0);
        let label = if unit == "V" {
            "V offset"
        } else {
            "Display offset"
        };
        ui.label(label);
        let speed = if range.is_finite() && range > 0.0 {
            range / 100.0
        } else {
            0.01
        };
        ui.add(
            egui::DragValue::new(offset)
                .range(-1.0e12..=1.0e12)
                .speed(speed)
                .max_decimals(8)
                .suffix(if unit.is_empty() {
                    String::new()
                } else {
                    format!(" {unit}")
                }),
        )
        .on_hover_text("Vertical shift for this display");
        if *offset != 0.0 {
            ui.colored_label(ui.visuals().warn_fg_color, "Shifted");
            if ui.button("Reset").clicked() {
                *offset = 0.0;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Scale {
    pub display: Peak,
    pub samples: Peak,
}

impl Scale {
    pub fn new(bounds: Peak, offset: f32) -> Self {
        let mut display = bounds;
        // Retain the original axis extent and keep shifted samples in view.
        if bounds.valid() && offset != 0.0 {
            display.merge(Peak {
                min: bounds.min + offset,
                max: bounds.max + offset,
            });
        }
        let samples = Peak {
            min: display.min - offset,
            max: display.max - offset,
        };
        Self { display, samples }
    }
}

impl Viewer {
    pub(super) fn lane(&self, area: Rect, slot: usize) -> Lane {
        Lane::new(area, slot, self.lane_count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_scale_projects_raw_samples_at_shifted_display_values() {
        let bounds = Peak {
            min: -1.0,
            max: 2.0,
        };
        let axis = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 80.0));
        for offset in [-2.0, 0.0, 2.0] {
            let scale = Scale::new(bounds, offset);
            assert_eq!(
                plot::y_at(axis, scale.samples, 1.0),
                plot::y_at(axis, scale.display, 1.0 + offset)
            );
            assert!(scale.display.min <= bounds.min);
            assert!(scale.display.max >= bounds.max);
        }
    }
}
