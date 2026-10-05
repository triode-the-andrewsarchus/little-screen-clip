//! Visible listing previews reuse the Shelf decoder and cancellable cache.
use super::{
    content::Payload,
    filesystem::Entry,
    shelf::preview::{Previews, Source},
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub(super) struct EntryPreviews {
    cache: Previews,
    identities: BTreeMap<std::path::PathBuf, (u64, uuid::Uuid)>,
}
impl EntryPreviews {
    pub fn music(
        &mut self,
        ctx: &egui::Context,
        entry: &Entry,
    ) -> Option<&super::shelf::preview::Ready> {
        self.cache.poll(ctx);
        let id = self.identity(entry);
        if self.cache.needs(id) {
            self.cache.request(id, Source::Music(entry.path.clone()));
        }
        self.cache.get(id)
    }
    fn identity(&mut self, entry: &Entry) -> uuid::Uuid {
        let (version, id) = self
            .identities
            .entry(entry.path.clone())
            .or_insert_with(|| (entry.revision, uuid::Uuid::new_v4()));
        if *version != entry.revision {
            self.cache.invalidate(*id);
            *version = entry.revision;
        }
        *id
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
    }
    pub fn retain(&mut self, visible: &BTreeSet<std::path::PathBuf>) {
        self.identities.retain(|path, _| visible.contains(path));
        self.cache
            .retain(&self.identities.values().map(|(_, id)| *id).collect());
    }
    pub fn show(&mut self, ui: &mut egui::Ui, rect: egui::Rect, entry: &Entry) {
        self.show_inner(ui, rect, entry, false);
    }
    pub fn show_gallery(&mut self, ui: &mut egui::Ui, rect: egui::Rect, entry: &Entry) {
        self.show_inner(ui, rect, entry, true);
    }
    fn show_inner(&mut self, ui: &mut egui::Ui, rect: egui::Rect, entry: &Entry, gallery: bool) {
        let id = self.identity(entry);
        if self.cache.needs(id) {
            self.cache.request(
                id,
                if gallery {
                    Source::Gallery(entry.path.clone())
                } else {
                    Source::Payload(Payload::Files {
                        paths: vec![entry.path.clone()],
                        cut: false,
                    })
                },
            );
        }
        if let Some(ready) = self.cache.get(id) {
            if let Some(texture) = &ready.texture {
                ui.painter().image(
                    texture.id(),
                    if gallery {
                        egui::Rect::from_center_size(
                            rect.center(),
                            texture.size_vec2()
                                * (rect.width() / texture.size_vec2().x)
                                    .min(rect.height() / texture.size_vec2().y),
                        )
                    } else {
                        rect
                    },
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                ready
                    .data
                    .icon
                    .paint(ui.painter(), rect, ui.visuals().text_color());
            }
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
                            egui::Stroke::new(1.0_f32, ui.visuals().text_color()),
                        );
                    }
                }
            });
        } else {
            crate::icons::Icon::Text.paint(ui.painter(), rect, ui.visuals().text_color());
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_entry_replaces_its_preview_and_retired_entries_release_cached_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        let ctx = egui::Context::default();
        let mut previews = EntryPreviews::default();
        let mut previous_id = None;
        for text in ["initial", "updated with more bytes"] {
            std::fs::write(&path, text).unwrap();
            let entry = super::super::filesystem::read_directory(
                dir.path(),
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap()
            .entries
            .remove(0);
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
}
