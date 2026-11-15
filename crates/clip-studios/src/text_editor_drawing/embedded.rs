//! Embeds native drawing tools alongside host-owned live content.
//!
//! The host owns source history and durable writes. Supply returned source edits
//! on the next frame; overlays use the exact camera used to paint the artwork.
use super::*;
pub use crate::diagram_preview::ViewState;

/// Controls interaction and reserves world-space areas for live widgets.
#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Uses a themed composition grid without document-editing controls.
    ///
    /// Primary input belongs to host placement/activation; only middle drag pans.
    /// Existing drawing source is displayed as transparent legacy artwork.
    pub composition: bool,
    /// Enables drawing tools and properties rather than shortcut browsing.
    pub editable: bool,
    /// Temporarily disables edits without hiding or moving their controls.
    pub enabled: bool,
    /// Prevents drawing or shortcut activation through these widget bounds.
    pub excluded: &'a [Bounds],
    /// Includes live content in Fit without creating artificial drawing shapes.
    pub extent: Option<Bounds>,
}
impl Options<'_> {
    /// Returns ordinary drawing-editor interaction with no live overlays.
    pub fn editing() -> Self {
        Self {
            composition: false,
            editable: true,
            enabled: true,
            excluded: &[],
            extent: None,
        }
    }
}

/// Maps live content through the drawing surface's current camera.
pub struct Viewport<'a> {
    camera: &'a Camera,
    /// Contains the clipped drawing surface in screen coordinates.
    pub rect: Rect,
}
impl Viewport<'_> {
    /// Maps a world point to screen coordinates.
    pub fn screen(&self, point: [f64; 2]) -> egui::Pos2 {
        self.camera.screen(point, self.rect)
    }
    /// Maps a screen point to world coordinates.
    pub fn world(&self, point: egui::Pos2) -> [f64; 2] {
        self.camera.world(point, self.rect)
    }
    /// Returns screen pixels per world unit.
    pub fn scale(&self) -> f64 {
        self.camera.scale
    }
    /// Maps an unrotated live widget into screen coordinates.
    pub fn bounds(&self, bounds: Bounds) -> Rect {
        Rect::from_two_pos(
            self.screen([bounds.x, bounds.y]),
            self.screen([bounds.x + bounds.width, bounds.y + bounds.height]),
        )
    }
}

/// Reports a source transaction or a browse-mode artwork shortcut click.
#[derive(Default)]
pub struct Response {
    /// Contains accepted drawing source for the host's history and save owner.
    pub changed: Option<String>,
    /// Identifies clicked artwork without imposing a URL or filesystem policy.
    pub activated: Option<String>,
}

