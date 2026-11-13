//! Native diagram preview ownership shared by document and source editors.
use little_clip_core::{BackgroundJobs, diagram::Format, jobs::task::Task};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
mod camera;
pub(crate) use camera::Camera;

/// Delivery from a graphical source transaction to the session's undo owner.
pub(crate) enum GraphicalUpdate {
    Changed(String),
    Replaced,
    Failed,
}

#[derive(Default)]
struct Artwork {
    math: Option<little_clip_render::diagram::MathLayout>,
    drawing: Option<little_clip_render::diagram::DrawingLayout>,
    vector: Option<Arc<clip_ui::vectors::svg::Scene>>,
    geometry: Option<Arc<clip_ui::vectors::svg::Geometry>>,
}
struct Pixels {
    image: egui::ColorImage,
    warnings: Vec<String>,
    artwork: Artwork,
}

impl Pixels {
    // Both derived representations share one bounded preparation job. The UI
    // only receives ready pixels/geometry, never a decoder or source resolver.
    fn from_rendered(
        mut rendered: little_clip_render::diagram::Rendered,
        format: Format,
    ) -> Result<Self, String> {
        let mut geometry = None;
        let prepared = if matches!(
            format,
            Format::Excalidraw | Format::Wireviz | Format::Mermaid
        ) {
            little_clip_render::diagram::retained::prepare(&rendered.svg)
        } else {
            little_clip_render::diagram::retained::prepare_preview(&rendered.svg)
        };
        let vector = {
            match prepared.and_then(|prepared| {
                if matches!(format, Format::Wireviz | Format::Mermaid) {
                    geometry = Some(Arc::new(clip_ui::vectors::svg::Geometry::from_tree(
                        &prepared.tree,
                        if format == Format::Mermaid {
                            "graph-0-clip-mermaid-"
                        } else {
                            "graph-0-"
                        },
                    )));
                }
                clip_ui::vectors::svg::Scene::new(prepared.tree, prepared.images)
            }) {
                Ok(scene) => Some(scene),
                Err(error) => {
                    rendered.warnings.push(error);
                    None
                }
            }
        };
        let mut reader = image::ImageReader::with_format(
            std::io::Cursor::new(&rendered.png),
            image::ImageFormat::Png,
        );
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(1536);
        limits.max_image_height = Some(1024);
        limits.max_alloc = Some(8 * 1024 * 1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .map_err(|error| error.to_string())?
            .to_rgba8();
        let pixels = egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        );
        Ok(Pixels {
            image: pixels,
            warnings: rendered.warnings,
            artwork: Artwork {
                math: rendered.math,
                drawing: rendered.drawing,
                vector,
                geometry,
            },
        })
    }
}

/// The last canonical drawing image, with explicit source freshness.
pub(crate) struct NativeFrame {
    pub texture: egui::TextureId,
    pub layout: little_clip_render::diagram::DrawingLayout,
    pub fresh: bool,
    pub vector: Option<Arc<clip_ui::vectors::svg::Scene>>,
    pub geometry: Option<Arc<clip_ui::vectors::svg::Geometry>>,
}
impl NativeFrame {
    /// Paints ready vectors, retaining the bounded raster as an explicit fallback.
    pub(crate) fn paint(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        image: egui::Rect,
        changes: clip_ui::vectors::svg::Changes,
        tint: egui::Color32,
    ) -> Option<String> {
        let result = self
            .vector
            .as_ref()
            .map(|scene| scene.paint(ui, rect, image, changes));
        if matches!(result, Some(Ok(()))) {
            return None;
        }
        ui.painter_at(rect).image(
            self.texture,
            image,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            tint,
        );
        result.and_then(Result::err)
    }
}

