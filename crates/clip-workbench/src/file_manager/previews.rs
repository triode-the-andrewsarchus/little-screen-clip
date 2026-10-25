//! Visible listing previews reuse the Shelf decoder and cancellable cache.
use super::{
    content::Payload,
    filesystem::Entry,
    shelf::preview::{Previews, Source},
};
use std::collections::{BTreeMap, BTreeSet};

/// One-based item ordinals with a middle ellipsis when a strip is full.
pub(super) fn strip_slots(count: usize, capacity: usize) -> Vec<Option<usize>> {
    let capacity = capacity.max(3);
    if count <= capacity {
        return (1..=count).map(Some).collect();
    }
    let first = capacity / 2;
    let last = capacity - first - 1;
    (1..=first)
        .map(Some)
        .chain(std::iter::once(None))
        .chain((count - last + 1..=count).map(Some))
        .collect()
}
#[derive(Default)]
pub(super) struct EntryPreviews {
    cache: Previews,
    audio_spectrum: bool,
    identities: BTreeMap<std::path::PathBuf, (u64, uuid::Uuid)>,
    gallery_sizes: BTreeMap<std::path::PathBuf, u32>,
    rasters: Previews,
    raster_sizes: BTreeMap<uuid::Uuid, ((u32, u32, Vec<[u8; 3]>), uuid::Uuid)>,
    image_hover: super::image_strip::Hover,
    scenes: super::scene_strip::Previews,
    materials: super::material_strip::Previews,
    pdf_pages: super::pdf_strip::Pages,
    system_refresh: BTreeMap<uuid::Uuid, (u64, std::time::Instant, u32)>,
    text_status: BTreeMap<std::path::PathBuf, (u64, bool)>,
}
impl EntryPreviews {
    pub fn audio_spectrum(&self) -> bool {
        self.audio_spectrum
    }
    pub fn set_audio_spectrum(&mut self, spectrum: bool) {
        if self.audio_spectrum != spectrum {
            self.retire();
            self.audio_spectrum = spectrum;
        }
    }

    pub fn music(
        &mut self,
        ctx: &egui::Context,
        entry: &Entry,
    ) -> Option<&super::shelf::preview::Ready> {
        self.cache.poll(ctx);
        self.listing(entry)
    }