/// Owns the existing native drawing editor and its bounded preview preparation.
#[derive(Default)]
pub struct Canvas {
    editor: DrawingEditor,
    preview: Preview,
}
impl Canvas {
    /// Reports drawing text that has not been submitted through Apply text.
    pub fn has_unapplied_text(&self) -> bool {
        if self.editor.busy() {
            return false;
        }
        self.editor
            .text_draft
            .as_ref()
            .is_some_and(|draft| match &draft.target {
                text::TextTarget::New { .. } => true,
                text::TextTarget::Existing(id) => self
                    .editor
                    .model
                    .as_ref()
                    .and_then(|model| model.text_info(id).ok().flatten())
                    .is_none_or(|info| info.text != draft.value),
            })
    }
    /// Discards only unsubmitted text; accepted drawing transactions retain ownership.
    pub fn discard_text(&mut self) {
        if !self.editor.busy() {
            self.editor.text_draft = None;
            self.editor.tool = None;
            self.editor.text_focus = false;
        }
    }
    /// Returns the current finite camera after its first completed Fit.
    pub fn view(&self) -> Option<ViewState> {
        self.editor.camera.view()
    }
    /// Restores a saved camera, returning false without changes for invalid input.
    pub fn restore_view(&mut self, view: ViewState) -> bool {
        self.editor.camera.restore(view)
    }
    /// Returns selected artwork IDs for host-managed shortcut binding.
    pub fn selected(&self) -> &[String] {
        &self.editor.selected
    }
    /// Returns whether an accepted drawing edit is awaiting publication.
    pub fn busy(&self) -> bool {
        self.editor.busy()
    }
    /// Reports a preparation or accepted-edit failure.
    pub fn error(&self) -> Option<&str> {
        self.editor.parse_error.as_deref().or(self.editor.error())
    }
    /// Cancels an active gesture, tool, selection or text draft without cancelling accepted edits.
    pub fn cancel(&mut self) -> bool {
        self.editor.cancel_interaction()
    }
    /// Requests Fit once current artwork is ready.
    pub fn fit(&mut self) {
        self.editor.camera.fitted = false;
    }
    /// Retires hidden read preparation while retaining accepted source edits.
    pub fn suspend(&mut self) {
        self.editor.suspend();
    }
    /// Publishes accepted source edits even while the surface is hidden.
    ///
    /// The host must retain the returned source as its current draft before
    /// polling again. Durable save ownership remains with the host.
    pub fn poll(&mut self, ctx: &egui::Context, source: &str) -> Option<String> {
        match self.editor.poll(ctx, source) {
            Some(Poll::Changed(source)) => Some(source),
            _ => None,
        }
    }
    /// Displays drawing artwork and invokes the host's live overlay painter.
    ///
    /// `overlay` receives the same world mapping used by native vectors. Bounds
    /// in `options.excluded` must describe live interactive content before the
    /// frame, so pencil/selection gestures cannot start through its controls.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &str,
        palette: &AppPalette,
        options: Options<'_>,
        overlay: impl FnOnce(&mut egui::Ui, Viewport<'_>),
    ) -> Response {
        let changed = self.poll(ui.ctx(), source);
        self.preview.set_drawing_transparency(options.composition);
        let current = changed.as_deref().unwrap_or(source);
        self.editor.prepare_model(current);
        if options.editable && !options.composition {
            ui.add_enabled_ui(options.enabled, |ui| self.editor.toolbar(ui, palette));
        }
        let size = ui.available_size().max(Vec2::splat(1.0));
        let (_, activated) = ui
            .horizontal_top(|ui| {
                if options.editable && !options.composition {
                    let width = (size.x * 0.22).clamp(170.0, 216.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(width, size.y),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_source("embedded_drawing_properties")
                                .min_scrolled_height(0.0)
                                .max_height(size.y)
                                .show(ui, |ui| {
                                    ui.add_enabled_ui(options.enabled, |ui| {
                                        self.editor.text_controls(ui);
                                        self.editor.properties(ui, palette);
                                    });
                                });
                        },
                    );
                }
                let (rect, activated) = self.editor.canvas_ui(
                    ui,
                    Vec2::new(ui.available_width().max(1.0), size.y),
                    current,
                    &mut self.preview,
                    palette,
                    options,
                );
                let mut clipped = ui.child_ui(rect, egui::Layout::top_down(egui::Align::Min));
                clipped.set_clip_rect(rect.intersect(ui.clip_rect()));
                overlay(
                    &mut clipped,
                    Viewport {
                        camera: &self.editor.camera,
                        rect,
                    },
                );
                (rect, activated)
            })
            .inner;
        Response { changed, activated }
    }
}

pub(super) fn composition_grid(
    painter: &egui::Painter,
    camera: &Camera,
    rect: Rect,
    palette: &AppPalette,
) {
    let colors = clip_ui::ui_appearance::WorkbenchColors::from_palette(palette);
    painter.rect_filled(rect, 0.0, colors.canvas);
    let mut step = 32.0;
    while step * camera.scale < 24.0 {
        step *= 2.0;
    }
    while step * camera.scale > 72.0 {
        step *= 0.5;
    }
    while (f64::from(rect.width() * rect.height()) / (step * camera.scale).powi(2)) > 4096.0 {
        step *= 2.0;
    }
    let start = camera.world(rect.min, rect);
    let end = camera.world(rect.max, rect);
    let ink = colors.text_muted.gamma_multiply(0.35);
    for y in (start[1] / step).floor() as i64..=(end[1] / step).ceil() as i64 {
        for x in (start[0] / step).floor() as i64..=(end[0] / step).ceil() as i64 {
            let point = camera.screen([x as f64 * step, y as f64 * step], rect);
            if rect.contains(point) {
                painter.circle_filled(point, 0.9, ink);
            }
        }
    }
}

pub(super) fn fit_layout(
    layout: little_clip_render::diagram::DrawingLayout,
    extent: Option<Bounds>,
) -> little_clip_render::diagram::DrawingLayout {
    let Some(bounds) = extent.filter(|bounds| bounds.valid()) else {
        return layout;
    };
    let min = [
        layout.origin[0].min(bounds.x),
        layout.origin[1].min(bounds.y),
    ];
    let max = [
        (layout.origin[0] + layout.size[0]).max(bounds.x + bounds.width),
        (layout.origin[1] + layout.size[1]).max(bounds.y + bounds.height),
    ];
    little_clip_render::diagram::DrawingLayout {
        origin: min,
        size: [(max[0] - min[0]).max(1.0), (max[1] - min[1]).max(1.0)],
    }
}

