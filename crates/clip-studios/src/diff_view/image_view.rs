use super::*;
use little_clip_core::palette::AppPalette;
use little_clip_ops::image_diff::downsample_rgba;

pub(super) struct RasterPreview {
    pub dimensions: [(u32, u32); 2],
    pub images: Option<[egui::ColorImage; 2]>,
}

impl RasterPreview {
    pub fn new(pair: [image::RgbaImage; 2]) -> Self {
        let dimensions = pair.each_ref().map(image::RgbaImage::dimensions);
        let images = pair.map(|image| {
            let (width, height, pixels) =
                downsample_rgba(image.width(), image.height(), image.as_raw(), 2048);
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &pixels)
        });
        Self {
            dimensions,
            images: Some(images),
        }
    }

    pub fn canvas_size(&self) -> (u32, u32) {
        self.dimensions[0].max(self.dimensions[1])
    }
}

pub(super) fn overlay_image(result: &ImageDiffResult) -> egui::ColorImage {
    let sample = |rgba: &[u8]| downsample_rgba(result.width, result.height, rgba, 2048);
    let (width, height, base) = sample(&result.base_rgba);
    let (_, _, revised) = sample(&result.comp_rgba);
    let (_, _, mask) = sample(&result.mask_rgba);
    let pixels = base
        .as_chunks::<4>()
        .0
        .iter()
        .zip(revised.as_chunks::<4>().0)
        .zip(mask.as_chunks::<4>().0)
        .map(|((base, revised), marker)| overlay_pixel(*base, *revised, *marker))
        .collect();
    egui::ColorImage::new([width as usize, height as usize], pixels)
}

fn overlay_pixel(base: [u8; 4], revised: [u8; 4], marker: [u8; 4]) -> Color32 {
    // Stack the two images' ink in red and green against a dim neutral image.
    let ink = |[r, g, b, a]: [u8; 4]| {
        let light = (77 * u32::from(r) + 150 * u32::from(g) + 29 * u32::from(b)) / 256;
        ((255 - light) * u32::from(a) / 255) as u8
    };
    let (a, b) = (ink(base), ink(revised));
    let gray = (255 - u16::from(a).midpoint(u16::from(b)) as u8) / 2;
    if marker == [0, 0, 0, 255] {
        Color32::from_gray(gray)
    } else if a.abs_diff(b) < 8 {
        // A color-only change can have equal luminance in both operands.
        Color32::from_rgb(200, 200, gray)
    } else {
        let red = if a > b {
            160_u8.saturating_add(a.abs_diff(b))
        } else {
            gray
        };
        let green = if b > a {
            160_u8.saturating_add(a.abs_diff(b))
        } else {
            gray
        };
        Color32::from_rgb(red, green, gray)
    }
}

impl DiffViewerState {
    pub(super) fn ensure_raster_overlay(&mut self) {
        if self.image_mode != ImageDiffViewMode::Overlay {
            self.overlay_error = None;
            return;
        }
        if self.raster_preview.is_none()
            || self.image_result.is_some()
            || self.overlay_error.is_some()
            || self.is_computing()
        {
            return;
        }
        let lhs = self.lhs.clone();
        let rhs = self.rhs.clone();
        let options = self.image_options.clone();
        self.compute_start_time = std::time::Instant::now();
        self.compute_job = Some(Task::start_unbounded(BackgroundJobs::global(), move |_| {
            let result = little_clip_ops::image_diff::compare_aligned_image_bytes(
                lhs.image_bytes().ok_or("Missing base image")?,
                lhs.image_mime(),
                rhs.image_bytes().ok_or("Missing comparison image")?,
                rhs.image_mime(),
                &options,
            )
            .map_err(|error| error.to_string())?
            .0;
            let overlay = overlay_image(&result);
            Ok(DiffComputation::Overlay(result, overlay))
        }));
    }

    pub(super) fn render_image_tolerance(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.label("Tolerance");
        let slider = egui::Slider::new(&mut self.image_options.threshold, 0.0..=0.5).step_by(0.01);
        let changed = ui
            .add(slider)
            .on_hover_text("Allowable perceptual difference")
            .changed();
        let mut preset = None;
        for value in [0.05_f32, 0.10, 0.20] {
            let is_current = (self.image_options.threshold - value).abs() < 0.005;
            if ui
                .add(crate::mode_tab_button(
                    &format!("{value:.2}"),
                    is_current,
                    palette,
                ))
                .clicked()
                && !is_current
            {
                preset = Some(value);
            }
        }
        if let Some(value) = preset {
            self.image_options.threshold = value;
        }
        if changed || preset.is_some() {
            if self.raster_preview.is_some() {
                self.compute_job = None;
            }
            self.image_result = None;
            self.raster_overlay = None;
            self.overlay_error = None;
            self.graphical.clear_overlay();
        }
    }

    pub(super) fn copy_image_diff_to_clipboard(&mut self) {
        if self.copy_job.is_some() {
            return;
        }
        let Some(result) = self.image_result.clone() else {
            return;
        };
        self.copy_job = Some(crate::desktop_ipc::PendingRequest::start(
            crate::desktop_ipc::mutation_jobs(),
            move || {
                let request =
                    image_mask_push_request(&result).map_err(|error| error.to_string())?;
                crate::send_ipc_request(&request).map_err(|error| error.to_string())
            },
        ));
    }

    /// Returns whether a diff mask copy is awaiting completion.
    pub fn copy_pending(&self) -> bool {
        self.copy_job.is_some()
    }

    pub(super) fn poll_copy_job(&mut self, now: f64) {
        let Some(result) = self.copy_job.as_ref().and_then(|job| job.try_result()) else {
            return;
        };
        self.copy_job = None;
        if let Err(error) = result.and_then(crate::desktop_ipc::action_result) {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    Icon::Alert,
                    format!("Could not copy diff mask: {error}"),
                ),
                now,
            ));
        }
    }
}
