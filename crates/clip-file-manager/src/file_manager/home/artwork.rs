//! Bounded, visible-only preparation of borderless Home artwork references.
use super::Pin;
use little_clip_core::{BackgroundJobs, JobError, file_source::ReadCheck, jobs::task::Task};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::Duration,
};
use uuid::Uuid;

pub(super) static READS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 2));
const CAPACITY: usize = 128;
const MEMORY: usize = 128 * 1024 * 1024;
const SVG_BYTES: usize = 2 * 1024 * 1024;

pub(in crate::file_manager) fn supported(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("svg") || extension.eq_ignore_ascii_case("note")
    })
}

enum Prepared {
    Vector(Arc<clip_ui::vectors::svg::Scene>, Vec<String>, usize),
    Bitmap {
        pixels: egui::ColorImage,
        neutral: bool,
    },
}
impl Prepared {
    fn bytes(&self) -> usize {
        match self {
            Self::Vector(_, _, bytes) => *bytes,
            // Account for both the retained pixels and their uploaded texture.
            Self::Bitmap { pixels, .. } => pixels.pixels.len() * 8,
        }
    }
}

fn prepare(path: &Path, cancel: &AtomicBool) -> Result<Prepared, String> {
    let (mut file, source) = ReadCheck::default().open(path, None, cancel)?;
    let prepared = if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        let mut bytes = Vec::new();
        file.by_ref()
            .take(SVG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > SVG_BYTES {
            return Err("Home SVG exceeds 2 MiB".into());
        }
        let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
        let rendered = little_clip_render::diagram::render_svg_artwork(text, cancel)?;
        let svg = little_clip_render::diagram::retained::prepare_preview(&rendered.svg)?;
        // XML overestimates ordinary retained path storage; embedded pixel
        // allocations are charged separately, including their GPU texture.
        let bytes = rendered.svg.len().saturating_mul(8)
            + svg
                .images
                .values()
                .map(|image| image.as_raw().len() * 2)
                .sum::<usize>();
        Prepared::Vector(
            clip_ui::vectors::svg::Scene::new(svg.tree, svg.images)?,
            rendered.warnings,
            bytes,
        )
    } else {
        // Supernote stores bitmap ink. Keep that limitation explicit rather
        // than tracing it and claiming invented vector detail. Other notebook
        // formats need their actual vector exporter before admission here.
        let (png, _, _) = little_clip_core::notebook::render_ink_page(path, 1, 4096, cancel)?;
        let pixels = image::load_from_memory(&png)
            .map_err(|error| error.to_string())?
            .to_rgba8();
        prepare_bitmap(pixels)
    };
    source.check_file(&file)?;
    source.check()?;
    Ok(prepared)
}

fn prepare_bitmap(mut pixels: image::RgbaImage) -> Prepared {
    let neutral = pixels
        .pixels()
        .all(|pixel| pixel[3] == 0 || (pixel[0] == pixel[1] && pixel[1] == pixel[2]));
    if neutral {
        // Supernote's grayscale is black pigment on white paper. Retain that
        // coverage as alpha, so one texture works on either theme without
        // reparsing or uploading a recolored bitmap on each theme switch.
        for pixel in pixels.pixels_mut() {
            let alpha = u16::from(pixel[3]) * (255 - u16::from(pixel[0])) / 255;
            pixel.0 = [255, 255, 255, alpha as u8];
        }
    }
    Prepared::Bitmap {
        pixels: egui::ColorImage::from_rgba_unmultiplied(
            [pixels.width() as usize, pixels.height() as usize],
            pixels.as_raw(),
        ),
        neutral,
    }
}

#[derive(Default)]
struct View {
    path: PathBuf,
    pending: Option<Task<Result<Prepared, String>>>,
    prepared: Option<Prepared>,
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    seen: u64,
}

