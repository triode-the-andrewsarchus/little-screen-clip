//! Personal Home presentation; target handoffs retain ordinary browser guards.
use super::*;
use crate::file_manager::home::{Board, Content, Pin};
use little_clip_core::diagram::drawing::Bounds;
use uuid::Uuid;
mod cards;
mod projects;
use cards::{Action, Renderer};

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
        let back = self.home_header(ui, palette);
        if (back || self.home_keys(ui)) && self.home_surface.session.allow_leave() {
            self.home_surface.session.commit_labels();
            self.home_surface.open = false;
            self.home_surface.session.canvas.suspend();
            self.focus_list = true;
            return;
        }
        self.home_recovery(ui, palette);
        self.home_project_form(ui, palette);
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.home_surface.session.arrange {
            self.home_inspector(ui, palette);
        }
        ui.separator();
        let actions = self.home_canvas(ui, palette);
        self.home_actions(actions);
    }
    fn home_header(&mut self, ui: &mut Ui, palette: &AppPalette) -> bool {
        let editing = self.home_surface.editing_allowed();
        ui.horizontal_wrapped(|ui| {
            let back = ui
                .add(clip_ui::buttons::ButtonRole::Secondary.button("Back to folder", palette))
                .clicked();
            if self.home_surface.session.arrange {
                let session = &mut self.home_surface.session;
                let field = ui.add_enabled(
                    editing,
                    egui::TextEdit::singleline(&mut session.title_draft)
                        .id_source("home_title")
                        .desired_width(150.0)
                        .char_limit(64),
                );
                if field.lost_focus() && !session.title_draft.trim().is_empty() {
                    let title = session.title_draft.trim().to_owned();
                    session.edit(|board| board.title = title);
                }
            } else {
                ui.heading(&self.home_surface.session.board.title);
            }
            ui.add_enabled_ui(editing, |ui| {
                if ui
                    .selectable_label(self.home_surface.session.arrange, "Arrange")
                    .on_hover_text("Draw, write and move your shortcuts")
                    .clicked()
                {
                    let session = &mut self.home_surface.session;
                    if session.allow_leave() {
                        session.commit_labels();
                        session.arrange = !session.arrange;
                        session.selected = None;
                        session.canvas.cancel();
                    }
                }
                self.home_add_menu(ui);
            });
            let session = &mut self.home_surface.session;
            if ui
                .add_enabled(
                    editing && session.can_undo() && !session.canvas.busy(),
                    clip_ui::buttons::ButtonRole::Secondary.button("Undo", palette),
                )
                .clicked()
            {
                session.undo();
            }
            if ui
                .add_enabled(
                    editing && session.can_redo() && !session.canvas.busy(),
                    clip_ui::buttons::ButtonRole::Secondary.button("Redo", palette),
                )
                .clicked()
            {
                session.redo();
            }
            if ui
                .button("Fit")
                .on_hover_text("Show the whole desk")
                .clicked()
            {
                session.canvas.fit();
            }
            self.home_projects_menu(ui);
            if self.home_surface.busy() {
                ui.spinner();
            }
            back
        })
        .inner
    }
    fn home_add_menu(&mut self, ui: &mut Ui) {
        let selected = self.selected_paths();
        let can_pin = self.home_surface.ready()
            && self.directory.is_some()
            && self.archive_context().is_none()
            && !self.search.open
            && !self.filesystem_busy();
        ui.menu_button("Add", |ui| {
            self.home_project_add_actions(ui, can_pin);
            if ui
                .add_enabled(can_pin, egui::Button::new("Pin current folder"))
                .clicked()
            {
                self.home_surface
                    .add(vec![Pin::new(self.path().into(), true)]);
                ui.close_menu();
            }
            if ui
                .add_enabled(
                    can_pin && !selected.is_empty(),
                    egui::Button::new("Pin selection"),
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
                ui.close_menu();
            }
            if ui.button("Named section").clicked() {
                let session = &mut self.home_surface.session;
                session.arrange = true;
                session.section();
                ui.close_menu();
            }
            ui.menu_button("Existing shortcuts", |ui| {
                let mut add = None;
                for pin in self.home_surface.pins() {
                    if ui.button(cards::pin_name(pin)).clicked() {
                        add = Some(pin.id);
                    }
                }
                if let Some(pin) = add {
                    let session = &mut self.home_surface.session;
                    let [x, y] = session.spawn;
                    session.edit(|board| {
                        board.cards.push(crate::file_manager::home::Card {
                            id: Uuid::new_v4(),
                            rect: [x - 130.0, y - 55.0, 260.0, 110.0],
                            content: Content::Shortcut {
                                pin,
                                label: String::new(),
                            },
                        })
                    });
                    ui.close_menu();
                }
            });
        });
    }
    fn home_recovery(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let session = &mut self.home_surface.session;
        if session.leave_notice && session.canvas.has_unapplied_text() {
            ui.label("Unapplied drawing text. Apply it in the drawing panel, or discard it before leaving.");
            ui.horizontal(|ui| {
                if ui
                    .add(clip_ui::buttons::ButtonRole::Secondary.button("Keep editing", palette))
                    .clicked()
                {
                    session.leave_notice = false;
                }
                if ui
                    .add(
                        clip_ui::buttons::ButtonRole::Destructive
                            .button("Discard unapplied text", palette),
                    )
                    .clicked()
                {
                    session.canvas.discard_text();
                    session.leave_notice = false;
                }
            });
        }
        if let Some(error) = self.home_surface.error.clone() {
            ui.colored_label(ui.visuals().error_fg_color, format!("Home: {error}"));
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(clip_ui::buttons::ButtonRole::Primary.button(
                        if self.home_surface.session.dirty() {
                            "Retry save"
                        } else {
                            "Retry load"
                        },
                        palette,
                    ))
                    .clicked()
                {
                    self.home_surface.retry_board();
                }
                if ui
                    .add(
                        clip_ui::buttons::ButtonRole::Destructive
                            .button("Reload and discard unsaved Home edits", palette),
                    )
                    .clicked()
                {
                    self.home_surface.discard_board();
                }
            });
        }
        if let Some(error) = self.home_surface.session.canvas.error() {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
    fn home_inspector(&mut self, ui: &mut Ui, palette: &AppPalette) {
        let editable = self.home_surface.editing_allowed();
        let (pins, session) = self.home_surface.canvas_parts();
        ui.add_enabled_ui(editable, |ui| {
            ui.horizontal_wrapped(|ui| {
                if session.selected.is_some() {
                    ui.label("Label");
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut session.label_draft)
                            .id_source(("home_card_label", session.selected))
                            .desired_width(180.0)
                            .char_limit(64),
                    );
                    if field.lost_focus() {
                        session.rename_selected();
                    }
                    if ui
                        .add(
                            clip_ui::buttons::ButtonRole::Destructive
                                .button("Remove from desk", palette),
                        )
                        .clicked()
                    {
                        session.remove_selected();
                    }
                    if let Some(card) = session
                        .board
                        .cards
                        .iter()
                        .find(|card| Some(card.id) == session.selected)
                        && let Content::Section { color, .. } = card.content
                    {
                        let mut next = color;
                        if ui.color_edit_button_srgb(&mut next).changed() {
                            let selected = session.selected;
                            session.edit(|board| {
                                if let Some(card) = board
                                    .cards
                                    .iter_mut()
                                    .find(|card| Some(card.id) == selected)
                                    && let Content::Section { color, .. } = &mut card.content
                                {
                                    *color = next;
                                }
                            });
                        }
                    }
                }
                if !session.canvas.selected().is_empty() {
                    ui.menu_button("Link drawing to…", |ui| {
                        let mut link = None;
                        for pin in pins {
                            if ui.button(cards::pin_name(pin)).clicked() {
                                link = Some(pin.id);
                            }
                        }
                        if let Some(pin) = link {
                            session.link(pin);
                            ui.close_menu();
                        }
                    });
                }
                ui.weak("Pencil P · text T · select V · wheel zoom · middle-drag pan");
            });
        });
    }
    fn home_keys(&mut self, ui: &Ui) -> bool {
        if popup_open(ui.ctx()) {
            return false;
        }
        let text = ui
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ui.ctx(), id).is_some());
        if text {
            return false;
        }
        let editing = self.home_surface.editing_allowed();
        let session = &mut self.home_surface.session;
        if key(ui.ctx(), Modifiers::NONE, Key::Escape) {
            if session.cancel_drag() || session.canvas.cancel() || session.selected.take().is_some()
            {
                return false;
            }
            if session.arrange {
                session.arrange = false;
                return false;
            }
            return true;
        }
        if key(ui.ctx(), Modifiers::ALT, Key::ArrowLeft) {
            return true;
        }
        if editing && session.arrange && !session.canvas.busy() {
            home_edit_keys(ui.ctx(), session);
        }
        false
    }
    fn home_canvas(&mut self, ui: &mut Ui, palette: &AppPalette) -> Vec<Action> {
        let enabled = self.home_surface.editing_allowed();
        let unpin = self.home_surface.ready();
        let (pins, session) = self.home_surface.canvas_parts();
        let mut actions = Vec::new();
        let excluded = exclusions(&session.board, session.selected, session.arrange);
        let extent = board_extent(&session.board);
        let mut spawn = session.spawn;
        let mut paper_pressed = false;
        let options = clip_studios::drawing_canvas::Options {
            editable: session.arrange,
            enabled,
            excluded: &excluded,
            extent: Some(extent),
        };
        let response = session.canvas.show(
            ui,
            &session.board.drawing,
            palette,
            options,
            |ui, viewport| {
                spawn = viewport.world(viewport.rect.center());
                paper_pressed = ui.input(|input| input.pointer.primary_pressed())
                    && ui
                        .input(|input| input.pointer.interact_pos())
                        .is_some_and(|point| {
                            viewport.rect.contains(point)
                                && !excluded
                                    .iter()
                                    .any(|bound| viewport.bounds(*bound).contains(point))
                        });
                if session.board.is_empty() && !session.arrange {
                    let center = viewport.rect.center();
                    ui.painter().text(
                        center - vec2(0.0, 18.0),
                        egui::Align2::CENTER_CENTER,
                        "Make this place yours",
                        FontId::proportional(26.0),
                        Color32::from_rgb(73, 91, 91),
                    );
                    ui.painter().text(
                        center + vec2(0.0, 20.0),
                        egui::Align2::CENTER_CENTER,
                        "Add shortcuts. Arrange a corner. Draw a landmark.",
                        FontId::proportional(16.0),
                        Color32::from_rgb(117, 126, 122),
                    );
                }
                let renderer = Renderer {
                    viewport,
                    pins,
                    arrange: session.arrange,
                    enabled,
                    selected: session.selected,
                    unpin,
                };
                for card in session
                    .board
                    .cards
                    .iter()
                    .filter(|card| matches!(card.content, Content::Section { .. }))
                {
                    renderer.card(ui, card, &mut actions);
                }
                for card in session
                    .board
                    .cards
                    .iter()
                    .filter(|card| !matches!(card.content, Content::Section { .. }))
                {
                    renderer.card(ui, card, &mut actions);
                }
            },
        );
        session.spawn = spawn;
        if enabled {
            session.retain_view();
        }
        if paper_pressed || !session.canvas.selected().is_empty() {
            session.selected = None;
        }
        if let Some(source) = response.changed {
            session.edit(|board| board.drawing = source);
        }
        if let Some(element) = response.activated
            && let Some(binding) = session
                .board
                .bindings
                .iter()
                .find(|binding| binding.element == element)
        {
            actions.push(Action::Open(binding.pin));
        }
        if !ui.input(|input| input.pointer.any_down()) {
            session.end_drag();
        }
        actions
    }
    fn home_actions(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Open(id) => {
                    if let Some(pin) = self
                        .home_surface
                        .pins()
                        .iter()
                        .find(|pin| pin.id == id)
                        .cloned()
                    {
                        self.open_home_pin(pin);
                    }
                }
                Action::Unpin(id) => self.home_surface.remove(id),
                Action::Select(id) => {
                    self.home_surface.session.canvas.cancel();
                    self.home_surface.session.select(id);
                }
                Action::Begin(id) => {
                    self.home_surface.session.canvas.cancel();
                    self.home_surface.session.begin_drag(id);
                }
                Action::Drag(id, delta, resize) => {
                    self.home_surface.session.drag(id, delta, resize)
                }
                Action::End => self.home_surface.session.end_drag(),
                Action::Remove(id) => {
                    self.home_surface.session.select(id);
                    self.home_surface.session.remove_selected();
                }
            }
        }
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
            self.reveal_target = Some(pin.path.clone());
            self.home_surface.opening = Some(pin.path);
        }
    }
}
fn home_edit_keys(ctx: &egui::Context, session: &mut crate::file_manager::home::Session) {
    // egui's logical matching permits extra Shift; consume the specific chord first.
    if key(ctx, Modifiers::CTRL | Modifiers::SHIFT, Key::Z) {
        session.redo();
    } else if key(ctx, Modifiers::CTRL, Key::Z) {
        session.undo();
    }
    if session.selected.is_some() && key(ctx, Modifiers::NONE, Key::Delete) {
        session.remove_selected();
    }
}

