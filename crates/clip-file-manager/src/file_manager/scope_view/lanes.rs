use super::{Viewer, plot};
use egui::{
    Align, Align2, Color32, FontId, Layout, Painter, Rect, RichText, Ui, UiBuilder, pos2, vec2,
};
use little_clip_core::engineering::waveforms::Peak;

const HEADER_HEIGHT: f32 = 24.0;
pub(in crate::file_manager) const COLLAPSED_HEIGHT: f32 = 25.0;
pub(super) const MIN_HEIGHT: f32 = 68.0;

#[derive(Clone, Copy)]
pub(in crate::file_manager) struct Lane {
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

    pub fn comparison(area: Rect, visible: bool) -> Self {
        if visible {
            return Self::new(
                Rect::from_min_max(area.min + vec2(78.0, 0.0), area.max),
                0,
                1,
            );
        }
        let frame = area.shrink2(vec2(0.0, 1.0));
        Self {
            plot: Rect::NOTHING,
            header: frame,
            frame,
        }
    }

    pub fn background(self, ui: &Ui, painter: &Painter) {
        painter.rect_filled(self.frame, 3.0, ui.visuals().faint_bg_color);
        if self.plot.is_positive() {
            painter.rect_filled(self.plot, 0.0, ui.visuals().extreme_bg_color);
        }
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

pub(in crate::file_manager) fn trace_header<T: Copy + Send + Sync + 'static>(
    ui: &mut Ui,
    key: T,
    label: &str,
    source: Option<&str>,
    color: Color32,
    active: bool,
    visible: bool,
) -> (bool, bool) {
    use egui::{Button, CursorIcon, Label, Sense};
    let handle = ui.add_sized([21.0, 18.0], Button::new("⠿").sense(Sense::drag()));
    handle.dnd_set_drag_payload(key);
    if handle.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    }
    handle.on_hover_text("Drag to move this trace above or below another trace");
    let size = ui.available_width();
    let label_width = (size * 0.32).clamp(92.0, 148.0);
    let label = if active {
        RichText::new(label)
    } else {
        RichText::new(label).color(color)
    };
    let selected = ui
        .add_sized(
            [label_width, 18.0],
            Button::new(label).selected(active).truncate(),
        )
        .on_hover_text("Select this trace for measurements, spectrum and export")
        .clicked();
    if let Some(source) = source {
        let width = (size - label_width - 130.0).clamp(35.0, 188.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center))
                .id_salt("trace-source"),
        )
        .add(Label::new(RichText::new(source).weak()).truncate())
        .on_hover_text(source);
    }
    let caption = if visible { "Hide" } else { "Show" };
    let visibility = ui.add_sized([48.0, 18.0], Button::new(caption)).clicked();
    (selected, visibility)
}

pub(in crate::file_manager) fn trace_drop<T: Copy + Eq + Send + Sync + 'static>(
    ui: &Ui,
    rect: Rect,
    target: T,
) -> Option<(T, T, bool)> {
    let payload = egui::DragAndDrop::payload::<T>(ui.ctx())?;
    let pointer = ui.input(|input| input.pointer.latest_pos())?;
    if *payload == target || !rect.intersect(ui.clip_rect()).contains(pointer) {
        return None;
    }
    let after = pointer.y >= rect.center().y;
    let edge = if after { rect.bottom() } else { rect.top() };
    ui.painter()
        .hline(rect.x_range(), edge, ui.visuals().selection.stroke);
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary)) {
        egui::DragAndDrop::take_payload::<T>(ui.ctx()).map(|payload| (*payload, target, after))
    } else {
        None
    }
}

pub(in crate::file_manager) fn move_trace<T: Copy + Eq>(
    order: &mut Vec<T>,
    (from, to, after): (T, T, bool),
) {
    if from == to {
        return;
    }
    if let Some(index) = order.iter().position(|&key| key == from)
        && let Some(target) = order.iter().position(|&key| key == to)
    {
        let adjusted = target - usize::from(index < target);
        order.remove(index);
        order.insert(adjusted + usize::from(after), from);
    }
}

