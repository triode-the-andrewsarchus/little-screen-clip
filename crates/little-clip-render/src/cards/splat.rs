use super::*;
use crate::scene_preview::{self, Preview};
use std::time::{Duration, SystemTime};

impl ScreenRenderer {
    /// Routes scene files consistently for URI cards and text-path cards.
    pub(super) fn draw_scene_file_card(
        &self,
        pixmap: &mut Pixmap,
        path: &Path,
        bounds: [f32; 4],
        is_hero: bool,
    ) -> bool {
        if !path.is_file() {
            return false;
        }
        let [rx, ry, rw, rh] = bounds;
        if little_clip_scene::is_splat_file(path) {
            self.draw_splat_card(pixmap, path, bounds);
            return true;
        }
        if model3d::is_3d_model_file(path)
            && let Ok(model) = model3d::Model3d::load_cached(path)
        {
            self.draw_model3d_card(pixmap, &model, rx, ry, rw, rh, is_hero);
            return true;
        }
        false
    }

    pub(super) fn draw_splat_card(&self, pixmap: &mut Pixmap, path: &Path, bounds: [f32; 4]) {
        let [rx, ry, rw, rh] = bounds;
        let x = rx + 14.0;
        let right = rx + rw - 10.0;
        let badge_bg = self.color_from_hex(&self.palette.diff_mod);
        let badge_fg = self.contrast_color(
            badge_bg,
            Some(self.color_from_hex(&self.palette.colors.text_primary)),
        );
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("3D")
            .to_ascii_uppercase();
        self.draw_pill_badge(
            pixmap,
            &format!("3D {extension} SCENE"),
            x,
            ry + 124.0,
            14.0,
            badge_fg,
            badge_bg,
            right,
        );
        self.draw_text_clipped(
            pixmap,
            &path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default(),
            x,
            ry + 152.0,
            13.0,
            self.color_from_hex(&self.palette.colors.text_secondary),
            false,
            right,
        );
        let y = ry + 174.0;
        let width = (right - x).round().max(1.0) as u32;
        let height = (ry + rh - 30.0 - y).round().max(1.0) as u32;
        self.draw_card_viewport_frame(pixmap, x, y, width as f32, height as f32);
        let preview = scene_preview::request(path, [width, height], self);
        match preview {
            Preview::Ready(orbit) => {
                let elapsed = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO);
                let frame = orbit.frame_at(elapsed);
                if let Some(mut raster) = Pixmap::new(frame.width(), frame.height()) {
                    for (pixel, rgba) in raster.pixels_mut().iter_mut().zip(frame.pixels()) {
                        *pixel =
                            ColorU8::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]).premultiply();
                    }
                    pixmap.draw_pixmap(
                        x.round() as i32,
                        y.round() as i32,
                        raster.as_ref(),
                        &PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                }
            }
            Preview::Loading => {
                self.draw_text_clipped(
                    pixmap,
                    "Preparing orbit preview…",
                    x + 8.0,
                    y + 16.0,
                    13.0,
                    self.color_from_hex(&self.palette.colors.text_secondary),
                    false,
                    right - 8.0,
                );
            }
            Preview::Error(error) => {
                self.draw_text_clipped(
                    pixmap,
                    &format!("Preview unavailable: {error}"),
                    x + 8.0,
                    y + 16.0,
                    13.0,
                    self.color_from_hex(&self.palette.colors.text_secondary),
                    false,
                    right - 8.0,
                );
            }
        }
    }
}