#[derive(Default)]
pub(in crate::file_manager) struct Assets {
    entries: BTreeMap<Uuid, View>,
    generation: u64,
}
impl Assets {
    pub fn begin(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn end(&mut self) {
        self.entries.retain(|_, view| view.seen == self.generation);
    }
    pub fn retire(&mut self) {
        self.entries.clear();
    }
    pub fn reload(&mut self, pin: Uuid) {
        self.entries.remove(&pin);
    }

    fn request(&mut self, pin: &Pin) -> Option<&mut View> {
        if !self.entries.contains_key(&pin.id) && self.entries.len() >= CAPACITY {
            let old = self
                .entries
                .iter()
                .filter(|(_, view)| view.seen != self.generation)
                .min_by_key(|(_, view)| view.seen)
                .map(|(id, _)| *id)?;
            self.entries.remove(&old);
        }
        let reads = self
            .entries
            .values()
            .filter(|view| view.pending.is_some())
            .count();
        let retained = self
            .entries
            .iter()
            .filter(|(id, _)| **id != pin.id)
            .filter_map(|(_, view)| view.prepared.as_ref())
            .map(Prepared::bytes)
            .sum::<usize>();
        let view = self.entries.entry(pin.id).or_default();
        if view.path != pin.path {
            *view = View {
                path: pin.path.clone(),
                ..Default::default()
            };
        }
        view.seen = self.generation;
        if let Some(result) = view.pending.as_ref().and_then(Task::poll) {
            view.pending = None;
            match result {
                Ok(Ok(prepared)) => {
                    if retained.saturating_add(prepared.bytes()) > MEMORY {
                        view.error = Some(
                            "Artwork memory budget reached; zoom in and choose Reload artwork"
                                .into(),
                        );
                    } else {
                        view.prepared = Some(prepared);
                    }
                }
                Err(JobError::QueueFull) => {}
                Ok(Err(error)) => view.error = Some(error),
                Err(error) => view.error = Some(error.to_string()),
            }
        }
        if view.prepared.is_none() && view.error.is_none() && view.pending.is_none() && reads < 2 {
            let path = pin.path.clone();
            view.pending = Some(Task::start(
                &READS,
                Duration::from_secs(20),
                move |cancel| prepare(&path, cancel),
            ));
        }
        Some(view)
    }

    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        pin: &Pin,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let Some(view) = self.request(pin) else {
            ui.painter().text(
                rect.left_top(),
                egui::Align2::LEFT_TOP,
                "Zoom in to load artwork",
                egui::FontId::proportional(12.0),
                ui.visuals().weak_text_color(),
            );
            return;
        };
        if view.pending.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        let result = match &view.prepared {
            Some(Prepared::Vector(scene, warnings, _)) => {
                let size = scene.size();
                let fit = (rect.width() / size.x).min(rect.height() / size.y);
                let image = egui::Rect::from_center_size(rect.center(), size * fit);
                let result = scene.paint(
                    ui,
                    rect.intersect(ui.clip_rect()),
                    image,
                    clip_ui::vectors::svg::Changes {
                        invert_neutral_ink: scene.needs_lightness_inversion(palette.is_dark),
                        ..Default::default()
                    },
                );
                if !warnings.is_empty() {
                    ui.interact(
                        rect,
                        egui::Id::new(("home_art_warnings", pin.id)),
                        egui::Sense::hover(),
                    )
                    .on_hover_text(warnings.join("\n"));
                }
                result
            }
            Some(Prepared::Bitmap { pixels, neutral }) => {
                let texture = view.texture.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        "Home notebook ink",
                        pixels.clone(),
                        egui::TextureOptions::LINEAR,
                    )
                });
                let size = texture.size_vec2();
                let fit = (rect.width() / size.x).min(rect.height() / size.y);
                ui.painter().image(
                    texture.id(),
                    egui::Rect::from_center_size(rect.center(), size * fit),
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    if *neutral {
                        clip_ui::ui_appearance::WorkbenchColors::from_palette(palette).text_primary
                    } else {
                        egui::Color32::WHITE
                    },
                );
                ui.interact(
                    rect,
                    egui::Id::new(("home_bitmap_notice", pin.id)),
                    egui::Sense::hover(),
                )
                .on_hover_text("Supernote bitmap ink · first page");
                Ok(())
            }
            None => Ok(()),
        };
        if let Err(error) = result {
            view.error = Some(error);
        }
        if let Some(error) = &view.error {
            ui.painter().text(
                rect.left_top(),
                egui::Align2::LEFT_TOP,
                "Artwork unavailable",
                egui::FontId::proportional(14.0),
                ui.visuals().error_fg_color,
            );
            ui.interact(
                rect,
                egui::Id::new(("home_art_error", pin.id)),
                egui::Sense::hover(),
            )
            .on_hover_text(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grayscale_ink_mask_reproduces_original_tones_on_white_and_keeps_empty_pixels_clear() {
        let image =
            image::RgbaImage::from_raw(3, 1, vec![0, 0, 0, 255, 128, 128, 128, 255, 0, 0, 0, 0])
                .unwrap();
        let Prepared::Bitmap { pixels, neutral } = prepare_bitmap(image) else {
            panic!()
        };
        assert!(neutral);
        let on_white: Vec<_> = pixels.pixels.iter().map(|pixel| 255 - pixel.a()).collect();
        assert_eq!(on_white, [0, 128, 255]);
        assert_eq!(pixels.pixels[2], egui::Color32::TRANSPARENT);
    }
    #[test]
    fn colored_ink_keeps_its_authored_pixels() {
        let bytes = [255, 32, 64, 200];
        let image = image::RgbaImage::from_raw(1, 1, bytes.to_vec()).unwrap();
        let expected = egui::ColorImage::from_rgba_unmultiplied([1, 1], &bytes);
        let Prepared::Bitmap { pixels, neutral } = prepare_bitmap(image) else {
            panic!()
        };
        assert!(!neutral);
        assert_eq!(pixels, expected);
    }
}
