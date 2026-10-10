use super::super::{
    filesystem::Entry,
    shelf::preview::{Previews, Source},
};
use super::Analysis;
use egui::{Rect, TextureHandle, Ui, vec2};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub(in crate::file_manager) struct Hover {
    cache: Previews,
    key: Option<(PathBuf, uuid::Uuid, u32, uuid::Uuid)>,
    seen: bool,
}
impl Hover {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.key = None;
        self.seen = false;
    }
    pub fn retain(&mut self, paths: &BTreeSet<PathBuf>) {
        if !self.seen || self.key.as_ref().is_some_and(|key| !paths.contains(&key.0)) {
            self.clear();
        }
        self.seen = false;
    }
    pub fn show(
        &mut self,
        ui: &Ui,
        thumbnail: Rect,
        entry: &Entry,
        identity: uuid::Uuid,
        analysis: &Analysis,
        fallback: Option<&TextureHandle>,
    ) {
        self.seen = true;
        let screen = ui.ctx().screen_rect().shrink(8.0);
        let top = thumbnail.bottom() + 6.0;
        let bounds = vec2(screen.width(), screen.bottom() - top);
        if bounds.y < 16.0 {
            return;
        }
        let original = vec2(analysis.dimensions.0 as f32, analysis.dimensions.1 as f32);
        let scale = (bounds.x / original.x).min(bounds.y / original.y).min(1.0);
        let size = original * scale;
        let edge = (size.x.max(size.y) * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(128.0, 4096.0) as u32;
        let edge = edge.div_ceil(128) * 128;
        if !self
            .key
            .as_ref()
            .is_some_and(|key| key.1 == identity && key.2 == edge)
        {
            self.clear();
            self.seen = true;
            self.key = Some((entry.path.clone(), identity, edge, uuid::Uuid::new_v4()));
        }
        let id = self.key.as_ref().unwrap().3;
        if self.cache.needs(id) {
            self.cache.request(
                id,
                Source::Gallery {
                    path: entry.path.clone(),
                    edge,
                    orientation: analysis.orientation,
                },
            );
        }
        let texture = self
            .cache
            .get(id)
            .and_then(|ready| ready.texture.as_ref())
            .or(fallback);
        super::super::ui::preview_popup(
            ui,
            egui::Id::new(("image_strip_hover", identity)),
            thumbnail,
            size,
            texture,
            None,
        );
    }
}
