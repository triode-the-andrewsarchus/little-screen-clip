//! A browser-ordered photo collection retains each editor and its independent undo history.
use super::super::shelf::preview::{Previews, Source};
use super::*;
use crate::{icons::Icon, image_preview::ImageInputStamp};
use clip_ui::buttons::ButtonRole;
use std::collections::BTreeSet;
mod writes;

struct Photo {
    path: PathBuf,
    guard: Option<super::super::search::SourceGuard>,
    session: Option<Box<ImagePreviewState>>,
    preview_id: uuid::Uuid,
    preview_stamp: Option<ImageInputStamp>,
}
pub(super) struct Collection {
    photos: Vec<Photo>,
    index: usize,
    previews: Previews,
    ratings: super::super::photo_ratings::Ratings,
    export: Option<writes::Export>,
    export_open: bool,
    export_directory: String,
    status: Option<String>,
    reveal_active: bool,
}
impl Collection {
    fn new(
        paths: Vec<PathBuf>,
        mut guards: std::collections::HashMap<PathBuf, super::super::search::SourceGuard>,
        index: usize,
    ) -> Self {
        let export_directory = paths[index]
            .parent()
            .unwrap_or(Path::new("."))
            .join("Edited")
            .to_string_lossy()
            .into();
        Self {
            photos: paths
                .into_iter()
                .map(|path| Photo {
                    guard: guards.remove(&path),
                    path,
                    session: None,
                    preview_id: uuid::Uuid::new_v4(),
                    preview_stamp: None,
                })
                .collect(),
            index,
            previews: Previews::default(),
            ratings: Default::default(),
            export: None,
            export_open: false,
            export_directory,
            status: None,
            reveal_active: true,
        }
    }
    pub(super) fn footer(&self, enabled: bool) -> crate::shell::file_editor::PhotoFooter {
        crate::shell::file_editor::PhotoFooter {
            index: self.index,
            count: self.photos.len(),
            rating: self
                .ratings
                .values
                .get(&self.photos[self.index].path)
                .copied()
                .unwrap_or(0),
            enabled,
            changed: false,
        }
    }
    pub(super) fn set_rating(&mut self, rating: u8) {
        self.ratings
            .set(self.photos[self.index].path.clone(), rating);
    }
    pub(super) fn edited(&self, active: Option<&ImagePreviewState>) -> bool {
        self.photos
            .iter()
            .filter_map(|photo| photo.session.as_deref())
            .any(ImagePreviewState::has_collection_edits)
            || active.is_some_and(ImagePreviewState::has_collection_edits)
    }
    pub(super) fn export_controls(
        &mut self,
        ui: &mut Ui,
        active: Option<&ImagePreviewState>,
        ready: bool,
        palette: &AppPalette,
    ) {
        if ui
            .add_enabled(
                ready && self.edited(active),
                ButtonRole::Primary.button(
                    if self.export.is_some() {
                        "Exporting…"
                    } else {
                        "Export all edits"
                    },
                    palette,
                ),
            )
            .clicked()
        {
            self.export_open = true;
        }
    }
    pub(super) fn relocate(&mut self, source: &Path, destination: &Path) {
        for photo in &mut self.photos {
            if photo.path == source {
                photo.path = destination.into();
            }
        }
        self.ratings.relocate(source, destination);
    }
    pub(super) fn dirty(&self) -> bool {
        self.photos.iter().any(|photo| {
            photo
                .session
                .as_deref()
                .is_some_and(ImagePreviewState::has_unsaved_changes)
        })
    }
    fn pending(&mut self) -> bool {
        let mut pending = self.writes_pending();
        for photo in &mut self.photos {
            if let Some(state) = &mut photo.session {
                pending |= state.poll_cached_jobs();
            }
        }
        pending
    }
}
impl Viewers {
    pub(super) fn collection_pending(&mut self) -> bool {
        self.gallery.as_mut().is_some_and(Collection::pending)
    }
    pub(super) fn collection_modal(&self) -> bool {
        self.gallery
            .as_ref()
            .is_some_and(|collection| collection.export_open)
    }
    pub(super) fn collection_dirty(&self) -> bool {
        self.gallery.as_ref().is_some_and(Collection::dirty)
    }
    pub(in crate::file_manager) fn open_collection(
        &mut self,
        path: PathBuf,
        paths: Vec<PathBuf>,
        guards: std::collections::HashMap<PathBuf, super::super::search::SourceGuard>,
    ) {
        if self.active() {
            return;
        }
        self.gallery = paths
            .iter()
            .position(|p| p == &path)
            .map(|index| Collection::new(paths, guards, index));
        self.open(path);
    }
    pub(super) fn gallery_controls(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let Some(mut collection) = self.gallery.take() else {
            return;
        };
        let active = match &mut self.session {
            Some(ViewerSession::Image(state)) => Some(state),
            _ => None,
        };
        let mut active = active;
        if let Some(path) = active.as_ref().and_then(|state| state.path.as_ref()) {
            let photo = &mut collection.photos[collection.index];
            if photo.path != *path {
                collection.ratings.relocate(&photo.path, path);
                photo.path = path.clone();
            }
        }
        collection.poll_export(ui.ctx(), active.as_deref_mut());
        let active_pending = active
            .as_deref_mut()
            .is_some_and(|state| state.poll_cached_jobs());
        let pending = collection.pending() || active_pending;
        let can_back = !self.confirm_close
            && !pending
            && !collection.export_open
            && !self.rename.open
            && !self.rename.busy();
        let ready = self.loading.is_none()
            && can_back
            && active
                .as_ref()
                .is_none_or(|state| !state.show_fallback_save_modal);
        let target = collection.controls(ui, ready, active.as_deref());
        collection.export_ui(ui, active.as_deref_mut(), palette);
        self.gallery = Some(collection);
        if let Some(target) = target {
            self.select_photo(target);
        }
    }
    fn select_photo(&mut self, target: usize) {
        let Some(collection) = &mut self.gallery else {
            return;
        };
        if target == collection.index
            || target >= collection.photos.len()
            || self.loading.is_some()
            || collection.pending()
        {
            return;
        }
        if let Some(ViewerSession::Image(state)) = &mut self.session {
            if !state.poll_session_snapshots() {
                self.error =
                    Some("Wait for the image snapshot to finish before changing photos.".into());
                return;
            }
        } else if self.session.is_some() {
            return;
        }
        if let Some(ViewerSession::Image(state)) = self.session.take() {
            collection.photos[collection.index].session = Some(Box::new(state));
        }
        collection.index = target;
        collection.reveal_active = true;
        self.error = None;
        self.path = None;
        self.source_guard = collection.photos[target].guard.clone();
        if let Some(state) = collection.photos[target].session.take() {
            self.path = state.path.clone();
            self.session = Some(ViewerSession::Image(*state));
        } else {
            let path = collection.photos[target].path.clone();
            self.open(path);
        }
    }
}
impl Collection {
    fn controls(
        &mut self,
        ui: &mut Ui,
        ready: bool,
        active: Option<&ImagePreviewState>,
    ) -> Option<usize> {
        self.previews.poll(ui.ctx());
        let mut target = None;
        let mut visible = Vec::new();
        let mut keep = BTreeSet::new();
        egui::ScrollArea::horizontal()
            .id_source("image_studio_photo_strip")
            .max_height(112.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for index in 0..self.photos.len() {
                        let photo = &mut self.photos[index];
                        let state = if index == self.index {
                            active
                        } else {
                            photo.session.as_deref()
                        };
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(116.0, 100.0), egui::Sense::click());
                        if self.reveal_active && index == self.index {
                            response.scroll_to_me(Some(egui::Align::Center));
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        visible.push(photo.path.clone());
                        keep.insert(photo.preview_id);
                        let stamp = state.map(ImagePreviewState::input_stamp);
                        if stamp != photo.preview_stamp {
                            self.previews.invalidate(photo.preview_id);
                            photo.preview_stamp = stamp;
                        }
                        if self.previews.needs(photo.preview_id) {
                            self.previews.request(
                                photo.preview_id,
                                Source::StudioPhoto {
                                    path: photo.path.clone(),
                                    snapshot: state
                                        .map(ImagePreviewState::display_image_snapshot_request),
                                },
                            );
                        }
                        paint_photo(
                            ui,
                            rect,
                            photo,
                            state,
                            &self.previews,
                            index == self.index,
                            self.ratings.values.get(&photo.path).copied().unwrap_or(0),
                        );
                        if ready && response.clicked() {
                            target = Some(index);
                        }
                        response.on_hover_text(super::super::display_path(&photo.path));
                    }
                });
            });
        self.reveal_active = false;
        self.previews.retain(&keep);
        if let Some(error) = self.ratings.poll(ui.ctx(), &visible) {
            self.status = Some(error);
        }
        if let Some(status) = &self.status {
            ui.label(status);
        }
        self.keyboard(ui, ready).or(target)
    }
    fn keyboard(&mut self, ui: &Ui, ready: bool) -> Option<usize> {
        let text_focused = ui
            .ctx()
            .memory(|m| m.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
        if !ready || text_focused {
            return None;
        }
        for (value, key) in [
            egui::Key::Num0,
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
        ]
        .into_iter()
        .enumerate()
        {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                self.ratings
                    .set(self.photos[self.index].path.clone(), value as u8);
            }
        }
        if self.index > 0
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft))
        {
            return Some(self.index - 1);
        }
        if self.index + 1 < self.photos.len()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight))
        {
            return Some(self.index + 1);
        }
        None
    }
}