pub(super) fn pointer_blocked(
    ui: &egui::Ui,
    camera: &Camera,
    rect: Rect,
    bounds: &[Bounds],
) -> bool {
    ui.input(|input| input.pointer.interact_pos())
        .is_some_and(|pointer| {
            let point = camera.world(pointer, rect);
            bounds.iter().filter(|bounds| bounds.valid()).any(|bounds| {
                let local = bounds.to_local(point);
                (0.0..=bounds.width).contains(&local[0])
                    && (0.0..=bounds.height).contains(&local[1])
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_render::diagram::DrawingLayout;

    #[test]
    fn composition_primary_drag_does_not_pan_but_middle_drag_does() {
        for button in [egui::PointerButton::Primary, egui::PointerButton::Middle] {
            let mut editor = DrawingEditor::default();
            editor.camera.restore(ViewState {
                center: [0.0; 2],
                scale: 1.0,
            });
            let ctx = egui::Context::default();
            let start = egui::pos2(300.0, 200.0);
            for frame in 0..3 {
                let events = match frame {
                    1 => vec![
                        egui::Event::PointerMoved(start),
                        egui::Event::PointerButton {
                            pos: start,
                            button,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    2 => vec![egui::Event::PointerMoved(start + egui::vec2(40.0, 20.0))],
                    _ => vec![],
                };
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 600.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let (rect, response) = ui.allocate_exact_size(
                                ui.available_size(),
                                egui::Sense::click_and_drag(),
                            );
                            editor.camera.input(ui, &response, rect);
                            editor.surface_input(
                                ui,
                                &response,
                                rect,
                                false,
                                Options {
                                    composition: true,
                                    editable: false,
                                    enabled: true,
                                    excluded: &[],
                                    extent: None,
                                },
                            );
                        });
                    },
                );
            }
            assert_eq!(
                editor.camera.view().unwrap().center,
                if button == egui::PointerButton::Middle {
                    [-40.0, -20.0]
                } else {
                    [0.0, 0.0]
                }
            );
        }
    }

    #[test]
    fn only_changed_unsubmitted_text_requires_a_leave_decision() {
        let source = include_str!("../../../../docs/examples/diagrams/drawing.excalidraw");
        let model = Scene::parse(source).unwrap();
        let original = model.text_info("capture-label").unwrap().unwrap().text;
        let mut canvas = Canvas::default();
        canvas.editor.model = Some(Arc::new(model));
        canvas.editor.text_draft = Some(TextDraft {
            target: text::TextTarget::Existing("capture-label".into()),
            value: original,
        });
        assert!(!canvas.has_unapplied_text());
        canvas
            .editor
            .text_draft
            .as_mut()
            .unwrap()
            .value
            .push_str(" changed");
        assert!(canvas.has_unapplied_text());
        canvas.discard_text();
        assert!(canvas.editor.text_draft.is_none());
        canvas.editor.text_draft = Some(TextDraft {
            target: text::TextTarget::New {
                id: "new-label".into(),
                at: [10.0, 20.0],
                container: None,
                style: Style::default(),
            },
            value: "Unsubmitted".into(),
        });
        assert!(canvas.has_unapplied_text());
        canvas.editor.queued = Some(PendingEdit {
            change: Action::Text(canvas.editor.text_draft.clone().unwrap()),
            started: Instant::now(),
        });
        assert!(!canvas.has_unapplied_text());
        canvas.discard_text();
        assert!(canvas.editor.queued.is_some());
        assert!(canvas.editor.text_draft.is_some());
    }

    #[test]
    fn live_extent_participates_in_fit_and_overlay_mapping_survives_zoom_and_pan() {
        let layout = DrawingLayout {
            origin: [0.0; 2],
            size: [100.0; 2],
        };
        let bounds = Bounds {
            x: 600.0,
            y: -150.0,
            width: 280.0,
            height: 200.0,
            angle: 0.0,
        };
        let combined = fit_layout(layout, Some(bounds));
        let rect = Rect::from_min_size(egui::pos2(25.0, 20.0), egui::vec2(900.0, 600.0));
        let mut camera = Camera::default();
        camera.fit(combined, rect);
        let view = Viewport {
            camera: &camera,
            rect,
        };
        assert!(rect.contains_rect(view.bounds(bounds)));
        camera.pan(egui::vec2(100.0, -60.0));
        camera.scale *= 2.5;
        let view = Viewport {
            camera: &camera,
            rect,
        };
        for point in [[600.0, -150.0], [880.0, 50.0]] {
            let result = view.world(view.screen(point));
            assert!((result[0] - point[0]).abs() < 0.001);
            assert!((result[1] - point[1]).abs() < 0.001);
        }
    }

    #[test]
    fn browsing_artwork_activates_ids_and_live_widgets_block_underlying_gestures() {
        let ctx = egui::Context::default();
        let source = serde_json::json!({"type":"excalidraw", "version":2, "elements":[{
            "id":"shortcut", "type":"rectangle", "x":10, "y":10,
            "width":80, "height":60, "angle":0, "locked":false
        }]})
        .to_string();
        let model = Arc::new(Scene::parse(&source).unwrap());
        let mut canvas = Canvas {
            editor: DrawingEditor {
                source: Some(Arc::from(source.as_str())),
                model: Some(model),
                ..Default::default()
            },
            preview: Preview::drawing_fixture(&ctx, &source),
        };
        let mut point = egui::Pos2::ZERO;
        let mut frame = |canvas: &mut Canvas, events, editable, enabled, excluded: &[Bounds]| {
            let mut response = Response::default();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        response = canvas.show(
                            ui,
                            &source,
                            &AppPalette::default(),
                            Options {
                                composition: false,
                                editable,
                                enabled,
                                excluded,
                                extent: None,
                            },
                            |_, view| {
                                point = view.screen([50.0, 40.0]);
                            },
                        );
                    });
                },
            );
            (response, point)
        };
        let (_, point) = frame(&mut canvas, vec![], false, true, &[]);
        let pointer = |pressed| {
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let _ = frame(&mut canvas, pointer(true), false, true, &[]);
        let (clicked, _) = frame(&mut canvas, pointer(false), false, true, &[]);
        assert_eq!(clicked.activated.as_deref(), Some("shortcut"));
        assert!(!canvas.busy());
        let block = Bounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            angle: 0.0,
        };
        let _ = frame(&mut canvas, pointer(true), false, true, &[block]);
        let (blocked, _) = frame(&mut canvas, pointer(false), false, true, &[block]);
        assert!(blocked.activated.is_none());
        assert!(canvas.editor.gesture.is_none());
        let (_, interactive) = frame(&mut canvas, vec![], true, true, &[]);
        let (_, disabled) = frame(&mut canvas, vec![], true, false, &[]);
        assert_eq!(
            interactive, disabled,
            "temporarily disabling edits must keep the canvas mapping stable"
        );
        for pressed in [true, false] {
            let _ = frame(
                &mut canvas,
                vec![
                    egui::Event::PointerMoved(disabled),
                    egui::Event::PointerButton {
                        pos: disabled,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                true,
                false,
                &[],
            );
        }
        assert!(canvas.editor.gesture.is_none());
        assert!(!canvas.busy());
        assert_eq!(canvas.editor.model.as_ref().unwrap().source(), source);
    }
}

