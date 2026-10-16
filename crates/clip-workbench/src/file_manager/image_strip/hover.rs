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
    veil_texture: Option<(uuid::Uuid, usize, TextureHandle)>,
    dimmed_color: Option<[f32; 3]>,
}
impl Hover {
    #[cfg(test)]
    pub fn is_veil_active(&self) -> bool {
        self.veil_texture.is_some()
    }
    #[cfg(test)]
    pub fn current_dimmed_color(&self) -> Option<[f32; 3]> {
        self.dimmed_color
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.key = None;
        self.seen = false;
        self.veil_texture = None;
        self.dimmed_color = None;
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

        if let Some(seg) = segment {
            let (tr, tg, tb) = analysis.hilbert_strip.dimmed_color_for_segment(seg);
            let target = [tr as f32, tg as f32, tb as f32];
            let current = match self.dimmed_color {
                Some(prev) => {
                    let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.1);
                    // Smooth transition over ~5-6 frames (~80-100ms at 60fps)
                    let rate = 14.0;
                    let factor = 1.0 - (-rate * dt).exp();
                    let new_color = [
                        prev[0] + (target[0] - prev[0]) * factor,
                        prev[1] + (target[1] - prev[1]) * factor,
                        prev[2] + (target[2] - prev[2]) * factor,
                    ];
                    if (target[0] - new_color[0]).abs() > 1.0
                        || (target[1] - new_color[1]).abs() > 1.0
                        || (target[2] - new_color[2]).abs() > 1.0
                    {
                        ui.ctx().request_repaint();
                        new_color
                    } else {
                        target
                    }
                }
                None => target,
            };
            self.dimmed_color = Some(current);

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
            self.dimmed_color = None;
        }

        let overlay = self.veil_texture.as_ref().map(|(_, _, handle)| {
            let tint = match self.dimmed_color {
                Some(c) => egui::Color32::from_rgb(
                    c[0].round().clamp(0.0, 255.0) as u8,
                    c[1].round().clamp(0.0, 255.0) as u8,
                    c[2].round().clamp(0.0, 255.0) as u8,
                ),
                None => egui::Color32::WHITE,
            };
            (handle, tint)
        });

        super::super::ui::preview_popup_with_overlay(
            ui,
            egui::Id::new(("image_strip_hover", identity)),
            thumbnail,
            size,
            texture,
            overlay,
            None,
        );
    }
}
