use super::super::{
    filesystem::Entry,
    shelf::preview::{AnimatedTexture, Previews, Source},
};
use super::Analysis;
use egui::{Rect, TextureHandle, Ui, vec2};
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub(in crate::file_manager) struct Hover {
    pub(in crate::file_manager) context: super::super::search::PreviewContext,
    cache: Previews,
    animation: AnimatedTexture,
    key: Option<(PathBuf, uuid::Uuid, u32, uuid::Uuid)>,
    seen: bool,
    segment_hovered: bool,
    veil_texture: Option<(uuid::Uuid, usize, TextureHandle)>,
    veil_tint: Option<[f32; 4]>,
}
impl Hover {
    #[cfg(test)]
    pub fn is_veil_active(&self) -> bool {
        self.veil_texture.is_some()
    }
    #[cfg(test)]
    pub fn current_veil_tint(&self) -> Option<[f32; 4]> {
        self.veil_tint
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
        if let Some((_, _, _, id)) = self.key {
            self.animation
                .poll(ctx, id, self.cache.get(id), !self.segment_hovered);
        } else {
            self.animation.clear();
        }
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.animation.clear();
        self.key = None;
        self.seen = false;
        self.segment_hovered = false;
        self.veil_texture = None;
        self.veil_tint = None;
    }
    pub fn retain(&mut self, paths: &BTreeSet<PathBuf>) {
        if !self.seen || self.key.as_ref().is_some_and(|key| !paths.contains(&key.0)) {
            self.clear();
        }
        self.seen = false;
    }
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &Ui,
        thumbnail: Rect,
        entry: &Entry,
        identity: uuid::Uuid,
        analysis: &Analysis,
        fallback: Option<&TextureHandle>,
        segment: Option<usize>,
    ) {
        self.seen = true;
        self.segment_hovered = segment.is_some();
        let screen = ui.ctx().content_rect().shrink(8.0);
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
            self.segment_hovered = segment.is_some();
            self.key = Some((entry.path.clone(), identity, edge, uuid::Uuid::new_v4()));
        }
        let id = self.key.as_ref().unwrap().3;
        if self.cache.needs(id) {
            self.cache.request(
                id,
                Source::Gallery {
                    path: entry.path.clone(),
                    revision: entry.revision,
                    edge,
                    orientation: Some(analysis.orientation),
                    animate: true,
                }
                .checked(&entry.path, &self.context),
            );
        }
        let texture = segment
            .is_none()
            .then(|| self.animation.texture())
            .flatten()
            .or_else(|| self.cache.get(id).and_then(|ready| ready.texture.as_ref()))
            .or(fallback);

        if let Some(seg) = segment {
            let target = analysis
                .hilbert_strip
                .veil_tint_for_segment(seg)
                .map(f32::from);
            let current = match self.veil_tint {
                Some(prev) => {
                    let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
                    // Smooth transition over ~5-6 frames (~80-100ms at 60fps)
                    let rate = 14.0;
                    let factor = 1.0 - (-rate * dt).exp();
                    let tint = std::array::from_fn(|i| prev[i] + (target[i] - prev[i]) * factor);
                    if tint
                        .iter()
                        .zip(target)
                        .any(|(value, target)| (target - value).abs() > 1.0)
                    {
                        ui.ctx().request_repaint();
                        tint
                    } else {
                        target
                    }
                }
                None => target,
            };
            self.veil_tint = Some(current);

            if !self
                .veil_texture
                .as_ref()
                .is_some_and(|(id, s, _)| *id == identity && *s == seg)
            {
                let veil_image = analysis.hilbert_strip.create_veil_mask(seg);
                let handle = ui.ctx().load_texture(
                    format!("hilbert_veil_{identity}_{seg}"),
                    veil_image,
                    egui::TextureOptions::NEAREST,
                );
                self.veil_texture = Some((identity, seg, handle));
            }
        } else {
            self.veil_texture = None;
            self.veil_tint = None;
        }

        let overlay =
            self.veil_texture
                .as_ref()
                .zip(self.veil_tint)
                .map(|((_, _, handle), tint)| {
                    let [r, g, b, a] = tint.map(|value| value.round() as u8);
                    (handle, egui::Color32::from_rgba_unmultiplied(r, g, b, a))
                });

        super::super::ui::preview_popup_with_overlay(
            ui,
            egui::Id::unique(("image_strip_hover", identity)),
            thumbnail,
            size,
            texture,
            overlay,
            None,
        );
    }
}
