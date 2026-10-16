//! A bottom-anchored shortcut list leaves the remaining sidebar to the folder tree.
use super::*;

impl FileManager {
    pub(super) fn sidebar_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        let bounds = ui.available_rect_before_wrap();
        if !self.bookmarks.entries.is_empty() {
            let row = ROW_HEIGHT + ui.spacing().item_spacing.y;
            let height =
                (36.0 + self.bookmarks.entries.len() as f32 * row).min(bounds.height() * 0.5);
            let panel = egui::TopBottomPanel::bottom("files_bookmarks")
                .exact_height(height)
                .frame(egui::Frame::none())
                .show_inside(ui, |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    self.bookmarks_ui(ui, colors);
                });
            self.bookmark_drop_rect = Some(panel.response.rect.intersect(bounds));
        } else {
            // Invisible until a drag reaches it. The first bookmark can also be
            // added through Edit, without reserving an empty section.
            self.bookmark_drop_rect = Some(Rect::from_min_max(
                pos2(bounds.left(), (bounds.bottom() - 32.0).max(bounds.top())),
                bounds.max,
            ));
        }
        if !ui.is_enabled() {
            self.bookmark_drop_rect = None;
        }
        let drop = self
            .bookmark_drop_rect
            .filter(|_| ui.is_enabled())
            .map(|rect| {
                (
                    rect,
                    ui.interact(rect, Id::new("files_bookmark_drop"), Sense::hover()),
                )
            });
        if let Some((_, response)) = &drop
            && let Some(payload) =
                response.dnd_release_payload::<super::super::transfers::FileDrag>()
        {
            self.bookmarks.add(payload.paths.clone(), None);
        }
        ui.scope(|ui| {
            // Scroll areas allow a paint margin; keep it inside the space left
            // above Bookmarks so tree rows cannot overlap its separator/header.
            ui.set_clip_rect(ui.clip_rect().intersect(ui.available_rect_before_wrap()));
            self.tree_ui(ui, colors);
        });
        if let Some((rect, response)) = drop {
            let hovering = self
                .native_drag_point
                .is_some_and(|point| rect.contains(point))
                || (response.contains_pointer() && egui::DragAndDrop::has_any_payload(ui.ctx()));
            if hovering {
                ui.painter().rect_filled(rect, 0.0, colors.recessed);
                ui.painter()
                    .rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, colors.accent));
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Drop folders to bookmark",
                    FontId::proportional(14.0),
                    colors.text_primary,
                );
            }
        }
    }
    fn bookmarks_ui(&mut self, ui: &mut Ui, colors: WorkbenchColors) {
        ui.add_space(4.0);
        ui.heading("Bookmarks");
        let mut open = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_source("files_bookmark_rows")
            .min_scrolled_height(0.0)
            .auto_shrink([false, false])
            .show_rows(ui, ROW_HEIGHT, self.bookmarks.entries.len(), |ui, rows| {
                for index in rows {
                    let entry = &self.bookmarks.entries[index];
                    let (rect, response) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::click(),
                    );
                    row_background(
                        ui,
                        rect,
                        entry.path.as_deref() == Some(self.path()),
                        response.hovered(),
                        colors,
                    );
                    Icon::Folder.paint(
                        ui.painter(),
                        Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(20.0, 20.0)),
                        colors.text_secondary,
                    );
                    paint_text(
                        ui,
                        rect,
                        32.0,
                        rect.width() - 32.0,
                        &entry.label,
                        colors.text_primary,
                    );
                    if response.clicked() {
                        open = entry.path.clone();
                    }
                    response.clone().on_hover_text(if entry.path.is_some() {
                        &entry.uri
                    } else {
                        "This remote bookmark needs a mounted local folder."
                    });
                    response.context_menu(|ui| {
                        if ui.button("Remove shortcut").clicked() {
                            remove = Some(entry.uri.clone());
                            ui.close_menu();
                        }
                    });
                }
            });
        if let Some(path) = open {
            self.navigate(path, Navigation::New);
        }
        if let Some(uri) = remove {
            self.bookmarks.remove(uri);
        }
    }
}
