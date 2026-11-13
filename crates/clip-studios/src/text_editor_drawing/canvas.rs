use super::*;
use little_clip_render::diagram::DrawingLayout;

#[derive(Default)]
pub(super) struct Camera {
    pub fitted: bool,
    center: [f64; 2],
    scale: f64,
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
            .clamp(0.1, 8.0);
        self.fitted = true;
    }
    pub fn screen(&self, point: [f64; 2], rect: Rect) -> Pos2 {
        rect.center()
            + egui::vec2(
                ((point[0] - self.center[0]) * self.scale) as f32,
                ((point[1] - self.center[1]) * self.scale) as f32,
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
                self.scale = (self.scale * (f64::from(scroll) * 0.003).exp()).clamp(0.1, 16.0);
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

#[derive(Clone)]
pub(super) enum Gesture {
    Move {
        start: [f64; 2],
        current: [f64; 2],
        ids: Vec<String>,
    },
    Draw {
        start: [f64; 2],
        current: [f64; 2],
        kind: Kind,
        points: Vec<[f64; 2]>,
    },
    Marquee {
        start: [f64; 2],
        current: [f64; 2],
        add: bool,
    },
}
impl Gesture {
    fn update(&mut self, point: [f64; 2], spacing: f64) {
        match self {
            Self::Move { current, .. } | Self::Marquee { current, .. } => *current = point,
            Self::Draw {
                current,
                kind,
                points,
                ..
            } => {
                *current = point;
                if *kind == Kind::Freedraw
                    && points
                        .last()
                        .is_none_or(|old| distance(*old, point) > spacing)
                {
                    if points.len() >= 8192 {
                        *points = points.iter().step_by(2).copied().collect();
                    }
                    points.push(point);
                }
            }
        }
    }
}
impl DrawingEditor {
    pub(super) fn interact(&mut self, ui: &mut egui::Ui, response: &egui::Response, rect: Rect) {
        let Some(model) = self.model.clone() else {
            return;
        };
        let pointer = ui.input(|input| input.pointer.interact_pos());
        if response.hovered()
            && ui.input(|input| input.pointer.primary_pressed())
            && let Some(pointer) = pointer
        {
            response.request_focus();
            self.begin(
                &model,
                self.camera.world(pointer, rect),
                ui.input(|input| input.modifiers.shift),
            );
        }
        if let Some(pointer) = pointer
            && let Some(gesture) = &mut self.gesture
        {
            gesture.update(self.camera.world(pointer, rect), 1.5 / self.camera.scale);
        }
        if ui.input(|input| input.pointer.primary_released())
            && let Some(gesture) = self.gesture.take()
        {
            self.finish(&model, gesture);
        }
    }

    pub(super) fn keyboard(&mut self, ui: &mut egui::Ui, response: &egui::Response) {
        // Selection commands use the current model, not rendered geometry.
        // Pointer hits stay gated on fresh pixels, but a preview refresh must
        // not swallow Delete or prevent cancelling a captured gesture.
        // egui clears focus before UI dispatch on Escape. Admit that loss only
        // when no other widget took focus, then keep the canvas keyboard owner.
        let escaped_focus = response.lost_focus()
            && ui.memory(|memory| memory.focused().is_none())
            && ui.input(|input| input.key_pressed(egui::Key::Escape));
        if response.has_focus() || escaped_focus {
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                response.request_focus();
                self.gesture = None;
                self.ghost = None;
                self.retained_move = None;
                self.selected.clear();
                self.tool = None;
            }
            let delete = ui.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    || input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
            });
            if delete && !self.selected.is_empty() {
                self.gesture = None;
                self.ghost = None;
                self.queue(Change::Delete {
                    ids: self.selected.clone(),
                });
            }
        }
    }
    fn begin(&mut self, model: &Scene, point: [f64; 2], shift: bool) {
        if let Some(kind) = &self.tool {
            self.gesture = Some(Gesture::Draw {
                start: point,
                current: point,
                kind: kind.clone(),
                points: vec![point],
            });
            return;
        }
        if let Some(item) = model
            .items
            .iter()
            .rev()
            .find(|item| hit(item, point, 6.0 / self.camera.scale))
        {
            if shift && self.selected.contains(&item.id) {
                self.selected.retain(|id| id != &item.id);
                return;
            }
            if !self.selected.contains(&item.id) {
                if !shift {
                    self.selected.clear();
                }
                self.selected.push(item.id.clone());
            }
            self.style = item.style.clone();
            match model.composition(&self.selected) {
                Ok(items) => {
                    self.selected = items.iter().map(|item| item.id.clone()).collect();
                    self.gesture = Some(Gesture::Move {
                        start: point,
                        current: point,
                        ids: self.selected.clone(),
                    });
                }
                Err(error) => self.edit_error = Some(error.to_string()),
            }
        } else {
            if !shift {
                self.selected.clear();
            }
            self.gesture = Some(Gesture::Marquee {
                start: point,
                current: point,
                add: shift,
            });
        }
    }
    fn finish(&mut self, model: &Scene, gesture: Gesture) {
        self.ghost = Some(gesture.clone());
        self.retained_move = None;
        match gesture {
            Gesture::Move {
                start,
                current,
                ids,
            } => {
                if distance(start, current) * self.camera.scale >= 2.0 {
                    // Keep the old vector scene in place through source/model
                    // publication until the replacement scene arrives.
                    self.retained_move = self.model.as_ref().map(|model| super::RetainedMove {
                        gesture: Gesture::Move {
                            start,
                            current,
                            ids: ids.clone(),
                        },
                        model: model.clone(),
                        lines: self.lines.clone(),
                    });
                    self.queue(Change::Move {
                        ids,
                        by: [current[0] - start[0], current[1] - start[1]],
                    });
                }
            }
            Gesture::Marquee {
                start,
                current,
                add,
            } => {
                if !add {
                    self.selected.clear();
                }
                for item in &model.items {
                    let center = item
                        .bounds
                        .to_world([item.bounds.width / 2.0, item.bounds.height / 2.0]);
                    if inside(center, start, current) && !self.selected.contains(&item.id) {
                        self.selected.push(item.id.clone());
                    }
                }
                self.ghost = None;
            }
            Gesture::Draw {
                start,
                current,
                kind,
                mut points,
            } => {
                if distance(start, current) * self.camera.scale < 2.0 && kind != Kind::Freedraw {
                    self.ghost = None;
                    return;
                }
                let path = [Kind::Line, Kind::Arrow, Kind::Freedraw].contains(&kind);
                let bounds = Bounds {
                    x: if path {
                        start[0]
                    } else {
                        start[0].min(current[0])
                    },
                    y: if path {
                        start[1]
                    } else {
                        start[1].min(current[1])
                    },
                    width: (current[0] - start[0]).abs(),
                    height: (current[1] - start[1]).abs(),
                    angle: 0.0,
                };
                if kind != Kind::Freedraw {
                    points = vec![start, current];
                } else if points.last() != Some(&current) {
                    points.push(current);
                }
                if points.len() < 2 {
                    self.ghost = None;
                    return;
                }
                let id = uuid::Uuid::new_v4().to_string();
                self.selected = vec![id.clone()];
                self.queue(Change::Create {
                    id,
                    kind,
                    bounds,
                    points: if path {
                        points
                            .into_iter()
                            .map(|p| [p[0] - start[0], p[1] - start[1]])
                            .collect()
                    } else {
                        Vec::new()
                    },
                    style: self.style.clone(),
                });
            }
        }
    }

    pub(super) fn paint_feedback(&self, painter: &egui::Painter, rect: Rect, palette: &AppPalette) {
        let accent = crate::hex_to_c32(&palette.harmonies.base);
        if let Some(model) = &self.model {
            let by = match self.gesture.as_ref().or(self.ghost.as_ref()) {
                Some(Gesture::Move { start, current, .. }) => {
                    [current[0] - start[0], current[1] - start[1]]
                }
                _ => [0.0; 2],
            };
            for item in &model.items {
                if !self.selected.contains(&item.id) {
                    continue;
                }
                let b = item.bounds;
                let corners = [
                    [0.0, 0.0],
                    [b.width, 0.0],
                    [b.width, b.height],
                    [0.0, b.height],
                    [0.0, 0.0],
                ];
                let points = corners
                    .map(|p| {
                        let p = b.to_world(p);
                        self.camera.screen([p[0] + by[0], p[1] + by[1]], rect)
                    })
                    .to_vec();
                painter.add(egui::Shape::line(
                    points,
                    egui::Stroke::new(1.5_f32, accent),
                ));
            }
        }
        match self.gesture.as_ref().or(self.ghost.as_ref()) {
            Some(Gesture::Marquee { start, current, .. }) => {
                let area = Rect::from_two_pos(
                    self.camera.screen(*start, rect),
                    self.camera.screen(*current, rect),
                );
                painter.rect_filled(area, 0.0, accent.gamma_multiply(0.12));
                painter.rect_stroke(area, 0.0, egui::Stroke::new(1.0_f32, accent));
            }
            Some(Gesture::Draw {
                start,
                current,
                kind,
                points,
            }) => {
                let a = self.camera.screen(*start, rect);
                let b = self.camera.screen(*current, rect);
                let stroke = egui::Stroke::new(
                    (self.style.width * self.camera.scale) as f32,
                    crate::hex_to_c32(&self.style.stroke),
                );
                match kind {
                    Kind::Freedraw => {
                        painter.add(egui::Shape::line(
                            points
                                .iter()
                                .map(|p| self.camera.screen(*p, rect))
                                .collect(),
                            stroke,
                        ));
                    }
                    Kind::Arrow => painter.arrow(a, b - a, stroke),
                    Kind::Line => {
                        painter.line_segment([a, b], stroke);
                    }
                    Kind::Ellipse => {
                        painter.add(egui::Shape::ellipse_stroke(
                            Rect::from_two_pos(a, b).center(),
                            (b - a).abs() / 2.0,
                            stroke,
                        ));
                    }
                    Kind::Diamond => {
                        let r = Rect::from_two_pos(a, b);
                        painter.add(egui::Shape::closed_line(
                            vec![
                                r.center_top(),
                                r.right_center(),
                                r.center_bottom(),
                                r.left_center(),
                            ],
                            stroke,
                        ));
                    }
                    _ => {
                        painter.rect_stroke(Rect::from_two_pos(a, b), 2.0, stroke);
                    }
                }
            }
            _ => {}
        }
    }
}
fn inside(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    (0..2).all(|i| p[i] >= a[i].min(b[i]) && p[i] <= a[i].max(b[i]))
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
fn hit(item: &Item, point: [f64; 2], tolerance: f64) -> bool {
    let p = item.bounds.to_local(point);
    if [Kind::Line, Kind::Arrow, Kind::Freedraw].contains(&item.kind) {
        return item.points.windows(2).any(|pair| {
            let d = [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]];
            let length = d[0] * d[0] + d[1] * d[1];
            let t = if length > 0.0 {
                ((p[0] - pair[0][0]) * d[0] + (p[1] - pair[0][1]) * d[1]) / length
            } else {
                0.0
            };
            distance(
                p,
                [
                    pair[0][0] + d[0] * t.clamp(0.0, 1.0),
                    pair[0][1] + d[1] * t.clamp(0.0, 1.0),
                ],
            ) <= tolerance
        });
    }
    let b = item.bounds;
    let radii = [b.width / 2.0 + tolerance, b.height / 2.0 + tolerance];
    if radii.into_iter().all(|radius| radius > 0.0) {
        let x = (p[0] - b.width / 2.0).abs() / radii[0];
        let y = (p[1] - b.height / 2.0).abs() / radii[1];
        match item.kind {
            Kind::Ellipse => return x * x + y * y <= 1.0,
            Kind::Diamond => return x + y <= 1.0,
            _ => {}
        }
    }
    inside(
        p,
        [-tolerance, -tolerance],
        [b.width + tolerance, b.height + tolerance],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        mut paint: impl FnMut(&mut egui::Ui),
    ) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, &mut paint);
            },
        );
    }

    #[test]
    fn drawing_keyboard_edits_current_selection_while_canonical_pixels_are_stale() {
        let ctx = egui::Context::default();
        let source = r#"{"type":"excalidraw","version":2,"elements":[{"id":"box","type":"rectangle","x":0,"y":0,"width":80,"height":60}]}"#;
        let mut editor = DrawingEditor {
            source: Some(Arc::from(source)),
            model: Some(Arc::new(Scene::parse(source).unwrap())),
            selected: vec!["box".into()],
            ..Default::default()
        };
        let mut preview = Preview::drawing_fixture(&ctx, "older drawing");
        let frame = |editor: &mut DrawingEditor, preview: &mut Preview, events| {
            ui_frame(&ctx, events, |ui| {
                editor.show(ui, source, preview, &AppPalette::default());
                assert!(!preview.drawing_frame(ui, source).unwrap().fresh);
            });
        };
        frame(&mut editor, &mut preview, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(editor.focus.unwrap()));
        editor.gesture = Some(Gesture::Move {
            start: [0.0; 2],
            current: [5.0; 2],
            ids: editor.selected.clone(),
        });
        for key in [egui::Key::Escape, egui::Key::Delete] {
            editor.selected = vec!["box".into()];
            frame(
                &mut editor,
                &mut preview,
                vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
            );
            assert!(editor.gesture.is_none());
        }
        assert!(
            editor.busy(),
            "Delete must reach the current model during rendering"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(Poll::Changed(updated)) = editor.poll(&ctx, source) {
                assert!(Scene::parse(&updated).unwrap().items.is_empty());
                assert!(editor.selected.is_empty());
                break;
            }
            assert!(Instant::now() < deadline, "{:?}", editor.edit_error);
            std::thread::sleep(Duration::from_millis(5));
        }
        preview.suspend();
    }

    #[test]
    fn drawing_curved_shape_selection_follows_rotated_geometry_instead_of_box_corners() {
        for kind in ["ellipse", "diamond"] {
            let source = serde_json::json!({
                "type": "excalidraw", "version": 2,
                "elements": [{"id": "shape", "type": kind, "x": 80, "y": -40,
                    "width": 120, "height": 60, "angle": std::f64::consts::FRAC_PI_2}]
            });
            let scene = Scene::parse(&source.to_string()).unwrap();
            let shape = &scene.items[0];
            for local in [[60.0, 30.0], [60.0, 0.0], [0.0, 30.0]] {
                assert!(
                    hit(shape, shape.bounds.to_world(local), 0.01),
                    "{kind} {local:?}"
                );
            }
            for local in [[2.0, 2.0], [118.0, 58.0], [62.0, -10.0]] {
                assert!(
                    !hit(shape, shape.bounds.to_world(local), 1.0),
                    "{kind} {local:?}"
                );
            }
            assert!(hit(shape, shape.bounds.to_world([60.0, -3.0]), 4.0));
        }
    }

    #[test]
    fn drawing_camera_keeps_source_precision_and_round_trips_large_origins() {
        let mut camera = Camera::default();
        let rect = Rect::from_min_size(egui::pos2(20.0, 30.0), egui::vec2(900.0, 500.0));
        let layout = DrawingLayout {
            origin: [900_000_000.0, -500_000_000.0],
            size: [400.0, 200.0],
        };
        camera.fit(layout, rect);
        let point = [layout.origin[0] + 127.25, layout.origin[1] + 82.5];
        let restored = camera.world(camera.screen(point, rect), rect);
        assert!((restored[0] - point[0]).abs() < 0.001);
        assert!((restored[1] - point[1]).abs() < 0.001);
    }

    #[test]
    fn drawing_pointer_release_outside_the_canvas_commits_one_complete_stroke() {
        let ctx = egui::Context::default();
        let source = r#"{"type":"excalidraw","version":2,"elements":[]}"#;
        let model = Arc::new(Scene::parse(source).unwrap());
        let mut editor = DrawingEditor {
            source: Some(Arc::from(source)),
            model: Some(model),
            tool: Some(Kind::Arrow),
            ..Default::default()
        };
        let rect = Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(300.0, 250.0));
        editor.camera.fit(
            DrawingLayout {
                origin: [0.0; 2],
                size: [100.0; 2],
            },
            rect,
        );
        let frame = |editor: &mut DrawingEditor, events| {
            ui_frame(&ctx, events, |ui| {
                let response = ui.interact(
                    rect,
                    egui::Id::new("drawing pointer fixture"),
                    egui::Sense::click_and_drag(),
                );
                editor.interact(ui, &response, rect);
            });
        };
        let a = egui::pos2(150.0, 150.0);
        let b = egui::pos2(510.0, 410.0);
        frame(&mut editor, Vec::new());
        frame(
            &mut editor,
            vec![
                egui::Event::PointerMoved(a),
                egui::Event::PointerButton {
                    pos: a,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        assert!(
            editor.gesture.is_some(),
            "pointer press starts the captured gesture"
        );
        frame(&mut editor, vec![egui::Event::PointerMoved(b)]);
        frame(
            &mut editor,
            vec![egui::Event::PointerButton {
                pos: b,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert!(editor.busy());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(Poll::Changed(source)) = editor.poll(&ctx, source) {
                let updated = Scene::parse(&source).unwrap();
                assert_eq!(updated.items.len(), 1);
                assert_eq!(updated.items[0].kind, Kind::Arrow);
                assert!(updated.items[0].bounds.width > 100.0);
                assert!(updated.items[0].bounds.height > 80.0);
                break;
            }
            assert!(Instant::now() < deadline, "{:?}", editor.edit_error);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(editor.gesture.is_none());
    }
}
