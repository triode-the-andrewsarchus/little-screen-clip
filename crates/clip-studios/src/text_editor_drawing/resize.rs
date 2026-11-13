//! Native corner handles and retained live artwork transforms.
use super::*;

pub(super) fn corners(bounds: Bounds) -> [[f64; 2]; 4] {
    [
        [bounds.x, bounds.y],
        [bounds.x + bounds.width, bounds.y],
        [bounds.x + bounds.width, bounds.y + bounds.height],
        [bounds.x, bounds.y + bounds.height],
    ]
}
pub(super) fn resized(
    original: Bounds,
    point: [f64; 2],
    corner: usize,
    proportional: bool,
) -> Bounds {
    let opposite = corners(original)[(corner + 2) % 4];
    let left = corner == 0 || corner == 3;
    let top = corner < 2;
    let mut width = if left {
        opposite[0] - point[0]
    } else {
        point[0] - opposite[0]
    }
    .max(1.0);
    let mut height = if top {
        opposite[1] - point[1]
    } else {
        point[1] - opposite[1]
    }
    .max(1.0);
    if proportional {
        let scale = (width / original.width).max(height / original.height);
        width = original.width * scale;
        height = original.height * scale;
    }
    Bounds {
        x: if left {
            opposite[0] - width
        } else {
            opposite[0]
        },
        y: if top {
            opposite[1] - height
        } else {
            opposite[1]
        },
        width,
        height,
        angle: 0.0,
    }
}

impl DrawingEditor {
    pub(super) fn resize_handle(
        &self,
        model: &Scene,
        point: [f64; 2],
    ) -> Option<(String, Bounds, usize, bool)> {
        if self.tool.is_some() || !self.resize_available {
            return None;
        }
        let id = self.selected.first()?;
        let item = model.items.iter().find(|item| &item.id == id)?;
        if model.resize(id, item.bounds).is_err() {
            return None;
        }
        corners(item.bounds)
            .iter()
            .position(|corner| distance(*corner, point) <= 8.0 / self.camera.scale)
            .map(|corner| {
                (
                    id.clone(),
                    item.bounds,
                    corner,
                    matches!(item.kind, Kind::Image | Kind::Text),
                )
            })
    }

    pub(super) fn resize_motion(
        &mut self,
        ctx: &egui::Context,
        model: &Arc<Scene>,
        lines: &Arc<Lines>,
        id: &str,
        bounds: Bounds,
        origin: [f64; 2],
    ) -> Result<clip_ui::vectors::svg::Changes, String> {
        let plan = model.resize(id, bounds).map_err(|e| e.to_string())?;
        let old = model
            .items
            .iter()
            .find(|item| item.id == id)
            .ok_or("Resize element disappeared")?
            .bounds;
        let sx = (bounds.width / old.width) as f32;
        let sy = (bounds.height / old.height) as f32;
        let mut changes = self.movement_artwork(ctx, model, lines, plan.movement);
        changes.transforms.insert(
            group_id("element", id),
            [
                sx,
                0.0,
                0.0,
                sy,
                (bounds.x - origin[0] - bounds.width / old.width * (old.x - origin[0])) as f32,
                (bounds.y - origin[1] - bounds.height / old.height * (old.y - origin[1])) as f32,
            ],
        );
        Ok(changes)
    }

    pub(super) fn movement_artwork(
        &mut self,
        ctx: &egui::Context,
        model: &Arc<Scene>,
        lines: &Arc<Lines>,
        movement: little_clip_core::diagram::drawing::Movement,
    ) -> clip_ui::vectors::svg::Changes {
        clip_ui::vectors::svg::Changes {
            translations: movement
                .offsets
                .into_iter()
                .map(|(id, [x, y])| (group_id("element", &id), [x as f32, y as f32]))
                .collect(),
            replacements: self.connections.update(ctx, model, lines, movement.paths),
            ..Default::default()
        }
    }

    pub(super) fn paint_resize_handles(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        accent: Color32,
    ) {
        let (Some(model), Some(id)) = (&self.model, self.selected.first()) else {
            return;
        };
        let Some(item) = model.items.iter().find(|item| &item.id == id) else {
            return;
        };
        let bounds = match self.gesture.as_ref().or(self.ghost.as_ref()) {
            Some(Gesture::Resize { current, .. }) => *current,
            _ => item.bounds,
        };
        if self.tool.is_some() || !self.resize_available || model.resize(id, bounds).is_err() {
            return;
        }
        for corner in corners(bounds) {
            let point = self.camera.screen(corner, rect);
            painter.rect_filled(
                Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                1.0,
                Color32::WHITE,
            );
            painter.rect_stroke(
                Rect::from_center_size(point, egui::vec2(8.0, 8.0)),
                1.0,
                egui::Stroke::new(1.0_f32, accent),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corners_hold_the_opposite_anchor_and_proportional_resize_preserves_aspect() {
        let original = Bounds {
            x: -20.0,
            y: 15.0,
            width: 100.0,
            height: 50.0,
            angle: 0.0,
        };
        for corner in 0..4 {
            let point = corners(original)[corner];
            let moved = [
                point[0]
                    + if corner == 0 || corner == 3 {
                        -40.0
                    } else {
                        40.0
                    },
                point[1] + if corner < 2 { -30.0 } else { 30.0 },
            ];
            let next = resized(original, moved, corner, false);
            assert_eq!(
                corners(next)[(corner + 2) % 4],
                corners(original)[(corner + 2) % 4]
            );
            assert_eq!((next.width, next.height), (140.0, 80.0));
            let proportional = resized(original, moved, corner, true);
            assert!((proportional.width / proportional.height - 2.0).abs() < 1e-9);
        }
    }
    #[test]
    fn native_handles_plan_actual_artwork_and_bound_paths_without_source_mutation() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let model = Arc::new(Scene::parse(source).unwrap());
        let lines = Arc::new(little_clip_render::diagram::retained::drawing_lines(source).unwrap());
        let mut editor = DrawingEditor {
            model: Some(model.clone()),
            resize_available: true,
            selected: vec!["capture".into(), "capture-label".into()],
            ..Default::default()
        };
        editor.camera.scale = 1.0;
        let handle = editor.resize_handle(&model, [180.0, 150.0]).unwrap();
        assert_eq!(handle.2, 2);
        let bounds = Bounds {
            x: 0.0,
            y: 40.0,
            width: 220.0,
            height: 130.0,
            angle: 0.0,
        };
        let changes = editor
            .resize_motion(
                &egui::Context::default(),
                &model,
                &lines,
                "capture",
                bounds,
                [-10.0, 30.0],
            )
            .unwrap();
        let expected = [
            220.0 / 180.0,
            0.0,
            0.0,
            130.0 / 110.0,
            10.0 - (220.0 / 180.0) * 10.0,
            10.0 - (130.0 / 110.0) * 10.0,
        ];
        for (actual, expected) in changes.transforms[&group_id("element", "capture")]
            .iter()
            .zip(expected)
        {
            assert!((f64::from(*actual) - expected).abs() < 1e-5);
        }
        assert!(
            !changes
                .translations
                .contains_key(&group_id("element", "capture"))
        );
        assert_eq!(model.source(), source);
    }
}