#[cfg(test)]
mod pan_tests {
    use super::*;
    #[test]
    fn primary_pan_paints_artwork_and_live_content_with_the_same_frames_camera() {
        let ctx = egui::Context::default();
        let source = r#"{"type":"excalidraw","version":2,"elements":[]}"#;
        let preview = Preview::drawing_fixture(&ctx, source);
        let mut canvas = Canvas {
            editor: DrawingEditor {
                source: Some(Arc::from(source)),
                model: Some(Arc::new(Scene::parse(source).unwrap())),
                ..Default::default()
            },
            preview,
        };
        let mut overlay = Rect::NOTHING;
        let mut render = |canvas: &mut Canvas, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        canvas.show(
                            ui,
                            source,
                            &AppPalette::default(),
                            Options {
                                composition: false,
                                editable: false,
                                enabled: true,
                                excluded: &[],
                                extent: None,
                            },
                            |_, view| {
                                overlay = view.bounds(Bounds {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 100.0,
                                    height: 100.0,
                                    angle: 0.0,
                                });
                            },
                        );
                    });
                },
            )
        };
        let _ = render(&mut canvas, vec![]);
        let point = egui::pos2(450.0, 300.0);
        let _ = render(
            &mut canvas,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let output = render(
            &mut canvas,
            vec![egui::Event::PointerMoved(point + egui::vec2(40.0, 30.0))],
        );
        let artwork = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) => Some(Rect::from_points(
                    &mesh
                        .vertices
                        .iter()
                        .map(|vertex| vertex.pos)
                        .collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .expect("fixture artwork must be painted during the pan");
        assert!(
            (artwork.min - overlay.min).length() < 0.01,
            "native artwork lagged the live overlay by {:?}",
            overlay.min - artwork.min
        );
        assert!((artwork.max - overlay.max).length() < 0.01);
    }
}
