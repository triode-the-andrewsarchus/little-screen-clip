use super::*;

impl NodeCanvasState {
    pub(super) fn render_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        action: &mut Option<CanvasAction>,
        palette: &AppPalette,
    ) {
        ui.horizontal_wrapped(|ui| {
            // Group 1: Add Node button (Prominent)
            let add_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Plus),
                "Add Node",
                hex_to_c32(&palette.colors.surface_panel),
                palette,
            );
            if ui.add(add_btn).clicked() {
                self.catalog_open = true;
                self.catalog_search.clear();
                self.spawn_node_pos = None;
            }

            ui.separator();

            // Group 2: Recipe Management
            let new_btn = crate::ui_ext::secondary_button_icon(
                Some(Icon::Text),
                "New Recipe",
                hex_to_c32(&palette.colors.surface_recessed),
                palette,
            );
            if ui
                .add(new_btn)
                .on_hover_text("Start a fresh recipe from the basic template")
                .clicked()
            {
                self.snapshot_undo();
                let template = make_basic_template_recipe();
                self.selected_recipe_name = template.name.clone();
                self.graph = template.graph;
                self.current_recipe_file = None;
                self.node_previews.clear();
                self.status_message = Some(("Created new recipe from template".to_string(), false));
            }

            ui.label(
                egui::RichText::new("Recipe:")
                    .color(hex_to_c32(&palette.colors.text_muted))
                    .size(BODY_FONT_SIZE),
            );

            let mut recipe_to_load = None;
            egui::ComboBox::from_id_source("recipe_combobox")
                .selected_text(
                    egui::RichText::new(&self.selected_recipe_name)
                        .color(hex_to_c32(&palette.harmonies.base))
                        .strong(),
                )
                .show_ui(ui, |ui| {
                    for r in &self.available_recipes {
                        let is_sel = r.name == self.selected_recipe_name;
                        if ui.selectable_label(is_sel, &r.name).clicked() {
                            recipe_to_load = Some(r.clone());
                        }
                    }
                });

            if let Some(r) = recipe_to_load {
                self.snapshot_undo();
                self.selected_recipe_name = r.name.clone();
                self.graph = r.graph.clone();
                let safe: String = r
                    .name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '_' || c == '-' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let p = get_recipes_dir().join(format!("{}.yaml", safe.to_lowercase()));
                self.current_recipe_file = if p.exists() { Some(p) } else { None };
                self.status_message = Some((format!("Loaded recipe '{}'", r.name), false));
            }

            // Direct Save button
            let save_btn = crate::ui_ext::secondary_button_icon(
                Some(Icon::Save),
                "Save",
                hex_to_c32(&palette.colors.surface_recessed),
                palette,
            );
            if ui
                .add(save_btn)
                .on_hover_text("Save changes to this recipe")
                .clicked()
            {
                if let Err(error) = self.save_script_drafts() {
                    self.status_message = Some((error, true));
                    return;
                }
                if self.current_recipe_file.is_some()
                    && self.selected_recipe_name != "Untitled Recipe"
                {
                    let _ = sync_recipe_script_headers(
                        &mut self.graph,
                        &self.selected_recipe_name,
                        false,
                    );
                    self.refresh_saved_script_editors();
                    let recipe = Recipe {
                        name: self.selected_recipe_name.clone(),
                        description: "Custom user recipe".to_string(),
                        input_type: little_clip_ops::pipeline::types::RecipeInputType::Any,
                        tags: vec!["custom".to_string()],
                        graph: self.graph.clone(),
                    };
                    if let Ok(path) = save_recipe(&recipe) {
                        self.current_recipe_file = Some(path);
                        self.reload_recipes();
                        self.status_message = Some((
                            format!("Saved recipe '{}'", self.selected_recipe_name),
                            false,
                        ));
                    }
                } else {
                    self.save_is_save_as = false;
                    self.save_name = if self.selected_recipe_name == "Untitled Recipe" {
                        String::new()
                    } else {
                        self.selected_recipe_name.clone()
                    };
                    self.save_dialog_open = true;
                }
            }

            // Save As button
            let save_as_btn = crate::ui_ext::secondary_button_icon(
                Some(Icon::Save),
                "Save As...",
                hex_to_c32(&palette.colors.surface_recessed),
                palette,
            );
            if ui
                .add(save_as_btn)
                .on_hover_text("Save copy under a new recipe name")
                .clicked()
            {
                if let Err(error) = self.save_script_drafts() {
                    self.status_message = Some((error, true));
                    return;
                }
                self.save_is_save_as = true;
                self.save_name = format!("{} Copy", self.selected_recipe_name);
                self.save_dialog_open = true;
            }

            ui.separator();

            // Group 3: Undo, Redo, Clear
            let undo_enabled = !self.undo_stack.is_empty();
            let undo_btn = crate::ui_ext::secondary_button_icon(
                Some(Icon::Undo),
                "Undo",
                hex_to_c32(&palette.colors.surface_recessed),
                palette,
            );
            if ui
                .add_enabled(undo_enabled, undo_btn)
                .on_hover_text("Undo last change (Ctrl+Z)")
                .clicked()
            {
                self.undo();
            }

            let redo_enabled = !self.redo_stack.is_empty();
            let redo_btn = crate::ui_ext::secondary_button_icon(
                Some(Icon::Redo),
                "Redo",
                hex_to_c32(&palette.colors.surface_recessed),
                palette,
            );
            if ui
                .add_enabled(redo_enabled, redo_btn)
                .on_hover_text("Redo change (Ctrl+Y or Ctrl+Shift+Z)")
                .clicked()
            {
                self.redo();
            }

            // Clear button
            let clear_btn = crate::ui_ext::secondary_button_icon(
                Some(Icon::Trash),
                "Clear",
                hex_to_c32(&palette.colors.surface_recessed),
                palette,
            );
            if ui
                .add(clear_btn)
                .on_hover_text("Clear all nodes from the canvas")
                .clicked()
            {
                self.snapshot_undo();
                self.graph.nodes.clear();
                self.graph.connections.clear();
                self.node_previews.clear();
                self.status_message = Some(("Canvas cleared".to_string(), false));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // RUN PIPELINE BUTTON (Themed hero accent)
                let run_btn = crate::ui_ext::action_button_icon(
                    Some(Icon::Play),
                    "RUN PIPELINE",
                    hex_to_c32(&palette.harmonies.base),
                    palette,
                )
                .min_size(Vec2::new(140.0, CONTROL_HEIGHT));

                if ui.add(run_btn).clicked() {
                    *action = self.run_pipeline_action();
                }
            });
        });

        // Keep status feedback in its own row so a long message never displaces
        // or paints over the Undo, Redo, and Clear controls at compact widths.
        if let Some((ref msg, is_err)) = self.status_message {
            let col = if is_err {
                hex_to_c32(&palette.diff_del)
            } else {
                hex_to_c32(&palette.diff_add)
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(msg)
                        .color(col)
                        .size(BODY_FONT_SIZE)
                        .strong(),
                );
            });
        }
    }

    pub(super) fn render_catalog_window(
        &mut self,
        ui: &mut egui::Ui,
        parent_rect: Rect,
        palette: &AppPalette,
    ) {
        let center = parent_rect.center();
        let modal_w = 480.0;
        let modal_h = 420.0;
        let modal_rect = Rect::from_center_size(center, Vec2::new(modal_w, modal_h));

        // Dark dim backdrop
        ui.painter().rect_filled(
            parent_rect,
            Rounding::ZERO,
            Color32::from_rgba_unmultiplied(0, 0, 0, 160),
        );

        egui::Area::new(Id::new("catalog_modal_area"))
            .fixed_pos(modal_rect.min)
            .show(ui.ctx(), |ui| {
                egui::Frame::window(&ui.style())
                    .fill(hex_to_c32(&palette.colors.surface_recessed))
                    .stroke(Stroke::new(1.5_f32, hex_to_c32(&palette.harmonies.base)))
                    .rounding(crate::ui_ext::CARD_CORNER_RADIUS)
                    .inner_margin(egui::Margin::same(crate::ui_ext::PANEL_INSET))
                    .show(ui, |ui| {
                        ui.set_width(modal_w);
                        ui.set_height(modal_h);

                        ui.horizontal(|ui| {
                            ui.heading(
                                egui::RichText::new("Add Node to Canvas")
                                    .color(hex_to_c32(&palette.harmonies.base))
                                    .strong(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let close_btn = crate::ui_ext::danger_button_icon(
                                        Some(Icon::Close),
                                        "Close",
                                        hex_to_c32(&palette.colors.surface_recessed),
                                        palette,
                                    )
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        hex_to_c32(&palette.undo_border),
                                    ));
                                    if ui.add(close_btn).clicked() {
                                        self.catalog_open = false;
                                    }
                                },
                            );
                        });

                        ui.add_space(crate::ui_ext::SPACE_SM);

                        // Search box (autofocused)
                        let resp = ui.add_sized(
                            [ui.available_width(), CONTROL_HEIGHT],
                            egui::TextEdit::singleline(&mut self.catalog_search).hint_text(
                                "Search nodes or tags (e.g. caption, frame, normalize, python)...",
                            ),
                        );
                        resp.request_focus();

                        ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                        // Category Pills
                        ui.horizontal(|ui| {
                            let categories = [
                                "All", "Source", "Text", "Image", "Audio", "Files", "Script",
                                "Sink",
                            ];
                            for &cat in &categories {
                                let is_sel = self.catalog_category == cat;
                                if ui.selectable_label(is_sel, cat).clicked() {
                                    self.catalog_category = cat.to_string();
                                }
                            }
                        });

                        ui.add_space(crate::ui_ext::SPACE_SM);
                        ui.separator();

                        // Filtered Node List
                        let filtered =
                            search_templates(&self.catalog_search, Some(&self.catalog_category));

                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                if filtered.is_empty() {
                                    ui.label(
                                        egui::RichText::new("No matching nodes found.")
                                            .color(hex_to_c32(&palette.colors.text_muted))
                                            .italics(),
                                    );
                                } else {
                                    for template in filtered {
                                        let frame = egui::Frame::none()
                                            .fill(hex_to_c32(&palette.colors.surface_panel))
                                            .rounding(crate::ui_ext::CONTROL_CORNER_RADIUS)
                                            .inner_margin(egui::Margin::symmetric(
                                                crate::ui_ext::BUTTON_PADDING_X,
                                                crate::ui_ext::BUTTON_PADDING_Y,
                                            ));

                                        frame.show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(template.name)
                                                        .color(hex_to_c32(
                                                            &palette.colors.text_primary,
                                                        ))
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "[{}]",
                                                        template.category
                                                    ))
                                                    .color(hex_to_c32(&palette.colors.text_muted))
                                                    .size(BODY_FONT_SIZE),
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        let add_btn = crate::action_button(
                                                            "+ Add",
                                                            hex_to_c32(&palette.harmonies.base),
                                                            palette,
                                                        );
                                                        if ui.add(add_btn).clicked() {
                                                            let spawn_pos = self
                                                                .spawn_node_pos
                                                                .take()
                                                                .unwrap_or_else(|| {
                                                                    let center_world =
                                                                        screen_to_world(
                                                                            parent_rect,
                                                                            self.graph.pan,
                                                                            self.graph.zoom,
                                                                            parent_rect.center(),
                                                                        );
                                                                    [center_world.x, center_world.y]
                                                                });
                                                            self.snapshot_undo();
                                                            let mut new_node = instantiate_template(
                                                                template, spawn_pos,
                                                            );

                                                            // Generate script scaffolding for python / bash script nodes
                                                            if template.kind
                                                                == NodeKind::PythonScript
                                                            {
                                                                if let Ok(fname) =
                                                                    create_script_file_scaffold(
                                                                        &self.selected_recipe_name,
                                                                        true,
                                                                    )
                                                                {
                                                                    new_node.props.insert(
                                                                        "script_file".to_string(),
                                                                        fname,
                                                                    );
                                                                    new_node.props.remove("script");
                                                                }
                                                            } else if template.kind
                                                                == NodeKind::BashCommand
                                                            {
                                                                if let Ok(fname) =
                                                                    create_script_file_scaffold(
                                                                        &self.selected_recipe_name,
                                                                        false,
                                                                    )
                                                                {
                                                                    new_node.props.insert(
                                                                        "script_file".to_string(),
                                                                        fname,
                                                                    );
                                                                    new_node
                                                                        .props
                                                                        .remove("command");
                                                                }
                                                            }

                                                            self.graph.nodes.push(new_node);
                                                            self.catalog_open = false;
                                                        }
                                                    },
                                                );
                                            });
                                            ui.label(
                                                egui::RichText::new(template.description)
                                                    .color(hex_to_c32(
                                                        &palette.colors.text_secondary,
                                                    ))
                                                    .size(BODY_FONT_SIZE),
                                            );
                                        });
                                        ui.add_space(crate::ui_ext::SPACE_XS);
                                    }
                                }
                            });
                    });
            });
    }

    pub(super) fn render_save_window(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        self.render_save_window_in_dir(ui, palette, &get_recipes_dir());
    }

    pub(super) fn render_save_window_in_dir(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        recipes_dir: &Path,
    ) {
        let title = if self.save_is_save_as {
            "Save Recipe As..."
        } else {
            "Save Pipeline Recipe"
        };

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(340.0);
                ui.label("Enter recipe name:");
                let resp = ui.add_sized(
                    [ui.available_width(), CONTROL_HEIGHT],
                    egui::TextEdit::singleline(&mut self.save_name),
                );
                resp.request_focus();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let save_btn =
                        crate::action_button("Save", hex_to_c32(&palette.harmonies.base), palette);
                    if ui
                        .add_enabled(!self.save_name.trim().is_empty(), save_btn)
                        .clicked()
                    {
                        if let Err(error) = self.save_script_drafts() {
                            self.status_message = Some((error, true));
                            return;
                        }
                        let new_name = self.save_name.trim().to_string();
                        let duplicate_scripts = self.save_is_save_as;
                        let _ = sync_recipe_script_headers_in_dir(
                            &mut self.graph,
                            &new_name,
                            duplicate_scripts,
                            recipes_dir,
                        );

                        self.refresh_saved_script_editors();
                        let recipe = Recipe {
                            name: new_name.clone(),
                            description: "Custom user recipe".to_string(),
                            input_type: little_clip_ops::pipeline::types::RecipeInputType::Any,
                            tags: vec!["custom".to_string()],
                            graph: self.graph.clone(),
                        };

                        match save_recipe_in_dir(&recipe, recipes_dir) {
                            Ok(path) => {
                                self.status_message =
                                    Some((format!("Saved recipe '{}'", recipe.name), false));
                                self.current_recipe_file = Some(path);
                                self.selected_recipe_name = recipe.name;
                                self.reload_recipes_in_dir(recipes_dir);
                            }
                            Err(e) => {
                                self.status_message =
                                    Some((format!("Error saving recipe: {}", e), true));
                            }
                        }
                        self.save_dialog_open = false;
                    }
                    let cancel_btn = crate::secondary_button(
                        "Cancel",
                        hex_to_c32(&palette.colors.surface_panel),
                        palette,
                    );
                    if ui.add(cancel_btn).clicked() {
                        self.save_dialog_open = false;
                    }
                });
            });
    }
}
