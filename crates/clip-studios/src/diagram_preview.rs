//! Native diagram preview ownership shared by document and source editors.
use little_clip_core::{BackgroundJobs, diagram::Format, jobs::task::Task};
use std::time::{Duration, Instant};

struct Pixels {
    image: egui::ColorImage,
    warnings: Vec<String>,
}

/// Owns debouncing, cancellation and the last valid image for one diagram source.
#[derive(Default)]
pub struct Preview {
    source: Option<(Format, String)>,
    pending: Option<Task<Result<Pixels, String>>>,
    due: Option<Instant>,
    texture: Option<egui::TextureHandle>,
    aspect: Option<f32>,
    prepared_at: Option<Instant>,
    error: Option<String>,
    warnings: Vec<String>,
    stale: bool,
}
impl Preview {
    /// Cancels hidden reads and releases GPU pixels, retaining only layout geometry.
    pub fn suspend(&mut self) {
        let pending = self.pending.take().is_some();
        let texture = self.texture.take().is_some();
        if pending || texture {
            self.due = Some(Instant::now());
            self.stale = true;
        }
    }

    fn update_source(&mut self, format: Format, source: &str) {
        if self
            .source
            .as_ref()
            .is_some_and(|(old_format, old)| *old_format == format && old == source)
        {
            return;
        }
        self.pending = None;
        self.source = Some((format, source.to_owned()));
        self.due = Some(Instant::now() + Duration::from_millis(120));
        self.prepared_at = None;
        self.error = None;
        self.warnings.clear();
        self.stale = true;
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
        self.update_source(format, source);
        if format == Format::Mermaid
            && self.pending.is_none()
            && self.due.is_none()
            && self
                .prepared_at
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
        {
            self.due = Some(Instant::now());
        }
        if let Some(result) = self.pending.as_ref().and_then(Task::poll) {
            self.pending = None;
            match result {
                Ok(Ok(pixels)) => {
                    self.warnings = pixels.warnings;
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
                    let rendered = little_clip_render::diagram::render(format, &source, cancelled)?;
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
                    })
                },
            ));
        }
        ui.painter().rect_filled(rect, 4.0, egui::Color32::WHITE);
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let image = egui::Rect::from_center_size(rect.center(), size * scale);
            ui.painter().image(
                texture.id(),
                image,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        if format == Format::Mermaid && self.texture.is_some() {
            ui.ctx().request_repaint_after(Duration::from_secs(60));
        }
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
    fn source_revisions_cancel_pending_work_and_keep_the_new_source() {
        let jobs = BackgroundJobs::new(1, 2);
        let task = Task::start(&jobs, Duration::from_secs(2), |_| {
            Ok(Pixels {
                image: egui::ColorImage::example(),
                warnings: Vec::new(),
            })
        });
        let token = task.token.clone();
        let mut preview = Preview {
            pending: Some(task),
            ..Default::default()
        };
        preview.update_source(Format::Pikchr, "box");
        assert!(token.load(std::sync::atomic::Ordering::Relaxed));
        assert!(preview.pending.is_none());
        assert_eq!(preview.source, Some((Format::Pikchr, "box".into())));
        let due = preview.due;
        preview.update_source(Format::Pikchr, "box");
        assert_eq!(
            preview.due, due,
            "unchanged source must not restart debounce"
        );
    }
}
