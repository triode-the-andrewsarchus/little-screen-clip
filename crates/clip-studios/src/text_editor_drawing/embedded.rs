//! Embeds native drawing tools alongside host-owned live content.
//!
//! The host owns source history and durable writes. Supply returned source edits
//! on the next frame; overlays use the exact camera used to paint the artwork.
use super::*;

/// Controls interaction and reserves world-space areas for live widgets.
#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Enables drawing tools and properties rather than shortcut browsing.
    pub editable: bool,
    /// Prevents drawing or shortcut activation through these widget bounds.
    pub excluded: &'a [Bounds],
    /// Includes live content in Fit without creating artificial drawing shapes.
    pub extent: Option<Bounds>,
}
impl Options<'_> {
    /// Returns ordinary drawing-editor interaction with no live overlays.
    pub fn editing() -> Self {
        Self {
            editable: true,
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
        let current = changed.as_deref().unwrap_or(source);
        self.editor.prepare_model(current);
        if options.editable {
            self.editor.toolbar(ui, palette);
        }
        let size = ui.available_size().max(Vec2::splat(1.0));
        let (_, activated) = ui
            .horizontal_top(|ui| {
                if options.editable {
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
                                    self.editor.text_controls(ui);
                                    self.editor.properties(ui, palette);
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
        let mut frame = |canvas: &mut Canvas, events, editable, excluded: &[Bounds]| {
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
                                editable,
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
        let (_, point) = frame(&mut canvas, vec![], false, &[]);
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
        let _ = frame(&mut canvas, pointer(true), false, &[]);
        let (clicked, _) = frame(&mut canvas, pointer(false), false, &[]);
        assert_eq!(clicked.activated.as_deref(), Some("shortcut"));
        assert!(!canvas.busy());
        let block = Bounds {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            angle: 0.0,
        };
        let _ = frame(&mut canvas, pointer(true), false, &[block]);
        let (blocked, _) = frame(&mut canvas, pointer(false), false, &[block]);
        assert!(blocked.activated.is_none());
        assert!(canvas.editor.gesture.is_none());
        assert_eq!(canvas.editor.model.as_ref().unwrap().source(), source);
    }
}
