//! Dedicated native-resolution hover preview cache for gallery and similarity grid tiles.
use super::super::shelf::preview::{Previews, Source};
use std::path::{Path, PathBuf};
use std::time::Duration;

const UNHOVER_GRACE_PERIOD: Duration = Duration::from_millis(250);
/// Minimum target edge length in pixels for hover previews.
pub const MIN_HOVER_EDGE: u32 = 128;
/// Maximum target edge length in pixels for hover previews.
pub const MAX_HOVER_EDGE: u32 = 4096;
/// Resolution step increment for hover preview caching.
pub const HOVER_EDGE_STEP: u32 = 128;

#[derive(Default)]
pub(in crate::file_manager) struct GalleryHover {
    cache: Previews,
    key: Option<(PathBuf, u32, uuid::Uuid, u64, u8)>,
    unhover_time: Option<std::time::Instant>,
}

impl GalleryHover {
    /// Polls in-flight hover preview loads and uploads completed textures.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
    }

    /// Clears all cached textures and active requests.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.key = None;
        self.unhover_time = None;
    }

    /// Retains the cached preview if `hovered_path` matches the active key, otherwise schedules eviction.
    pub fn retain(&mut self, hovered_path: Option<&Path>) {
        if let Some(path) = hovered_path {
            self.unhover_time = None;
            if self.key.as_ref().is_some_and(|k| k.0.as_path() != path) {
                self.clear();
            }
        } else if self.key.is_some() {
            let now = std::time::Instant::now();
            if let Some(unhover_time) = self.unhover_time {
                if now.duration_since(unhover_time) > UNHOVER_GRACE_PERIOD {
                    self.clear();
                }
            } else {
                self.unhover_time = Some(now);
            }
        }
    }

    /// Returns a cached texture handle for the given image path at or above `edge` resolution.
    pub fn texture_for(
        &mut self,
        path: &Path,
        revision: u64,
        edge: u32,
        orientation: u8,
        fallback: Option<(egui::TextureId, egui::Vec2)>,
    ) -> Option<(egui::TextureId, egui::Vec2)> {
        self.unhover_time = None;
        let edge = edge
            .clamp(MIN_HOVER_EDGE, MAX_HOVER_EDGE)
            .div_ceil(HOVER_EDGE_STEP)
            * HOVER_EDGE_STEP;
        if !self.key.as_ref().is_some_and(|key| {
            key.0 == path && key.1 == edge && key.3 == revision && key.4 == orientation
        }) {
            self.clear();
            self.key = Some((
                path.to_path_buf(),
                edge,
                uuid::Uuid::new_v4(),
                revision,
                orientation,
            ));
        }
        let Some((_, _, id, _, _)) = self.key else {
            return fallback;
        };
        if self.cache.needs(id) {
            self.cache.request(
                id,
                Source::Gallery {
                    path: path.to_path_buf(),
                    revision,
                    edge,
                    orientation,
                },
            );
        }
        self.cache
            .get(id)
            .and_then(|ready| ready.texture.as_ref())
            .map(|t| (t.id(), t.size_vec2()))
            .or(fallback)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_hovered_photo_retires_the_old_texture_identity() {
        let mut hover = GalleryHover::default();
        let path = Path::new("/photos/replaced.png");
        hover.texture_for(path, 1, 128, 1, None);
        let before = hover.key.as_ref().unwrap().2;
        hover.texture_for(path, 2, 128, 1, None);
        assert_ne!(hover.key.as_ref().unwrap().2, before);
        let before = hover.key.as_ref().unwrap().2;
        hover.texture_for(path, 2, 128, 6, None);
        assert_ne!(hover.key.as_ref().unwrap().2, before);
    }

    #[test]
    fn gallery_hover_requests_native_edge_and_evicts_on_retain() {
        let mut hover = GalleryHover::default();
        let path_a = Path::new("/photos/a.png");
        let path_b = Path::new("/photos/b.png");

        // Request native edge 2048 for photo A with fallback
        let fallback = Some((egui::TextureId::Managed(42), egui::vec2(128.0, 128.0)));
        let result = hover.texture_for(path_a, 1, 2048, 1, fallback);
        assert_eq!(result, fallback);
        assert!(hover.key.is_some());
        assert_eq!(hover.key.as_ref().unwrap().0, path_a);
        assert_eq!(hover.key.as_ref().unwrap().1, 2048);

        // Retain with same hovered path preserves key
        hover.retain(Some(path_a));
        assert!(hover.key.is_some());

        // Retain with different hovered path or None clears cache
        hover.retain(Some(path_b));
        assert!(hover.key.is_none());
    }
}
