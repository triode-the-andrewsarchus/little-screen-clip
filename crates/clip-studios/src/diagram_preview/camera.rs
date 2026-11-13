//! Shared pan/zoom mapping for native diagram canvases.
use egui::{Pos2, Rect};
use little_clip_render::diagram::DrawingLayout;
const MIN_SCALE: f64 = 0.001;

#[derive(Default)]
pub(crate) struct Camera {
    pub fitted: bool,
    center: [f64; 2],
    pub(crate) scale: f64,
}

impl Camera {
    pub fn zoom_percent(&self) -> f64 {
        self.scale * 100.0
    }
    pub fn fit(&mut self, layout: DrawingLayout, rect: Rect) {
        self.center = [
            layout.origin[0] + layout.size[0] / 2.0,
            layout.origin[1] + layout.size[1] / 2.0,
        ];
        self.scale = ((f64::from(rect.width()) / layout.size[0])
            .min(f64::from(rect.height()) / layout.size[1])
            * 0.92)
            .clamp(MIN_SCALE, 8.0);
        self.fitted = true;
    }
    pub fn screen(&self, point: [f64; 2], rect: Rect) -> Pos2 {
        rect.center()
            + egui::vec2(
                ((point[0] - self.center[0]) * self.scale) as f32,
                ((point[1] - self.center[1]) * self.scale) as f32,
            )
    }
    /// Maps worker-derived source bounds through the same camera as the artwork.
    pub fn screen_rect(&self, bounds: Rect, rect: Rect) -> Rect {
        Rect::from_two_pos(
            self.screen([f64::from(bounds.left()), f64::from(bounds.top())], rect),
            self.screen(
                [f64::from(bounds.right()), f64::from(bounds.bottom())],
                rect,
            ),
        )
    }
    pub fn world(&self, point: Pos2, rect: Rect) -> [f64; 2] {
        [
            self.center[0] + f64::from(point.x - rect.center().x) / self.scale,
            self.center[1] + f64::from(point.y - rect.center().y) / self.scale,
        ]
    }
    pub fn image_rect(&self, layout: DrawingLayout, rect: Rect) -> Rect {
        Rect::from_min_size(
            self.screen(layout.origin, rect),
            egui::vec2(
                (layout.size[0] * self.scale) as f32,
                (layout.size[1] * self.scale) as f32,
            ),
        )
    }
    pub fn input(&mut self, ui: &egui::Ui, response: &egui::Response, rect: Rect) {
        if response.hovered() {
            let scroll = ui.input(|input| input.raw_scroll_delta.y);
            if scroll != 0.0
                && let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
            {
                let before = self.world(pointer, rect);
                self.scale =
                    (self.scale * (f64::from(scroll) * 0.003).exp()).clamp(MIN_SCALE, 16.0);
                let after = self.world(pointer, rect);
                self.center = [
                    self.center[0] + before[0] - after[0],
                    self.center[1] + before[1] - after[1],
                ];
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            let delta = ui.input(|input| input.pointer.delta());
            self.center[0] -= f64::from(delta.x) / self.scale;
            self.center[1] -= f64::from(delta.y) / self.scale;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fit_keeps_a_large_native_diagram_inside_the_viewport() {
        let rect = Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(800.0, 400.0));
        let layout = DrawingLayout {
            origin: [0.0; 2],
            size: [16384.0; 2],
        };
        let mut camera = Camera::default();
        camera.fit(layout, rect);
        let image = camera.image_rect(layout, rect);
        assert!(rect.contains_rect(image));
        assert!(image.height() >= rect.height() * 0.9);
        assert!(camera.scale < 0.1);
    }
}