fn paint_photo(
    ui: &Ui,
    rect: egui::Rect,
    photo: &Photo,
    state: Option<&ImagePreviewState>,
    previews: &Previews,
    selected: bool,
    rating: u8,
) {
    ui.painter()
        .rect_filled(rect, 5.0, ui.visuals().extreme_bg_color);
    if selected {
        ui.painter().rect_stroke(
            rect,
            5.0,
            egui::Stroke::new(2.0_f32, ui.visuals().selection.bg_fill),
        );
    }
    let image_rect =
        egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(108.0, 66.0));
    if let Some(texture) = previews
        .get(photo.preview_id)
        .and_then(|r| r.texture.as_ref())
    {
        let size = texture.size_vec2();
        let scale = (image_rect.width() / size.x).min(image_rect.height() / size.y);
        ui.painter().image(
            texture.id(),
            egui::Rect::from_center_size(image_rect.center(), size * scale),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        Icon::Image.paint(
            ui.painter(),
            egui::Rect::from_center_size(image_rect.center(), egui::Vec2::splat(24.0)),
            ui.visuals().weak_text_color(),
        );
    }
    if state.is_some_and(ImagePreviewState::has_unsaved_changes) {
        let badge = egui::Rect::from_min_size(image_rect.min, egui::vec2(58.0, 17.0));
        ui.painter()
            .rect_filled(badge, 3.0, egui::Color32::from_rgb(121, 66, 20));
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            "Unsaved",
            egui::FontId::proportional(10.0),
            egui::Color32::WHITE,
        );
    }
    let name = super::super::display_path(Path::new(photo.path.file_name().unwrap_or_default()));
    let galley = ui.painter().layout(
        name,
        egui::FontId::proportional(10.0),
        ui.visuals().text_color(),
        108.0,
    );
    ui.painter().with_clip_rect(rect).galley(
        rect.min + egui::vec2(4.0, 73.0),
        galley,
        ui.visuals().text_color(),
    );

    super::super::photo_ratings::paint_badge(
        ui,
        egui::Rect::from_min_size(
            rect.min + egui::vec2(4.0, 85.0),
            egui::vec2(rating as f32 * 12.0 + 8.0, 14.0),
        ),
        rating,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready_collection() -> (tempfile::TempDir, Viewers) {
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = ["a.png", "b.png"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                image::RgbaImage::from_pixel(4, 4, image::Rgba([40, 80, 120, 255]))
                    .save(&path)
                    .unwrap();
                path
            })
            .collect();
        let config = AppConfig::default();
        let loaded = load(
            &paths[0],
            &config,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let viewer = session(
            loaded,
            paths[0].clone(),
            &config,
            0.0,
            crate::file_source::ReadCheck::default(),
        );
        (
            dir,
            Viewers {
                path: Some(paths[0].clone()),
                session: Some(viewer),
                gallery: Some(Collection::new(paths, Default::default(), 0)),
                ..Viewers::default()
            },
        )
    }
    fn finish_loading(viewers: &mut Viewers) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while viewers.loading.is_some() {
            viewers.poll(&Context::default());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn switching_photos_retains_unsaved_pixels_and_independent_undo_history() {
        let (_dir, mut viewers) = ready_collection();
        let Some(ViewerSession::Image(state)) = &mut viewers.session else {
            panic!()
        };
        let original = state.current_rgba.clone();
        state.undo_stack.push(original.clone());
        std::sync::Arc::make_mut(&mut state.current_rgba).put_pixel(
            0,
            0,
            image::Rgba([255, 0, 0, 255]),
        );
        state.mark_pixels_changed();
        viewers.select_photo(1);
        assert_eq!(viewers.gallery.as_ref().unwrap().index, 1);
        assert!(viewers.collection_dirty());
        finish_loading(&mut viewers);
        let Some(ViewerSession::Image(state)) = &viewers.session else {
            panic!()
        };
        assert!(state.undo_stack.is_empty());
        viewers.select_photo(0);
        let Some(ViewerSession::Image(state)) = &mut viewers.session else {
            panic!()
        };
        assert_eq!(state.current_rgba.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert!(state.has_unsaved_changes());
        assert_eq!(state.undo_stack.len(), 1);
        state.undo();
        assert_eq!(state.current_rgba, original);
        assert_eq!(state.redo_stack.len(), 1);
    }
    #[test]
    fn exported_edits_remain_available_for_export_to_another_folder() {
        let (_dir, mut viewers) = ready_collection();
        let Some(ViewerSession::Image(state)) = &mut viewers.session else {
            panic!()
        };
        state.flip_horizontal();
        state.mark_collection_exported(state.input_stamp());
        assert!(!state.has_unsaved_changes());
        assert!(state.has_collection_edits());
        state.undo();
        assert!(state.has_unsaved_changes());
    }
    #[test]
    fn closing_checks_hidden_edits_and_keeps_source_bytes() {
        let (_dir, mut viewers) = ready_collection();
        let path = viewers.path.clone().unwrap();
        let original = std::fs::read(&path).unwrap();
        if let Some(ViewerSession::Image(state)) = &mut viewers.session {
            state.mark_pixels_changed();
        }
        viewers.select_photo(1);
        finish_loading(&mut viewers);
        assert!(!viewers.request_close());
        assert!(viewers.confirm_close);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    #[test]
    fn renaming_preserves_edits_and_guarded_save_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let mut state = ImagePreviewState::from_path(path.clone(), &AppConfig::default())
            .unwrap()
            .with_file_source(FileSource::capture(&path).unwrap());
        state.undo_stack.push(state.current_rgba.clone());
        state.mark_pixels_changed();
        let (destination, source) = super::super::super::edits::rename_file(
            &path,
            "renamed.png",
            &state.file_source().unwrap(),
        )
        .unwrap();
        state.relocate_file(destination.clone(), source.clone());
        assert!(state.has_unsaved_changes());
        assert_eq!(state.undo_stack.len(), 1);
        assert!(!path.exists());
        source.check().unwrap();
        source.write(&destination, b"saved after rename").unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"saved after rename");
    }
    #[test]
    fn rename_rejects_changed_source_and_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        std::fs::write(&path, b"original").unwrap();
        let source = FileSource::capture(&path).unwrap();
        std::fs::write(dir.path().join("taken.png"), b"existing").unwrap();
        assert!(super::super::super::edits::rename_file(&path, "taken.png", &source).is_err());
        assert_eq!(
            std::fs::read(dir.path().join("taken.png")).unwrap(),
            b"existing"
        );
        std::fs::write(&path, b"external edits").unwrap();
        assert!(super::super::super::edits::rename_file(&path, "new.png", &source).is_err());
        assert!(path.exists());
    }
}