    fn identity(&mut self, entry: &Entry) -> uuid::Uuid {
        let (version, id) = self
            .identities
            .entry(entry.path.clone())
            .or_insert_with(|| (entry.revision, uuid::Uuid::new_v4()));
        if *version != entry.revision {
            self.cache.invalidate(*id);
            if let Some((_, raster)) = self.raster_sizes.remove(id) {
                self.rasters.invalidate(raster);
            }
            *version = entry.revision;
            self.system_refresh.remove(id);
        }
        // A failed first collection has no Document, but still needs bounded
        // visible retries. Classification owns scheduling, not successful data.
        if entry.system.is_some() {
            self.system_refresh.entry(*id).or_insert_with(|| {
                (
                    super::system::event_revision(),
                    std::time::Instant::now(),
                    0,
                )
            });
        }
        *id
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        self.rasters.poll(ctx);
        self.pdf_pages.poll(ctx);
        self.image_hover.poll(ctx);
        self.scenes.poll(ctx);
        self.materials.poll(ctx);
        let now = std::time::Instant::now();

        let systems = self
            .identities
            .iter()
            .filter_map(|(path, (_, id))| {
                self.system_refresh.contains_key(id).then_some(())?;
                self.cache.get(*id).map(|ready| {
                    let failed = ready
                        .data
                        .system
                        .as_ref()
                        .is_none_or(|document| document.last_error.is_some());
                    (path.clone(), *id, failed)
                })
            })
            .collect::<Vec<_>>();
        let event = if systems.is_empty() {
            0
        } else {
            super::system::event_revision()
        };
        self.refresh_systems(ctx, now, event, systems);
        let changed = self.cache.poll(ctx);
        if changed {
            for (path, (revision, id)) in &self.identities {
                if let Some(ready) = self.cache.get(*id) {
                    self.text_status.insert(
                        path.clone(),
                        (
                            *revision,
                            ready.data.document.is_some() || ready.data.binary.is_some(),
                        ),
                    );
                }
            }
        }
        changed
    }
    fn refresh_systems(
        &mut self,
        ctx: &egui::Context,
        now: std::time::Instant,
        event: u64,
        systems: Vec<(std::path::PathBuf, uuid::Uuid, bool)>,
    ) {
        for (path, id, failed) in systems {
            let previous = self.system_refresh.entry(id).or_insert((event, now, 0));
            if !failed {
                previous.2 = 0;
            }
            let elapsed = now.duration_since(previous.1);
            if elapsed >= std::time::Duration::from_secs(5)
                && (previous.0 != event
                    || elapsed
                        >= std::time::Duration::from_secs((5_u64 << previous.2.min(3)).min(30)))
            {
                if self.cache.refresh(id, Source::System(path)) {
                    *previous = (
                        event,
                        now,
                        if failed {
                            previous.2.saturating_add(1)
                        } else {
                            0
                        },
                    );
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
    pub fn retain(&mut self, visible: &BTreeSet<std::path::PathBuf>) {
        if visible.is_empty() {
            self.text_status.clear();
        }
        self.pdf_pages.retain(visible);
        self.image_hover.retain(visible);
        self.scenes.retain(visible);
        self.identities.retain(|path, _| visible.contains(path));
        self.gallery_sizes.retain(|path, _| visible.contains(path));
        self.cache
            .retain(&self.identities.values().map(|(_, id)| *id).collect());
        let ids: BTreeSet<_> = self.identities.values().map(|(_, id)| *id).collect();
        self.materials.retain(&ids);
        self.system_refresh.retain(|id, _| ids.contains(id));
        self.raster_sizes.retain(|id, _| ids.contains(id));
        self.rasters
            .retain(&self.raster_sizes.values().map(|(_, id)| *id).collect());
    }
    pub fn is_binary(&self, entry: &Entry) -> bool {
        entry.regular_file
            && !little_clip_core::document::is_office_document(&entry.path)
            && (super::binary_strip::known(&entry.path)
                || self
                    .identities
                    .get(&entry.path)
                    .and_then(|(_, id)| self.cache.get(*id))
                    .is_some_and(|ready| ready.data.binary.is_some()))
    }
    pub fn is_text(&self, entry: &Entry) -> bool {
        if !text_candidate(entry) {
            return false;
        }
        let known = self
            .text_status
            .get(&entry.path)
            .filter(|(revision, _)| *revision == entry.revision);
        known.map_or_else(
            || {
                super::text_strip::known_text(&entry.path)
                    || super::binary_strip::known(&entry.path)
            },
            |(_, text)| *text,
        )
    }
    /// Stop hidden preview work without changing this folder's row geometry.
    pub fn retire(&mut self) {
        self.cache.clear();
        self.pdf_pages.clear();
        self.image_hover.clear();
        self.scenes.clear();
        self.materials.clear();
        self.identities.clear();
        self.gallery_sizes.clear();
        self.rasters.clear();
        self.raster_sizes.clear();
        self.system_refresh.clear();
    }
    pub fn listing(&mut self, entry: &Entry) -> Option<&super::shelf::preview::Ready> {
        let id = self.identity(entry);
        if self.cache.needs(id) {
            self.cache.request(
                id,
                if entry.system.is_some() {
                    Source::System(entry.path.clone())
                } else if entry.regular_file && entry.is_photo() {
                    Source::Photo {
                        path: entry.path.clone(),
                        revision: entry.revision,
                    }
                } else if entry.regular_file && super::font_strip::known(&entry.path) {
                    Source::Font(entry.path.clone())
                } else if entry.is_blend() {
                    Source::Blend(entry.path.clone())
                } else if entry.is_material() {
                    Source::Material(super::material_strip::Request::Library(entry.path.clone()))
                } else if super::midi_strip::is_midi(&entry.path) {
                    Source::Midi(entry.path.clone())
                } else if little_clip_core::audio::is_audio_path(&entry.path) {
                    Source::Music(entry.path.clone(), self.audio_spectrum)
                } else if super::binary_strip::known(&entry.path) && entry.regular_file {
                    Source::Binary(entry.path.clone())
                } else if text_candidate(entry) {
                    Source::Text(entry.path.clone())
                } else {
                    Source::Payload(Payload::Files {
                        paths: vec![entry.path.clone()],
                        cut: false,
                    })
                },
            );
        }
        self.cache.get(id)
    }
    pub fn show_scene(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        entry: &Entry,
        columns: Option<super::ui::ListColumns>,
        colors: crate::ui_appearance::WorkbenchColors,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        self.scenes.show(ui, row, entry, columns, colors, palette);
    }
    pub fn show_visual(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        entry: &Entry,
        columns: Option<super::ui::ListColumns>,
        colors: crate::ui_appearance::WorkbenchColors,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let id = self.identity(entry);
        self.listing(entry);
        if entry.system.is_some() {
            super::system::show(ui, row, entry, columns, self.cache.get(id), colors, palette);
        } else if super::font_strip::known(&entry.path) {
            let document = self.cache.get(id).and_then(|r| r.data.font.clone());
            let strip =
                super::font_strip::show(ui, row, entry, columns, self.cache.get(id), colors);
            if let Some(document) = document {
                let (width, height) = raster_dimensions(ui, strip);
                let color = [
                    colors.text_primary.r(),
                    colors.text_primary.g(),
                    colors.text_primary.b(),
                ];
                let source = Source::FontRaster {
                    document,
                    width,
                    height,
                    color,
                };
                self.paint_raster(ui, strip, id, (width, height, vec![color]), source);
            }
        } else if entry.is_blend() {
            super::blend_preview::show(ui, row, entry, columns, self.cache.get(id), colors);
        } else if entry.is_material() {
            self.materials
                .show(ui, row, entry, columns, id, self.cache.get(id), colors);
        } else {
            super::image_strip::show(
                ui,
                row,
                entry,
                columns,
                id,
                self.cache.get(id),
                &mut self.image_hover,
                colors,
            );
        }
    }
    pub fn show_midi(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        entry: &Entry,
        palette: &little_clip_core::palette::AppPalette,
    ) -> Option<std::sync::Arc<super::midi_strip::Document>> {
        let id = self.identity(entry);
        let document = self.listing(entry)?.data.midi_document.clone()?;
        let (width, height) = raster_dimensions(ui, rect);
        let colors: Vec<_> = document
            .groups
            .iter()
            .map(|(track, channel)| {
                let color = crate::midi_view::get_track_color(
                    *track,
                    *channel,
                    document.data.track_count,
                    palette,
                );
                [color.r(), color.g(), color.b()]
            })
            .collect();
        let source = Source::MidiRaster {
            document: document.clone(),
            width,
            height,
            colors: colors.clone(),
        };
        self.paint_raster(ui, rect, id, (width, height, colors), source);
        Some(document)
    }
    fn paint_raster(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        id: uuid::Uuid,
        key: (u32, u32, Vec<[u8; 3]>),
        source: Source,
    ) {
        if self
            .raster_sizes
            .get(&id)
            .is_some_and(|(previous, _)| *previous != key)
        {
            if let Some((_, raster)) = self.raster_sizes.remove(&id) {
                self.rasters.invalidate(raster);
            }
        }
        let (_, raster) = self
            .raster_sizes
            .entry(id)
            .or_insert_with(|| (key, uuid::Uuid::new_v4()));
        if self.rasters.needs(*raster) {
            self.rasters.request(*raster, source);
        }
        if let Some(texture) = self
            .rasters
            .get(*raster)
            .and_then(|ready| ready.texture.as_ref())
        {
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
    }
    pub fn show_pdf(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        entry: &Entry,
        colors: crate::ui_appearance::WorkbenchColors,
    ) -> Option<usize> {
        let id = self.identity(entry);
        let info = self
            .listing(entry)
            .and_then(|ready| ready.data.pdf_info.clone());
        self.pdf_pages
            .show(ui, row, entry, id, info.as_deref(), colors)
    }
    pub fn show_listing(
        &mut self,
        ui: &mut egui::Ui,
        row: egui::Rect,
        entry: &Entry,
        columns: Option<super::ui::ListColumns>,
        colors: crate::ui_appearance::WorkbenchColors,
        palette: &little_clip_core::palette::AppPalette,
    ) -> Option<usize> {
        let id = self.identity(entry);
        self.listing(entry);
        if self.is_binary(entry) {
            let document = self
                .cache
                .get(id)
                .and_then(|ready| ready.data.binary.clone());
            let strip =
                super::binary_strip::show(ui, row, entry, columns, self.cache.get(id), colors);
            if let Some(document) = document {
                let (width, height) = raster_dimensions(ui, strip);
                let source = Source::BinaryRaster {
                    document,
                    width,
                    height,
                };
                self.paint_raster(ui, strip, id, (width, height, Vec::new()), source);
            }
            return None;
        }
        if row.height() <= 28.0 {
            self.show(
                ui,
                egui::Rect::from_min_size(row.min + egui::vec2(6.0, 4.0), egui::vec2(20.0, 20.0)),
                entry,
            );
            return None;
        }
        let badge = self
            .listing(entry)
            .and_then(|ready| ready.data.document.as_ref())
            .map(|document| document.badge.clone())
            .unwrap_or_else(|| {
                entry
                    .path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("TXT")
                    .to_uppercase()
            });
        let icon = egui::Rect::from_min_size(
            row.min + egui::vec2(6.0, 32.0),
            egui::vec2(64.0, row.height() - 42.0),
        );
        crate::icons::Icon::Text.paint(
            ui.painter(),
            egui::Rect::from_center_size(
                icon.center() - egui::vec2(0.0, 9.0),
                egui::vec2(26.0, 26.0),
            ),
            colors.text_secondary,
        );
        ui.painter().text(
            icon.center() + egui::vec2(0.0, 18.0),
            egui::Align2::CENTER_CENTER,
            badge,
            egui::FontId::monospace(12.0),
            colors.text_secondary,
        );
        let strip = egui::Rect::from_min_max(
            egui::pos2(icon.right() + 8.0, icon.top()),
            egui::pos2(row.right() - 6.0, icon.bottom()),
        );
        ui.painter().rect_filled(strip, 3.0, colors.recessed);
        self.text_strip(ui, strip, entry, palette);
        let clicked_line = ui
            .ctx()
            .pointer_interact_pos()
            .filter(|point| ui.is_enabled() && strip.intersect(ui.clip_rect()).contains(*point))
            .and_then(|point| {
                self.listing(entry)
                    .and_then(|ready| ready.data.document.as_ref())
                    .map(|document| {
                        let fraction = ((point.x - strip.left()) / strip.width()).clamp(0.0, 1.0);
                        // Source lines progress horizontally, including blank lines.
                        ((fraction as f64 * document.lines as f64).floor() as usize + 1)
                            .min(document.lines as usize)
                            .max(1)
                    })
            });
        if super::ui::preview_hovered(ui, row)
            && ui
                .ctx()
                .pointer_hover_pos()
                .is_some_and(|point| !strip.contains(point))
            && let Some(document) = self
                .listing(entry)
                .and_then(|ready| ready.data.document.clone())
        {
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("text_strip_hover", &entry.path)),
                |ui| document.excerpt_ui(ui, 0.0, usize::MAX, palette),
            );
        }
        clicked_line
    }
    pub fn text_strip(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        entry: &Entry,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let id = self.identity(entry);
        let Some(document) = self
            .cache
            .get(id)
            .and_then(|ready| ready.data.document.clone())
        else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Loading text preview…",
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
            return;
        };
        let (width, height) = raster_dimensions(ui, rect);
        let colors = super::text_strip::palette_colors(palette);
        let source = Source::TextRaster {
            document: document.clone(),
            width,
            height,
            colors,
        };
        self.paint_raster(ui, rect, id, (width, height, colors.to_vec()), source);
        let hovered = super::ui::preview_hovered(ui, rect);
        if hovered {
            let point = ui.ctx().pointer_hover_pos().unwrap_or(rect.min);
            let fraction = ((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            ui.painter().line_segment(
                [
                    egui::pos2(point.x, rect.top()),
                    egui::pos2(point.x, rect.bottom()),
                ],
                egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
            );
            egui::show_tooltip_at_pointer(
                ui.ctx(),
                egui::Id::new(("text_strip_hover", id)),
                |ui| document.excerpt_ui(ui, fraction, width as usize, palette),
            );
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, rect: egui::Rect, entry: &Entry) {
        self.show_inner(ui, rect, entry, false);
    }
    pub fn show_video(
        &mut self,
        ui: &mut egui::Ui,
        top_left: egui::Pos2,
        height: f32,
        entry: &Entry,
    ) -> egui::Rect {
        let id = self.identity(entry);
        let aspect = self
            .cache
            .get(id)
            .and_then(|ready| ready.texture.as_ref())
            .map_or(16.0 / 9.0, |texture| {
                let size = texture.size_vec2();
                size.x / size.y.max(1.0)
            });
        let rect = egui::Rect::from_min_size(top_left, egui::vec2(height * aspect, height));
        self.show(ui, rect, entry);
        rect
    }
    pub fn is_ready(&mut self, entry: &Entry) -> bool {
        let id = self.identity(entry);
        self.cache.get(id).is_some()
    }
    pub fn has_pending(&self) -> bool {
        self.cache.has_pending()
    }

    pub const MIN_GALLERY_EDGE: u32 = 128;
    pub const MAX_GALLERY_EDGE: u32 = 4096;
    pub const GALLERY_EDGE_STEP: u32 = 128;

    pub fn gallery_size(&self, path: &std::path::Path) -> u32 {
        self.gallery_sizes.get(path).copied().unwrap_or(0)
    }

    pub fn request_gallery(&mut self, entry: &Entry, edge: u32, orientation: u8) {
        let id = self.identity(entry);
        let edge = edge
            .clamp(Self::MIN_GALLERY_EDGE, Self::MAX_GALLERY_EDGE)
            .div_ceil(Self::GALLERY_EDGE_STEP)
            * Self::GALLERY_EDGE_STEP;
        let previous = self.gallery_sizes.entry(entry.path.clone()).or_default();
        if edge > *previous {
            let started = self.cache.refresh(
                id,
                Source::Gallery {
                    path: entry.path.clone(),
                    revision: entry.revision,
                    edge,
                    orientation,
                },
            );
            if started {
                *previous = edge;
            }
        } else if self.cache.needs(id) {
            let started = self.cache.request(
                id,
                Source::Gallery {
                    path: entry.path.clone(),
                    revision: entry.revision,
                    edge,
                    orientation,
                },
            );
            if started {
                *previous = edge;
            }
        }
    }
    pub fn show_gallery(&mut self, ui: &mut egui::Ui, rect: egui::Rect, entry: &Entry) {
        self.show_inner(ui, rect, entry, true);
    }
    fn show_inner(&mut self, ui: &mut egui::Ui, rect: egui::Rect, entry: &Entry, gallery: bool) {
        let id = self.identity(entry);
        if !gallery && self.cache.needs(id) {
            self.cache.request(
                id,
                Source::Payload(Payload::Files {
                    paths: vec![entry.path.clone()],
                    cut: false,
                }),
            );
        }
        if let Some(ready) = self.cache.get(id) {
            if let Some(texture) = &ready.texture {
                let size = texture.size_vec2();
                let full_uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                let (destination, uv) = if gallery {
                    let scale = (rect.width() / size.x).max(rect.height() / size.y);
                    let extent = rect.size() / (size * scale);
                    (
                        rect,
                        egui::Rect::from_center_size(egui::pos2(0.5, 0.5), extent),
                    )
                } else {
                    (
                        egui::Rect::from_center_size(
                            rect.center(),
                            size * (rect.width() / size.x).min(rect.height() / size.y),
                        ),
                        full_uv,
                    )
                };
                ui.painter()
                    .image(texture.id(), destination, uv, egui::Color32::WHITE);
            } else {
                ready
                    .data
                    .icon
                    .paint(ui.painter(), rect, ui.visuals().text_color());
            }
            if !gallery && !super::ui::popup_open(ui.ctx()) {
                ui.interact(
                    rect,
                    egui::Id::new(("listing_preview", id)),
                    egui::Sense::hover(),
                )
                .on_hover_ui(|ui| {
                    ui.label(&ready.data.detail);
                    if let Some(texture) = &ready.texture {
                        ui.image((texture.id(), texture.size_vec2()));
                    }
                    if !ready.data.text.is_empty() {
                        ui.label(&ready.data.text);
                    }
                    if !ready.data.waveform.is_empty() {
                        let (wave, _) =
                            ui.allocate_exact_size(egui::vec2(180.0, 40.0), egui::Sense::hover());
                        let step = wave.width() / ready.data.waveform.len() as f32;
                        for (i, level) in ready.data.waveform.iter().enumerate() {
                            let x = wave.left() + i as f32 * step;
                            let amp = level * wave.height() / 2.0;
                            ui.painter().line_segment(
                                [
                                    egui::pos2(x, wave.center().y - amp),
                                    egui::pos2(x, wave.center().y + amp),
                                ],
                                egui::Stroke::new(
                                    1.0_f32,
                                    ready
                                        .data
                                        .waveform_colors
                                        .get(i)
                                        .map_or(ui.visuals().text_color(), |rgb| {
                                            egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2])
                                        }),
                                ),
                            );
                        }
                    }
                });
            }
        } else {
            let icon = if entry.is_video() {
                crate::icons::Icon::Film
            } else if gallery {
                crate::icons::Icon::Image
            } else {
                crate::icons::Icon::Text
            };
            icon.paint(ui.painter(), rect, ui.visuals().text_color());
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

fn raster_dimensions(ui: &egui::Ui, rect: egui::Rect) -> (u32, u32) {
    (
        (rect.width() * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(1.0, 2048.0) as u32,
        (rect.height() * ui.ctx().pixels_per_point())
            .ceil()
            .clamp(1.0, 128.0) as u32,
    )
}

fn text_candidate(entry: &Entry) -> bool {
    entry.regular_file
        && !super::font_strip::known(&entry.path)
        && !super::midi_strip::is_midi(&entry.path)
        && !entry.is_scene()
        && !entry.is_blend()
        && !entry.is_material()
        && !entry.is_photo()
        && !entry.is_video()
        && !little_clip_core::audio::is_audio_path(&entry.path)
        && !entry
            .path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        && !little_clip_core::document::is_office_document(&entry.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_preview(mut ready: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "preview worker did not complete"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn single_entry(folder: &std::path::Path) -> Entry {
        let mut directory = super::super::filesystem::read_directory(
            folder,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(directory.entries.len(), 1, "fixture must contain one file");
        directory.entries.pop().unwrap()
    }

    #[test]
    fn initial_system_failure_retries_and_recovery_resets_backoff() {
        use std::os::unix::fs::symlink;
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("system");
        symlink("/proc/uptime", &path).unwrap();
        let entry = single_entry(dir.path());
        assert!(entry.system.is_some());
        std::fs::remove_file(&path).unwrap();
        let ctx = egui::Context::default();
        let mut previews = EntryPreviews::default();
        previews.listing(&entry);
        let id = previews.identities[&path].1;
        wait_for_preview(|| {
            previews.poll(&ctx);
            previews.cache.get(id).is_some()
        });
        assert!(previews.cache.get(id).unwrap().data.system.is_none());
        let initial = previews.system_refresh[&id];
        previews.poll(&ctx);
        assert_eq!(
            previews.system_refresh[&id], initial,
            "no immediate retry loop"
        );
        previews.system_refresh.get_mut(&id).unwrap().1 = Instant::now() - Duration::from_secs(6);
        previews.poll(&ctx);
        assert_eq!(
            previews.system_refresh[&id].2, 1,
            "first failure uses existing backoff"
        );
        for (seconds, failures) in [(10, 1), (20, 2), (30, 3), (30, 4)] {
            // Retire the previous worker so queue timing cannot decide whether
            // the next retry was admitted. Advance only the scheduling clock.
            previews.cache.invalidate(id);
            let previous = previews.system_refresh[&id];
            previews.refresh_systems(
                &ctx,
                previous.1 + Duration::from_secs(seconds - 1),
                previous.0,
                vec![(path.clone(), id, true)],
            );
            assert_eq!(
                previews.system_refresh[&id], previous,
                "backoff must hold for {seconds}s"
            );
            previews.refresh_systems(
                &ctx,
                previous.1 + Duration::from_secs(seconds),
                previous.0,
                vec![(path.clone(), id, true)],
            );
            assert_eq!(previews.system_refresh[&id].2, failures + 1);
        }
        previews.cache.invalidate(id);
        let previous = previews.system_refresh[&id];
        let changed_event = previous.0 + 1;
        previews.refresh_systems(
            &ctx,
            previous.1 + Duration::from_secs(4),
            changed_event,
            vec![(path.clone(), id, true)],
        );
        assert_eq!(
            previews.system_refresh[&id], previous,
            "events cannot exceed the five-second measurement rate"
        );
        previews.refresh_systems(
            &ctx,
            previous.1 + Duration::from_secs(5),
            changed_event,
            vec![(path.clone(), id, true)],
        );
        assert_eq!(
            previews.system_refresh[&id].0, changed_event,
            "event invalidation bypasses longer backoff after the rate limit"
        );
        previews.cache.invalidate(id);
        // Inject a successful outcome at the same scheduling boundary used by
        // poll. Unit-test executables do not dispatch the GUI's internal helper.
        let recovered = previews.system_refresh[&id].1 + Duration::from_secs(1);
        previews.refresh_systems(
            &ctx,
            recovered,
            previews.system_refresh[&id].0,
            vec![(path.clone(), id, false)],
        );
        assert_eq!(
            previews.system_refresh[&id].2, 0,
            "recovery restores normal freshness immediately"
        );
        previews.retain(&BTreeSet::new());
        assert!(previews.system_refresh.is_empty());
        assert!(previews.cache.get(id).is_none());
        previews.poll(&ctx);
        assert!(
            previews.system_refresh.is_empty(),
            "hidden rows must not restart work"
        );
    }

    #[test]
    fn text_resize_reuses_analysis_and_retirement_preserves_row_classification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.unknown");
        std::fs::write(&path, "A walk in the garden.\n".repeat(150)).unwrap();
        let entry = single_entry(dir.path());
        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut previews = EntryPreviews::default();
        previews.listing(&entry);
        wait_for_preview(|| {
            previews.poll(&ctx);
            previews.listing(&entry).is_some()
        });
        assert!(previews.is_text(&entry));
        let id = previews.identity(&entry);
        let document = previews
            .cache
            .get(id)
            .unwrap()
            .data
            .document
            .clone()
            .unwrap();
        for width in [512.0, 256.0] {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    previews.text_strip(
                        ui,
                        egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(width, 66.0)),
                        &entry,
                        &palette,
                    );
                });
            });
            let raster = previews.raster_sizes[&id].1;
            wait_for_preview(|| {
                previews.poll(&ctx);
                previews.rasters.get(raster).is_some()
            });
            assert_eq!(
                previews
                    .rasters
                    .get(raster)
                    .unwrap()
                    .texture
                    .as_ref()
                    .unwrap()
                    .size()[0],
                width as usize
            );
            assert!(std::sync::Arc::ptr_eq(
                &document,
                previews
                    .cache
                    .get(id)
                    .unwrap()
                    .data
                    .document
                    .as_ref()
                    .unwrap()
            ));
        }
        // Retain another visible file: release the heavy summary and raster,
        // but keep this folder's row height stable when scrolling back.
        previews.retain(&[dir.path().join("another.txt")].into_iter().collect());
        assert!(previews.cache.get(id).is_none());
        assert!(previews.raster_sizes.is_empty());
        assert!(previews.is_text(&entry));
        previews.retire();
        assert!(previews.is_text(&entry));
        previews.retain(&Default::default());
        assert!(!previews.is_text(&entry));
    }

    #[test]
    fn changed_entry_replaces_its_preview_and_retired_entries_release_cached_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        let ctx = egui::Context::default();
        let mut previews = EntryPreviews::default();
        let mut previous_id = None;
        for text in ["initial", "updated with more bytes"] {
            std::fs::write(&path, text).unwrap();
            let entry = single_entry(dir.path());
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    previews.show(
                        ui,
                        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(20.0, 20.0)),
                        &entry,
                    );
                });
            });
            let id = previews.identities[&path].1;
            if let Some(previous) = previous_id {
                assert_eq!(id, previous);
                assert!(previews.cache.get(id).is_none());
            }
            previous_id = Some(id);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while previews.cache.get(id).is_none() {
                previews.poll(&ctx);
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(previews.cache.get(id).unwrap().data.text, text);
            assert_eq!(previews.identities.len(), 1);
        }
        previews.retain(&BTreeSet::new());
        assert!(previews.identities.is_empty());
        assert!(previews.cache.get(previous_id.unwrap()).is_none());
    }

    #[test]
    fn office_document_is_not_binary_and_provides_pdf_info() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.rtf");
        std::fs::write(
            &path,
            b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Courier;}}\\f0\\fs24 Office Document Preview Test\\par Page 1.}",
        )
        .unwrap();

        let entry = single_entry(dir.path());
        let mut previews = EntryPreviews::default();
        let ctx = egui::Context::default();

        assert!(!previews.is_binary(&entry));
        assert!(!previews.is_text(&entry));

        previews.listing(&entry);
        let id = previews.identity(&entry);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while previews.cache.get(id).is_none() {
            previews.poll(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "office doc preview timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let ready = previews.cache.get(id).unwrap();
        assert!(!previews.is_binary(&entry));
        assert!(
            ready.data.pdf_info.is_some(),
            "expected pdf_info for office doc"
        );
        assert_eq!(ready.data.pdf_info.as_ref().unwrap().page_count, 1);
        assert_eq!(ready.data.icon, crate::icons::Icon::Pdf);
    }
}
