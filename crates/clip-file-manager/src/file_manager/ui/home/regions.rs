//! Inline region manager, explicit action picker and exact freehand hit testing.
use super::*;
use crate::file_manager::home::{
    Card,
    regions::{Draft, Interaction, Region, RegionAction},
};
use clip_studios::drawing_canvas::Viewport;

pub(super) enum Activation {
    Run(RegionAction),
    Choose,
}

impl FileManager {
    pub(super) fn home_region_panel(&mut self, ui: &mut Ui, palette: &AppPalette) {
        if self.home_surface.recipes.writing() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running saved recipe…");
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut interaction = std::mem::take(&mut self.home_surface.region_ui);
        if !interaction.chooser.is_empty() {
            self.home_region_chooser(ui, &mut interaction);
        }
        if interaction.open {
            self.home_region_list(ui, &mut interaction);
        }
        if interaction.leave_notice && interaction.draft.is_some() {
            ui.horizontal(|ui| {
                ui.label("This click region has not been saved.");
                if ui.button("Keep editing region").clicked() {
                    interaction.leave_notice = false;
                }
                if ui
                    .add_enabled(
                        !self.home_surface.busy(),
                        clip_ui::buttons::ButtonRole::Destructive
                            .button("Discard region draft", palette),
                    )
                    .clicked()
                {
                    interaction.cancel();
                }
            });
        }
        if let Some(draft) = &mut interaction.draft
            && self.home_region_draft(ui, palette, draft)
        {
            interaction.draft = None;
        }
        if let Some(error) = &interaction.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        self.home_surface.region_ui = interaction;
    }
    /// Returns true only for an explicit Cancel; accepted saves own completion.
    fn home_region_draft(&mut self, ui: &mut Ui, palette: &AppPalette, draft: &mut Draft) -> bool {
        if let Some(target) = draft
            .target
            .as_ref()
            .and_then(|old| self.home_surface.pins().iter().find(|pin| pin.id == old.id))
            && draft.path != target.path.to_string_lossy()
        {
            draft.path = target.path.to_string_lossy().into_owned();
            draft.folder = target.folder;
            draft.target = Some(target.clone());
        }
        let ready = self.home_surface.ready();
        let mut save = false;
        let mut cancel = false;
        ui.group(|ui| {
                ui.label("Click region · choose an action");
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut draft.region.label).desired_width(240.0).char_limit(64));
                    ui.checkbox(&mut draft.region.enabled, "Enabled");
                    if ui.button("Redraw outline").clicked() { draft.retrace = true; }
                    if draft.retrace { ui.weak("Hold right button and trace the replacement outline below."); }
                });
                self.home_region_targets(ui, draft);
                ui.horizontal(|ui| {
                    let valid = draft.valid();
                    save = ui.add_enabled(ready && valid,
                        clip_ui::buttons::ButtonRole::Primary.button("Save region", palette)).clicked();
                    cancel = ui.add_enabled(!self.home_surface.busy(),
                        clip_ui::buttons::ButtonRole::Secondary.button("Cancel region", palette)).clicked();
                });
                ui.weak("Actions run only when you click the saved region. Editing or previewing never runs them.");
            });
        if save {
            let mut region = draft.region.clone();
            if draft.recipe {
                region.action = Some(RegionAction::Recipe {
                    name: draft.recipe_name.clone(),
                    prompt: (!draft.prompt.is_empty()).then(|| draft.prompt.clone()),
                });
            }
            self.home_surface.save_region(
                region,
                if draft.recipe {
                    None
                } else {
                    draft.target.clone()
                },
                draft.original.clone(),
            );
        }
        cancel
    }
    fn home_region_targets(&mut self, ui: &mut Ui, draft: &mut Draft) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut draft.recipe, false, "Open file or folder");
            ui.selectable_value(&mut draft.recipe, true, "Run saved recipe");
        });
        if draft.recipe {
            self.home_region_recipe(ui, draft);
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.menu_button("Choose target", |ui| {
                if self.directory.is_some()
                    && self.archive_context().is_none()
                    && ui
                        .button(format!("Current folder · {}", display_path(self.path())))
                        .clicked()
                {
                    draft.choose(Pin::new(self.path().into(), true));
                    ui.close_menu();
                }
                ui.menu_button("Projects", |ui| {
                    for project in self.home_surface.projects.records() {
                        if ui
                            .button(format!(
                                "{} · {}",
                                project.title,
                                display_path(&project.folder)
                            ))
                            .clicked()
                        {
                            draft.choose(Pin::new(project.folder.clone(), true));
                            ui.close_menu();
                        }
                    }
                });
                for pin in self.home_surface.pins() {
                    if ui
                        .button(format!(
                            "{} · {}",
                            cards::pin_name(pin),
                            display_path(&pin.path)
                        ))
                        .clicked()
                    {
                        draft.choose(pin.clone());
                        ui.close_menu();
                    }
                }
            });
            let changed = ui.checkbox(&mut draft.folder, "Open as folder").changed();
            let edited = ui
                .add(
                    egui::TextEdit::singleline(&mut draft.path)
                        .desired_width(400.0)
                        .hint_text("Absolute file or folder path"),
                )
                .changed();
            if edited {
                draft.target = Some(Pin::new(PathBuf::from(&draft.path), draft.folder));
            } else if changed && let Some(pin) = &mut draft.target {
                pin.folder = draft.folder;
            }
        });
        if let Some(pin) = &draft.target {
            ui.label(format!(
                "{}: {}",
                if pin.folder {
                    "Open folder"
                } else {
                    "Open file"
                },
                display_path(&pin.path)
            ));
        }
    }
    fn home_region_recipe(&mut self, ui: &mut Ui, draft: &mut Draft) {
        let recipes = &mut self.home_surface.recipes;
        recipes.load(false);
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_source("home_region_recipe")
                .selected_text(if draft.recipe_name.is_empty() {
                    "Choose a saved recipe"
                } else {
                    &draft.recipe_name
                })
                .show_ui(ui, |ui| {
                    for name in &recipes.names {
                        ui.selectable_value(&mut draft.recipe_name, name.clone(), name);
                    }
                });
            if ui.button("Refresh recipes").clicked() {
                recipes.load(true);
            }
            if recipes.loading() {
                ui.spinner();
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
            }
        });
        if let Some(error) = &recipes.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.horizontal(|ui| {
            ui.label("Optional prompt");
            ui.add(
                egui::TextEdit::singleline(&mut draft.prompt)
                    .desired_width(350.0)
                    .char_limit(4096),
            );
        });
        ui.weak("Runs the saved recipe on the current clipboard hands. Only clicking the saved region starts it.");
    }
    pub(super) fn activate_home_region(&mut self, action: RegionAction) {
        match action {
            RegionAction::Open { pin } => {
                if let Some(pin) = self
                    .home_surface
                    .pins()
                    .iter()
                    .find(|p| p.id == pin)
                    .cloned()
                {
                    self.open_home_pin(pin);
                }
            }
            RegionAction::Recipe { name, prompt } => self.home_surface.recipes.run(name, prompt),
        }
    }
    fn home_region_chooser(&mut self, ui: &mut Ui, interaction: &mut Interaction) {
        let mut chosen = None;
        let mut cancel = false;
        ui.group(|ui| {
            ui.label("Overlapping click regions · choose an action");
            for id in &interaction.chooser {
                if let Some(region) = self
                    .home_surface
                    .session
                    .board
                    .regions
                    .iter()
                    .find(|region| region.id == *id)
                    && region.enabled
                    && let Some(action) = &region.action
                    && ui
                        .button(format!(
                            "{} · {}",
                            region.label,
                            action_label(action, self.home_surface.pins())
                        ))
                        .clicked()
                {
                    chosen = Some(action.clone());
                }
            }
            cancel = ui.button("Cancel choice").clicked();
        });
        if let Some(action) = chosen {
            interaction.chooser.clear();
            self.activate_home_region(action);
        } else if cancel {
            interaction.chooser.clear();
        }
    }
    fn home_region_list(&mut self, ui: &mut Ui, interaction: &mut Interaction) {
        let enabled = self.home_surface.editing_allowed() && interaction.draft.is_none();
        let regions = self.home_surface.session.board.regions.clone();
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Click regions");
                ui.weak("Hold the right button and trace a closed outline on the desk to add one.");
                if ui
                    .add_enabled(enabled, egui::Button::new("Add region"))
                    .clicked()
                {
                    interaction.adding = true;
                    interaction.open = false;
                }
                if ui.button("Hide regions").clicked() {
                    interaction.open = false;
                }
            });
            if regions.is_empty() {
                ui.weak("No click regions yet. Existing artwork stays untouched.");
            }
            egui::ScrollArea::vertical()
                .id_source("home_region_manager")
                .max_height(190.0)
                .show(ui, |ui| {
                    for (index, region) in regions.iter().enumerate().rev() {
                        ui.horizontal_wrapped(|ui| {
                            let mut active = region.enabled;
                            if ui
                                .add_enabled(
                                    enabled,
                                    egui::Checkbox::new(&mut active, &region.label),
                                )
                                .changed()
                            {
                                self.home_surface
                                    .session
                                    .edit(|board| board.regions[index].enabled = active);
                            }
                            if ui
                                .add_enabled(enabled, egui::Button::new("Edit action"))
                                .clicked()
                            {
                                interaction.draft = Some(Draft::from_region(
                                    region.clone(),
                                    self.home_surface.pins(),
                                    Some(region.clone()),
                                ));
                            }
                            if ui
                                .add_enabled(
                                    enabled && index + 1 < regions.len(),
                                    egui::Button::new("Raise"),
                                )
                                .clicked()
                            {
                                self.home_surface
                                    .session
                                    .edit(|board| board.regions.swap(index, index + 1));
                            }
                            if ui
                                .add_enabled(enabled && index > 0, egui::Button::new("Lower"))
                                .clicked()
                            {
                                self.home_surface
                                    .session
                                    .edit(|board| board.regions.swap(index, index - 1));
                            }
                            if ui
                                .add_enabled(enabled, egui::Button::new("Delete region"))
                                .clicked()
                            {
                                self.home_surface
                                    .session
                                    .edit(|board| board.regions.retain(|old| old.id != region.id));
                            }
                            if let Some(action) = &region.action {
                                ui.weak(action_label(action, self.home_surface.pins()));
                            } else {
                                ui.weak("No target · choose an action");
                            }
                        });
                    }
                });
        });
    }
}
impl Interaction {
    pub(super) fn surface(
        &mut self,
        ui: &mut Ui,
        viewport: &Viewport<'_>,
        board: &Board,
        enabled: bool,
        arrange: bool,
    ) -> Option<Activation> {
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let blocked = pointer.is_some_and(|point| {
            board
                .cards
                .iter()
                .any(|card| live_control(card, viewport, point))
        });
        let popup = ui.memory(|memory| memory.any_popup_open());
        if self.tracing
            || (enabled
                && !popup
                && !blocked
                && self.draft.as_ref().is_none_or(|draft| draft.retrace))
        {
            self.trace(ui, viewport, pointer);
        }
        if self.adding {
            ui.painter().text(
                viewport.rect.left_top() + vec2(16.0, 16.0),
                egui::Align2::LEFT_TOP,
                "Hold right button and trace a click region · Esc to cancel",
                FontId::proportional(15.0),
                ui.visuals().text_color(),
            );
        }
        let hover = pointer
            .filter(|p| viewport.rect.contains(*p))
            .map(|p| viewport.world(p));
        let hits: Vec<_> = board
            .regions
            .iter()
            .rev()
            .filter(|region| {
                region.enabled
                    && region.action.is_some()
                    && hover.is_some_and(|point| region.contains(point))
            })
            .collect();
        self.paint(ui, viewport, board, &hits, arrange);
        self.activate(ui, pointer, &hits, !enabled || blocked || popup || arrange)
    }
    fn paint(
        &self,
        ui: &Ui,
        viewport: &Viewport<'_>,
        board: &Board,
        hits: &[&Region],
        arrange: bool,
    ) {
        for region in &board.regions {
            if self.open || arrange || hits.iter().any(|hit| hit.id == region.id) {
                outline(ui, viewport, &region.points, region.enabled);
            }
        }
        if !self.stroke.is_empty() {
            outline(ui, viewport, &self.stroke, true);
        }
        if let Some(draft) = &self.draft {
            outline(ui, viewport, &draft.region.points, true);
        }
    }
    fn activate(
        &mut self,
        ui: &Ui,
        pointer: Option<egui::Pos2>,
        hits: &[&Region],
        inhibited: bool,
    ) -> Option<Activation> {
        if inhibited || self.open || self.drafting() {
            return None;
        }
        let top = hits.first()?;
        let pointer = pointer?;
        let response = ui
            .interact(
                Rect::from_center_size(pointer, vec2(2.0, 2.0)),
                Id::new(("home_region_hit", top.id)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(if hits.len() > 1 {
                "Multiple click regions · click to choose"
            } else {
                &top.label
            });
        if response.clicked() {
            if hits.len() == 1 {
                return top.action.clone().map(Activation::Run);
            }
            self.chooser = hits.iter().map(|region| region.id).collect();
            return Some(Activation::Choose);
        }
        None
    }
    fn trace(&mut self, ui: &Ui, viewport: &Viewport<'_>, pointer: Option<egui::Pos2>) {
        let Some(pointer) = pointer else {
            if !ui.input(|i| i.pointer.secondary_down()) {
                self.tracing = false;
                self.stroke.clear();
            }
            return;
        };
        let (pressed, down, released) = ui.input(|i| {
            (
                i.pointer.secondary_pressed(),
                i.pointer.secondary_down(),
                i.pointer.secondary_released(),
            )
        });
        if pressed && viewport.rect.contains(pointer) {
            self.stroke = vec![viewport.world(pointer)];
            self.tracing = true;
            self.error = None;
        }
        if self.tracing && down && viewport.rect.contains(pointer) {
            let point = viewport.world(pointer);
            let separated = self
                .stroke
                .last()
                .is_none_or(|last| viewport.screen(*last).distance(pointer) >= 3.0);
            if separated {
                if self.stroke.len() < 512 {
                    self.stroke.push(point);
                } else {
                    self.error = Some(
                        "Outline exceeds 512 points. Try a shorter outline or zoom out first."
                            .into(),
                    );
                }
            }
        }
        if self.tracing && released {
            self.tracing = false;
            let points = std::mem::take(&mut self.stroke);
            let region = Region {
                id: Uuid::new_v4(),
                label: "New click region".into(),
                points,
                action: None,
                enabled: true,
            };
            if self.error.is_none()
                && region.valid()
                && region.points.iter().any(|p| {
                    viewport
                        .screen(*p)
                        .distance(viewport.screen(region.points[0]))
                        > 10.0
                })
            {
                self.adding = false;
                if let Some(draft) = &mut self.draft {
                    draft.region.points = region.points;
                    draft.retrace = false;
                } else {
                    self.draft = Some(Draft::from_region(region, &[], None));
                }
            }
        }
    }
}
fn action_label(action: &RegionAction, pins: &[Pin]) -> String {
    match action {
        RegionAction::Open { pin } => pins.iter().find(|p| p.id == *pin).map_or_else(
            || "Missing file reference".into(),
            |p| display_path(&p.path),
        ),
        RegionAction::Recipe { name, .. } => format!("Run recipe · {name}"),
    }
}
fn live_control(card: &Card, viewport: &Viewport<'_>, point: egui::Pos2) -> bool {
    matches!(
        card.content,
        Content::Project { .. }
            | Content::Projects { .. }
            | Content::Shortcut { .. }
            | Content::Folder { .. }
    ) && viewport.bounds(card.bounds()).contains(point)
}
fn outline(ui: &Ui, viewport: &Viewport<'_>, points: &[[f64; 2]], enabled: bool) {
    let points: Vec<_> = points.iter().map(|point| viewport.screen(*point)).collect();
    ui.painter().add(egui::Shape::closed_line(
        points,
        egui::Stroke::new(
            1.5_f32,
            if enabled {
                ui.visuals().hyperlink_color
            } else {
                ui.visuals().weak_text_color()
            },
        ),
    ));
}