/// Owns debouncing, cancellation and the last valid image for one diagram source.
#[derive(Default)]
pub struct Preview {
    source: Option<(Format, String, Option<[u8; 3]>)>,
    pending: Option<Task<Result<Pixels, String>>>,
    due: Option<Instant>,
    texture: Option<egui::TextureHandle>,
    aspect: Option<f32>,
    artwork: Artwork,
    prepared_at: Option<Instant>,
    error: Option<String>,
    warnings: Vec<String>,
    stale: bool,
}
impl Preview {
    #[cfg(test)]
    pub(crate) fn drawing_fixture(ctx: &egui::Context, source: &str) -> Self {
        Self {
            source: Some((Format::Excalidraw, source.into(), None)),
            texture: Some(ctx.load_texture(
                "drawing fixture",
                egui::ColorImage::new([2, 2], egui::Color32::WHITE),
                egui::TextureOptions::LINEAR,
            )),
            artwork: Artwork {
                drawing: Some(little_clip_render::diagram::DrawingLayout {
                    origin: [0.0; 2],
                    size: [100.0; 2],
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn equation_fixture(
        ctx: &egui::Context,
        layout: little_clip_render::diagram::MathLayout,
    ) -> (Self, egui::TextureId) {
        let texture = ctx.load_texture(
            "equation geometry fixture",
            egui::ColorImage::new([1, 1], egui::Color32::GREEN),
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        (
            Self {
                texture: Some(texture),
                artwork: Artwork {
                    math: Some(layout),
                    ..Default::default()
                },
                ..Default::default()
            },
            id,
        )
    }

    /// Cancels hidden reads and releases GPU pixels, retaining only layout geometry.
    pub fn suspend(&mut self) {
        let pending = self.pending.take().is_some();
        let texture = self.texture.take().is_some();
        let vector = self.artwork.vector.take().is_some();
        if pending || texture || vector {
            self.due = Some(Instant::now());
            self.stale = true;
        }
    }

    fn update_source(&mut self, format: Format, source: &str, foreground: Option<[u8; 3]>) {
        if self
            .source
            .as_ref()
            .is_some_and(|(old_format, old, old_color)| {
                *old_format == format && old == source && *old_color == foreground
            })
        {
            return;
        }
        self.pending = None;
        self.source = Some((format, source.to_owned(), foreground));
        self.due = Some(Instant::now() + Duration::from_millis(120));
        self.prepared_at = None;
        self.error = None;
        self.warnings.clear();
        self.stale = true;
    }

    fn poll(&mut self, ui: &egui::Ui) {
        if let Some(result) = self.pending.as_ref().and_then(Task::poll) {
            self.pending = None;
            ui.ctx().request_repaint();
            match result {
                Ok(Ok(pixels)) => {
                    self.warnings = pixels.warnings;
                    self.artwork = pixels.artwork;
                    self.prepared_at = Some(Instant::now());
                    let size = self.artwork.vector.as_ref().map_or_else(
                        || egui::vec2(pixels.image.size[0] as f32, pixels.image.size[1] as f32),
                        |scene| scene.size(),
                    );
                    self.aspect = Some(size.x / size.y);
                    self.texture = Some(ui.ctx().load_texture(
                        "native diagram",
                        pixels.image,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.error = None;
                    self.stale = false;
                }
                Ok(Err(error)) => self.error = Some(error),
                Err(little_clip_core::JobError::QueueFull) => {
                    self.due = Some(Instant::now() + Duration::from_millis(250));
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }

    fn prepare(
        &mut self,
        ui: &egui::Ui,
        format: Format,
        source: &str,
        foreground: Option<[u8; 3]>,
    ) {
        self.update_source(format, source, foreground);
        if format == Format::Mermaid
            && self.pending.is_none()
            && self.due.is_none()
            && self
                .prepared_at
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
        {
            self.due = Some(Instant::now());
        }
        self.poll(ui);
        if self.due.is_some_and(|due| Instant::now() >= due) && self.pending.is_none() {
            self.due = None;
            // Count failed refreshes too, so an expired image cannot cause a
            // render attempt every frame after a transient worker failure.
            self.prepared_at = Some(Instant::now());
            self.stale = true;
            let source = source.to_owned();
            self.pending = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(10),
                move |cancelled| {
                    let rendered = if let Some(color) = foreground {
                        little_clip_render::diagram::render_math(
                            &source,
                            format == Format::Math,
                            color,
                            cancelled,
                        )?
                    } else {
                        little_clip_render::diagram::render(format, &source, cancelled)?
                    };
                    Pixels::from_rendered(rendered, format)
                },
            ));
        }
        if self.pending.is_some() || self.due.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }

    pub(crate) fn prepare_math(
        &mut self,
        ui: &egui::Ui,
        source: &str,
        display: bool,
        color: egui::Color32,
    ) {
        self.prepare(
            ui,
            if display {
                Format::Math
            } else {
                Format::MathInline
            },
            source,
            Some([color.r(), color.g(), color.b()]),
        );
    }

    pub(crate) fn math_layout(&self) -> Option<little_clip_render::diagram::MathLayout> {
        self.artwork.math
    }

    pub(crate) fn paint_math(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        source: &str,
        id: egui::Id,
    ) -> egui::Response {
        self.paint_artwork(ui, rect, rect);
        ui.interact(rect, id, egui::Sense::click())
            .on_hover_ui(|ui| {
                ui.monospace(source.chars().take(2048).collect::<String>());
                if source.chars().nth(2048).is_some() {
                    ui.small(
                        "Source shortened; use the document source view for the complete equation.",
                    );
                }
                if let Some(error) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if self.stale && self.texture.is_some() {
                    ui.small("Preview shows the last valid equation.");
                }
            })
    }

    pub(crate) fn math_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Prepares the same native image used by passive previews. Canvas hit tests
    /// must use `fresh`; an old texture never authorizes editing newer source.
    pub(crate) fn native_frame(
        &mut self,
        ui: &egui::Ui,
        format: Format,
        source: &str,
    ) -> Option<NativeFrame> {
        self.prepare(ui, format, source, None);
        let texture = self.texture.as_ref()?;
        let layout = match format {
            Format::Excalidraw => self.artwork.drawing?,
            Format::Wireviz | Format::Mermaid => {
                let size = self
                    .artwork
                    .geometry
                    .as_ref()
                    .map_or_else(|| texture.size_vec2(), |g| g.size);
                little_clip_render::diagram::DrawingLayout {
                    origin: [0.0; 2],
                    size: [f64::from(size.x), f64::from(size.y)],
                }
            }
            _ => return None,
        };
        Some(NativeFrame {
            texture: texture.id(),
            layout,
            fresh: !self.stale,
            vector: self.artwork.vector.clone(),
            geometry: self.artwork.geometry.clone(),
        })
    }

    /// Shows a live preview without parsing, decoding or waiting on the UI thread.
    pub fn show(&mut self, ui: &mut egui::Ui, format: Format, source: &str) {
        let width = ui.available_width().clamp(1.0, 1536.0);
        let height = self
            .aspect
            .map_or(140.0, |aspect| (width / aspect).min(520.0));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            self.suspend();
            return;
        }
        self.prepare(ui, format, source, None);
        ui.painter().rect_filled(rect, 4.0, egui::Color32::WHITE);
        if let Some(texture) = &self.texture {
            let size = self
                .artwork
                .vector
                .as_ref()
                .map_or_else(|| texture.size_vec2(), |scene| scene.size());
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let image = egui::Rect::from_center_size(rect.center(), size * scale);
            self.paint_artwork(ui, rect, image);
        }
        if format == Format::Mermaid && self.texture.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(60));
        }
        self.status_ui(ui, format);
    }

    fn paint_artwork(&self, ui: &egui::Ui, rect: egui::Rect, image: egui::Rect) {
        if let Some(scene) = &self.artwork.vector
            && scene.paint(ui, rect, image, Default::default()).is_ok()
        {
            return;
        }
        if let Some(texture) = &self.texture {
            egui::Image::new(texture).paint_at(ui, image);
        }
    }

    pub(crate) fn status_ui(&self, ui: &mut egui::Ui, format: Format) {
        if self.pending.is_some() || self.due.is_some() {
            ui.label(format!("Rendering {}…", format.name()));
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            if self.stale && self.texture.is_some() {
                ui.small("Preview shows the last valid source.");
            }
        }
        for warning in &self.warnings {
            ui.colored_label(ui.visuals().warn_fg_color, warning);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passive_formats_prepare_vectors_at_logical_size_independent_of_thumbnail_pixels() {
        for format in [
            Format::Pikchr,
            Format::Svgbob,
            Format::Wavedrom,
            Format::Dbml,
            Format::Math,
        ] {
            let mut png = std::io::Cursor::new(Vec::new());
            image::RgbaImage::new(10, 8)
                .write_to(&mut png, image::ImageFormat::Png)
                .unwrap();
            let pixels = Pixels::from_rendered(little_clip_render::diagram::Rendered {
                svg: r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="800"><path d="M1 2C4 8 10 3 40 50" fill="none" stroke="black"/></svg>"#.into(),
                png: png.into_inner(), width: 10, height: 8,
                warnings: Vec::new(), math: None, drawing: None,
            }, format).unwrap();
            assert_eq!(
                pixels.artwork.vector.unwrap().size(),
                egui::vec2(1000.0, 800.0)
            );
            assert_eq!(
                pixels.image.size,
                [10, 8],
                "fallback pixels must not set vector resolution"
            );
            assert!(pixels.warnings.is_empty());
        }
    }

    #[test]
    fn raster_effect_fallback_retains_mermaid_semantic_hit_geometry() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(100, 80)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let rendered = little_clip_render::diagram::Rendered {
            svg: r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80"><defs><mask id="m"><rect width="100" height="80" fill="white"/></mask></defs><g id="graph-0-clip-mermaid-node-61-part-0" transform="translate(10 20)"><g mask="url(#m)"><path d="M0 0h10v8h-10Z"/></g></g></svg>"##.into(),
            png: png.into_inner(),
            width: 100,
            height: 80,
            warnings: Vec::new(),
            math: None,
            drawing: None,
        };
        let pixels = Pixels::from_rendered(rendered, Format::Mermaid).unwrap();
        assert!(pixels.artwork.vector.is_none());
        assert!(
            pixels
                .warnings
                .iter()
                .any(|warning| warning.contains("raster preview"))
        );
        let geometry = pixels.artwork.geometry.unwrap();
        assert_eq!(geometry.size, egui::vec2(100.0, 80.0));
        assert_eq!(
            geometry.groups["graph-0-clip-mermaid-node-61-part-0"],
            egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(20.0, 28.0))
        );
    }

    #[test]
    fn drawing_revisions_keep_old_pixels_but_disable_old_geometry_for_new_source() {
        let ctx = egui::Context::default();
        let mut preview = Preview::drawing_fixture(&ctx, "old source");
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(
                    preview
                        .native_frame(ui, Format::Excalidraw, "old source")
                        .unwrap()
                        .fresh
                );
                let old = preview
                    .native_frame(ui, Format::Excalidraw, "new source")
                    .unwrap();
                assert!(!old.fresh);
                assert_eq!(old.layout.size, [100.0; 2]);
            });
        });
        preview.suspend();
    }
    #[test]
    fn source_revisions_cancel_pending_work_and_keep_the_new_source() {
        let jobs = BackgroundJobs::new(1, 2);
        let task = Task::start(&jobs, Duration::from_secs(2), |_| {
            Ok(Pixels {
                image: egui::ColorImage::example(),
                warnings: Vec::new(),
                artwork: Artwork::default(),
            })
        });
        let token = task.token.clone();
        let mut preview = Preview {
            pending: Some(task),
            ..Default::default()
        };
        preview.update_source(Format::Pikchr, "box", None);
        assert!(token.load(std::sync::atomic::Ordering::Relaxed));
        assert!(preview.pending.is_none());
        assert_eq!(preview.source, Some((Format::Pikchr, "box".into(), None)));
        let due = preview.due;
        preview.update_source(Format::Pikchr, "box", None);
        assert_eq!(
            preview.due, due,
            "unchanged source must not restart debounce"
        );
    }
    #[test]
    fn math_theme_changes_cancel_the_previous_color_request() {
        let jobs = BackgroundJobs::new(1, 2);
        let mut preview = Preview::default();
        preview.update_source(Format::MathInline, "x", Some([17; 3]));
        let task = Task::start(&jobs, Duration::from_secs(2), |_| Err("unused".into()));
        let token = task.token.clone();
        preview.pending = Some(task);
        preview.update_source(Format::MathInline, "x", Some([240; 3]));
        assert!(token.load(std::sync::atomic::Ordering::Relaxed));
        assert!(preview.pending.is_none());
        assert_eq!(
            preview.source,
            Some((Format::MathInline, "x".into(), Some([240; 3])))
        );
    }
}
