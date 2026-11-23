use super::{Viewer, plot};
use egui::{Align, Align2, Color32, FontId, Layout, Painter, Rect, Ui, UiBuilder, pos2, vec2};
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
        plot::paint_y_scale(painter, self.plot, bounds, unit, color);
    }

    pub fn heading(self, ui: &mut Ui, id: impl egui::AsIdSalt) -> Ui {
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(id)
                .max_rect(self.header.shrink2(vec2(7.0, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        child.set_clip_rect(ui.clip_rect().intersect(self.header));
        child.spacing_mut().interact_size.y = 18.0;
        child.spacing_mut().button_padding.y = 1.0;
        child
    }
}

pub(super) fn shifted(bounds: Peak, offset: f32) -> Peak {
    if !bounds.valid() {
        return bounds;
    }
    Peak {
        min: bounds.min + offset,
        max: bounds.max + offset,
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
            let mut display = bounds;
            display.merge(shifted(bounds, offset));
            let samples = shifted(display, -offset);
            assert_eq!(
                plot::y_at(axis, samples, 1.0),
                plot::y_at(axis, display, 1.0 + offset)
            );
        }
    }
}