fn board_extent(board: &Board) -> Bounds {
    let mut min = [0.0_f64, 0.0];
    let mut max = [1000.0_f64, 700.0];
    for card in &board.cards {
        min[0] = min[0].min(card.rect[0]);
        min[1] = min[1].min(card.rect[1]);
        max[0] = max[0].max(card.rect[0] + card.rect[2]);
        max[1] = max[1].max(card.rect[1] + card.rect[3]);
    }
    Bounds {
        x: min[0],
        y: min[1],
        width: max[0] - min[0],
        height: max[1] - min[1],
        angle: 0.0,
    }
}
fn exclusions(board: &Board, selected: Option<Uuid>, arrange: bool) -> Vec<Bounds> {
    let mut bounds = Vec::new();
    for card in &board.cards {
        let mut bound = card.bounds();
        if matches!(card.content, Content::Section { .. }) {
            bound.height = 40.0;
            if arrange && selected == Some(card.id) {
                bounds.push(Bounds {
                    x: card.rect[0] + card.rect[2] - 16.0,
                    y: card.rect[1] + card.rect[3] - 16.0,
                    width: 16.0,
                    height: 16.0,
                    angle: 0.0,
                });
            }
        }
        bounds.push(bound);
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::filesystem;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn shifted_redo_is_not_consumed_as_plain_undo() {
        let mut session = crate::file_manager::home::Session::default();
        session.edit(|board| board.title = "My desk".into());
        session.undo();
        let ctx = egui::Context::default();
        for (modifiers, title) in [
            (Modifiers::CTRL | Modifiers::SHIFT, "My desk"),
            (Modifiers::CTRL, "Home"),
        ] {
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events: vec![egui::Event::Key {
                        key: Key::Z,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| home_edit_keys(ctx, &mut session),
            );
            assert_eq!(session.board.title, title);
        }
    }



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
    fn home_keyboard_mutations_obey_the_same_admission_as_disabled_controls() {
        let mut browser = FileManager::default();
        browser.home_surface.session.arrange = true;
        browser
            .home_surface
            .session
            .edit(|board| board.title = "Pending load draft".into());
        assert!(!browser.home_surface.editing_allowed());
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                modifiers: Modifiers::CTRL,
                events: vec![egui::Event::Key {
                    key: Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::CTRL,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    browser.home_keys(ui);
                });
            },
        );
        assert_eq!(
            browser.home_surface.session.board.title,
            "Pending load draft"
        );
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
