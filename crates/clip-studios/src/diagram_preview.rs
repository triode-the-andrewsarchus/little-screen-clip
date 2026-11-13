//! Native diagram preview ownership shared by document and source editors.
use little_clip_core::{BackgroundJobs, diagram::Format, jobs::task::Task};
use std::time::{Duration, Instant};

struct Pixels {
    image: egui::ColorImage,
    warnings: Vec<String>,
    math: Option<little_clip_render::diagram::MathLayout>,
    drawing: Option<little_clip_render::diagram::DrawingLayout>,
}

/// The last canonical drawing image, with explicit source freshness.
pub(crate) struct DrawingFrame {
    pub texture: egui::TextureId,
    pub layout: little_clip_render::diagram::DrawingLayout,
    pub fresh: bool,
}

/// Owns debouncing, cancellation and the last valid image for one diagram source.
#[derive(Default)]
pub struct Preview {
    source: Option<(Format, String, Option<[u8; 3]>)>,
    pending: Option<Task<Result<Pixels, String>>>,
    due: Option<Instant>,
    texture: Option<egui::TextureHandle>,
    aspect: Option<f32>,
    math: Option<little_clip_render::diagram::MathLayout>,
    drawing: Option<little_clip_render::diagram::DrawingLayout>,
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
            drawing: Some(little_clip_render::diagram::DrawingLayout {
                origin: [0.0; 2],
                size: [100.0; 2],
            }),
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
                math: Some(layout),
                ..Default::default()
            },
            id,
        )
    }

    /// Cancels hidden reads and releases GPU pixels, retaining only layout geometry.
    pub fn suspend(&mut self) {
        let pending = self.pending.take().is_some();
        let texture = self.texture.take().is_some();
        if pending || texture {
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
                    self.math = pixels.math;
                    self.drawing = pixels.drawing;
                    self.prepared_at = Some(Instant::now());
                    self.aspect = Some(pixels.image.size[0] as f32 / pixels.image.size[1] as f32);
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
                        math: rendered.math,
                        drawing: rendered.drawing,
                    })
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
        self.math
    }

    pub(crate) fn paint_math(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        source: &str,
        id: egui::Id,
    ) -> egui::Response {
        if let Some(texture) = &self.texture {
            egui::Image::new(texture).paint_at(ui, rect);
        }
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
    pub(crate) fn drawing_frame(&mut self, ui: &egui::Ui, source: &str) -> Option<DrawingFrame> {
        self.prepare(ui, Format::Excalidraw, source, None);
        Some(DrawingFrame {
            texture: self.texture.as_ref()?.id(),
            layout: self.drawing?,
            fresh: !self.stale,
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
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let image = egui::Rect::from_center_size(rect.center(), size * scale);
            egui::Image::new(texture).paint_at(ui, image);
        }
        if format == Format::Mermaid && self.texture.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(60));
        }
        self.status_ui(ui, format);
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
    fn drawing_revisions_keep_old_pixels_but_disable_old_geometry_for_new_source() {
        let ctx = egui::Context::default();
        let mut preview = Preview::drawing_fixture(&ctx, "old source");
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                assert!(preview.drawing_frame(ui, "old source").unwrap().fresh);
                let old = preview.drawing_frame(ui, "new source").unwrap();
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
                math: None,
                drawing: None,
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
