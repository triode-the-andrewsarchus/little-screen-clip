//! Static Home references; every target handoff uses ordinary browser guards.
use super::*;
use crate::file_manager::home::Pin;

impl FileManager {
    pub(super) fn show_home(&mut self) {
        self.archives.cancel();
        self.pending = None;
        self.watch = None;
        self.containers.retire();
        self.stop_previews();
        if self.search.open {
            self.close_search();
        }
        self.home_surface.open = true;
        self.home_surface.reload();
    }
    pub(super) fn home_ui(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let back = ui
            .horizontal(|ui| {
                let back = ui
                    .add(clip_ui::buttons::ButtonRole::Secondary.button("Back to folder", palette))
                    .clicked();
                ui.heading("Home");
                back
            })
            .inner
            || key(ui.ctx(), Modifiers::NONE, Key::Escape)
            || key(ui.ctx(), Modifiers::ALT, Key::ArrowLeft);
        if back {
            self.home_surface.open = false;
            self.focus_list = true;
            return;
        }
        ui.separator();
        ui.label("Keep files and folders you want to return to.");
        self.home_pin_actions(ui, palette);
        if let Some(error) = &self.home_surface.error {
            ui.colored_label(
                WorkbenchColors::from_palette(palette).warning,
                format!("Home: {error}"),
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(WorkbenchColors::from_palette(palette).warning, error);
        }
        if self.home_surface.busy() {
            ui.spinner();
        }
        ui.separator();
        let pins = self.home_surface.pins().to_vec();
        if pins.is_empty() && !self.home_surface.busy() {
            ui.label("No pins yet. Choose files in a folder, then pin them here.");
        }
        let mut open = None;
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_source("file_manager_home")
            .show(ui, |ui| {
                for pin in pins {
                    let (opened, removed) = pin_row(ui, &pin, self.home_surface.ready(), palette);
                    if opened {
                        open = Some(pin.clone());
                    }
                    if removed {
                        remove = Some(pin.id);
                    }
                }
            });
        if let Some(id) = remove {
            self.home_surface.remove(id);
        }
        if let Some(pin) = open {
            self.open_home_pin(pin);
        }
    }

    fn home_pin_actions(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let selected = self.selected_paths();
        let can_pin = self.home_surface.ready()
            && self.directory.is_some()
            && self.archive_context().is_none()
            && !self.search.open
            && !self.filesystem_busy();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    can_pin,
                    clip_ui::buttons::ButtonRole::Secondary.button("Pin current folder", palette),
                )
                .clicked()
            {
                self.home_surface
                    .add(vec![Pin::new(self.path().into(), true)]);
            }
            if ui
                .add_enabled(
                    can_pin && !selected.is_empty(),
                    clip_ui::buttons::ButtonRole::Secondary.button("Pin selection", palette),
                )
                .clicked()
            {
                let pins = self
                    .displayed_entries()
                    .into_iter()
                    .filter(|entry| selected.contains(&entry.path))
                    .map(|entry| Pin::new(entry.path.clone(), entry.directory))
                    .collect();
                self.home_surface.add(pins);
            }
            if ui
                .add_enabled(
                    !self.home_surface.busy(),
                    clip_ui::buttons::ButtonRole::Secondary.button("Reload pins", palette),
                )
                .clicked()
            {
                self.home_surface.reload();
            }
        });
    }

    fn open_home_pin(&mut self, pin: Pin) {
        if pin.folder {
            self.navigate(pin.path, Navigation::New);
        } else if let Some(parent) = pin.path.parent() {
            let navigation = if parent == self.path() {
                Navigation::Refresh
            } else {
                Navigation::New
            };
            self.navigate(parent.into(), navigation);
            // Always read the parent before selecting/opening a file, so
            // shared studios receive that folder's real sibling ordering.
            self.reveal_target = Some(pin.path.clone());
            self.home_surface.opening = Some(pin.path);
        }
    }
}

fn pin_row(ui: &mut Ui, pin: &Pin, editable: bool, palette: &AppPalette) -> (bool, bool) {
    ui.push_id(pin.id, |ui| {
        let actions = ui
            .horizontal_wrapped(|ui| {
                let name = pin.path.file_name().map_or_else(
                    || display_path(&pin.path),
                    |name| name.to_string_lossy().into_owned(),
                );
                let icon = if pin.folder { Icon::Folder } else { Icon::Text };
                let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
                icon.paint(ui.painter(), rect.shrink(3.0), ui.visuals().text_color());
                let open = ui
                    .add(clip_ui::buttons::ButtonRole::Secondary.button(name, palette))
                    .clicked();
                let remove = ui
                    .add_enabled(
                        editable,
                        clip_ui::buttons::ButtonRole::Secondary.button("Unpin", palette),
                    )
                    .on_hover_text("Remove this reference; keep the original file")
                    .clicked();
                (open, remove)
            })
            .inner;
        ui.label(egui::RichText::new(display_path(&pin.path)).small().weak());
        ui.add_space(8.0);
        actions
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::filesystem;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn home_suspends_folder_shortcuts_and_resumes_selection_without_a_fake_path() {
        let root = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt"] {
            std::fs::write(root.path().join(name), name).unwrap();
        }
        let mut browser = FileManager::new(root.path().into());
        browser.commit(
            filesystem::read_directory(root.path(), &AtomicBool::new(false)).unwrap(),
            Navigation::New,
        );
        browser.select(root.path().join("a.txt"));
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        browser.show_home();
        let render = |browser: &mut FileManager, key: Key| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1100.0, 800.0))),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| browser.ui(ui, &AppPalette::default()));
                },
            )
        };
        for key in [Key::ArrowDown, Key::Delete, Key::Enter] {
            let _ = render(&mut browser, key);
        }
        assert!(browser.home_surface.open);
        assert_eq!(browser.path(), root.path());
        assert_eq!(browser.selected_paths(), [root.path().join("a.txt")]);
        assert!(browser.edits.dialog.is_none());
        assert!(browser.open_file.is_none());
        assert!(browser.drop_targets.is_empty());
        let _ = render(&mut browser, Key::Escape);
        assert!(!browser.home_surface.open);
        let _ = render(&mut browser, Key::ArrowDown);
        assert_eq!(browser.selected_paths(), [root.path().join("b.txt")]);
    }

    #[test]
    fn file_pin_reloads_parent_before_opening_and_missing_pin_cannot_open_previous_selection() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.txt");
        std::fs::write(&file, "fixture").unwrap();
        let directory = filesystem::read_directory(root.path(), &AtomicBool::new(false)).unwrap();
        let mut browser = FileManager::new(root.path().into());
        browser.commit(directory.clone(), Navigation::New);
        browser.select(file.clone());
        browser.show_home();
        browser.open_home_pin(Pin::new(file.clone(), false));
        assert!(browser.open_file.is_none());
        assert!(browser.pending.is_some());
        browser.commit(directory.clone(), Navigation::Refresh);
        assert_eq!(browser.open_file.take(), Some(file));
        browser.show_home();
        browser.open_home_pin(Pin::new(root.path().join("missing.txt"), false));
        browser.commit(directory, Navigation::Refresh);
        assert!(browser.open_file.is_none());
        assert!(browser.error.as_ref().unwrap().contains("unavailable"));
    }
}