pub(in crate::file_manager) fn coverage_note(
    painter: &Painter,
    rect: Rect,
    viewport: &std::ops::Range<f64>,
    recorded: std::ops::Range<f64>,
    ink: Color32,
) {
    let start = recorded.start.max(viewport.start);
    let end = recorded.end.min(viewport.end);
    let note = if start >= end {
        "Outside this time range"
    } else if plot::x_at(rect, viewport, end) - plot::x_at(rect, viewport, start) < 2.0 {
        "Zoom in to see this recording"
    } else {
        return;
    };
    painter.text(
        rect.left_center() + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        note,
        FontId::proportional(11.0),
        ink,
    );
}

pub(in crate::file_manager) fn offset_control(
    ui: &mut Ui,
    unit: &str,
    bounds: Peak,
    offset: &mut f32,
) {
    let span = f64::from(bounds.max) - f64::from(bounds.min);
    let speed = if span.is_finite() && span > 0.0 {
        span / 100.0
    } else {
        0.01
    };
    let caption = if *offset != 0.0 {
        RichText::new("Shifted ").color(ui.visuals().warn_fg_color)
    } else if unit == "V" {
        RichText::new("V offset ")
    } else {
        RichText::new("Offset ")
    };
    let response = ui
        .add(
            egui::DragValue::new(offset)
                .range(-1.0e12..=1.0e12)
                .speed(speed)
                .max_decimals(8)
                .prefix(caption)
                .suffix(if unit.is_empty() {
                    String::new()
                } else {
                    format!(" {unit}")
                }),
        )
        .on_hover_text("Display offset. Measurements and export use the unshifted samples.");
    if *offset != 0.0
        && ui
            .small_button("Reset")
            .on_hover_text("Reset display offset")
            .clicked()
    {
        *offset = 0.0;
        ui.ctx().request_repaint();
    } else if response.changed() {
        ui.ctx().request_repaint();
    }
}

pub(in crate::file_manager) fn shifted(bounds: Peak, offset: f32) -> Peak {
    if !bounds.valid() {
        return bounds;
    }
    Peak {
        min: bounds.min + offset,
        max: bounds.max + offset,
    }
}

pub(in crate::file_manager) fn stacked_scale(mut bounds: Peak, offset: f32) -> Peak {
    // Keep the original reference and the shifted waveform on the same axis.
    bounds.merge(shifted(bounds, offset));
    bounds
}

impl Viewer {
    pub(super) fn lane(&self, area: Rect, slot: usize) -> Lane {
        if !self.has_collapsed_rows() {
            return Lane::new(area, slot, self.lane_count());
        }
        let hidden = self.visible.iter().filter(|&&visible| !visible).count();
        let collapsed_before = self.channel_order[..slot.min(self.channel_order.len())]
            .iter()
            .filter(|&&index| !self.visible[index])
            .count();
        let open = (self.lane_count() - hidden).max(1);
        let tall = (area.height() - hidden as f32 * COLLAPSED_HEIGHT) / open as f32;
        let y = area.top()
            + (slot - collapsed_before) as f32 * tall
            + collapsed_before as f32 * COLLAPSED_HEIGHT;
        let collapsed = self
            .channel_order
            .get(slot)
            .is_some_and(|&index| !self.visible[index]);
        let height = if collapsed { COLLAPSED_HEIGHT } else { tall };
        Lane::comparison(
            Rect::from_min_size(
                pos2(area.left() - 78.0, y),
                vec2(area.width() + 78.0, height),
            ),
            !collapsed,
        )
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
            let display = stacked_scale(bounds, offset);
            let samples = shifted(display, -offset);
            assert!(display.min <= bounds.min && display.max >= bounds.max);
            assert!(display.min <= bounds.min + offset && display.max >= bounds.max + offset);
            let shifted_y = plot::y_at(axis, samples, 1.0);
            let original_y = plot::y_at(axis, bounds, 1.0);
            assert_eq!(shifted_y, plot::y_at(axis, display, 1.0 + offset));
            assert_eq!(shifted_y.total_cmp(&original_y), 0.0_f32.total_cmp(&offset));
        }
    }
}
