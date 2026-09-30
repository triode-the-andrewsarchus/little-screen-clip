//! Visual node graph canvas editor for custom pipelines and recipes.
//!
//! Provides interactive drag-and-drop node graph design, wire connections, parameter tweaking,
//! recipe loading/saving, and live graph execution.

use crate::icons::Icon;
use crate::ui_ext::{BODY_FONT_SIZE, CONTROL_HEIGHT};
use crate::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui::{self, Color32, Id, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::config::AppConfig;
use little_clip_core::palette::AppPalette;
use little_clip_ops::pipeline::catalog::{instantiate_template, search_templates};
use little_clip_ops::pipeline::recipes::{
    create_script_file_scaffold, get_recipes_dir, load_all_recipes, load_recipes_in_dir,
    make_basic_template_recipe, make_pad_and_caption_recipe, save_recipe, save_recipe_in_dir,
    sync_recipe_script_headers, sync_recipe_script_headers_in_dir,
};
use little_clip_ops::pipeline::types::{
    Connection, NodeGraph, NodeInstance, NodeKind, PortType, Recipe,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Actions emitted by the canvas UI back to the main workbench loop.
pub enum CanvasAction {
    /// Request execution of the current graph by the daemon.
    RunPipeline { yaml: String },
}

/// Interactive state for the visual node pipeline graph editor canvas.
pub struct NodeCanvasState {
    pub graph: NodeGraph,
    pub selected_node: Option<Uuid>,
    pub dragging_from: Option<(Uuid, String, bool)>, // (node_id, pin_name, is_output)
    pub node_previews: HashMap<Uuid, String>,
    pub catalog_open: bool,
    pub catalog_search: String,
    pub catalog_category: String,
    pub spawn_node_pos: Option<[f32; 2]>,
    pub save_dialog_open: bool,
    pub save_name: String,
    pub save_is_save_as: bool,
    pub current_recipe_file: Option<PathBuf>,
    pub undo_stack: Vec<NodeGraph>,
    pub redo_stack: Vec<NodeGraph>,
    pub config: AppConfig,
    pub status_message: Option<(String, bool)>, // (text, is_error)
    pub available_recipes: Vec<Recipe>,
    pub selected_recipe_name: String,
    pub last_run_duration_ms: Option<u128>,
}

impl NodeCanvasState {
    /// Creates a default canvas state initialized with default configuration.
    pub fn new() -> Self {
        Self::with_config(AppConfig::load())
    }

    /// Creates a canvas state with customized application configuration.
    pub fn with_config(config: AppConfig) -> Self {
        let recipes = load_all_recipes();
        let default_recipe = recipes
            .iter()
            .find(|r| r.name.contains("Pad and Caption"))
            .cloned()
            .unwrap_or_else(make_pad_and_caption_recipe);

        let sel_name = default_recipe.name.clone();
        let graph = default_recipe.graph;

        let safe_name: String = sel_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = get_recipes_dir().join(format!("{}.yaml", safe_name.to_lowercase()));
        let current_recipe_file = if path.exists() { Some(path) } else { None };

        Self {
            graph,
            selected_node: None,
            dragging_from: None,
            node_previews: HashMap::new(),
            catalog_open: false,
            catalog_search: String::new(),
            catalog_category: "All".to_string(),
            spawn_node_pos: None,
            save_dialog_open: false,
            save_name: String::new(),
            save_is_save_as: false,
            current_recipe_file,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            config,
            status_message: None,
            available_recipes: recipes,
            selected_recipe_name: sel_name,
            last_run_duration_ms: None,
        }
    }

    pub fn snapshot_undo(&mut self) {
        if self.undo_stack.len() >= 50 {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.graph.clone());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.graph.clone());
            self.graph = prev;
            self.status_message = Some(("↶ Undo".to_string(), false));
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.graph.clone());
            self.graph = next;
            self.status_message = Some(("↷ Redo".to_string(), false));
        }
    }

    pub fn reload_recipes(&mut self) {
        self.available_recipes = load_all_recipes();
    }

    fn reload_recipes_in_dir(&mut self, dir: &Path) {
        self.available_recipes = load_recipes_in_dir(dir);
    }

    fn run_pipeline_action(&mut self) -> Option<CanvasAction> {
        match serde_yaml::to_string(&self.graph) {
            Ok(yaml) => {
                self.status_message = Some(("Running pipeline…".to_owned(), false));
                Some(CanvasAction::RunPipeline { yaml })
            }
            Err(error) => {
                self.status_message = Some((format!("Error: {}", error), true));
                None
            }
        }
    }

    pub fn apply_pipeline_response(&mut self, response: little_clip_core::ipc::IpcResponse) {
        match response {
            little_clip_core::ipc::IpcResponse::PipelineExecuted {
                node_previews,
                output_title,
                duration_ms,
            } => {
                self.node_previews = node_previews;
                self.last_run_duration_ms = Some(duration_ms);
                let output = output_title.as_deref().unwrap_or("Done");
                self.status_message = Some((
                    format!("Executed in {}ms. Output: {}", duration_ms, output),
                    false,
                ));
            }
            little_clip_core::ipc::IpcResponse::Error { error } => {
                self.status_message = Some((format!("Error: {}", error), true));
            }
            _ => {
                self.status_message =
                    Some(("Error: unexpected pipeline response".to_owned(), true));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> Option<CanvasAction> {
        let mut action = None;

        // Undo / Redo & Node Delete shortcuts (when not typing in a text field)
        let wants_kb = ui.ctx().wants_keyboard_input();
        if !wants_kb {
            let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
            if cmd && ui.input(|i| i.key_pressed(egui::Key::Z)) {
                if shift {
                    self.redo();
                } else {
                    self.undo();
                }
            } else if cmd && ui.input(|i| i.key_pressed(egui::Key::Y)) {
                self.redo();
            } else if ui
                .input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            {
                if let Some(sel_id) = self.selected_node {
                    self.snapshot_undo();
                    self.graph.nodes.retain(|n| n.id != sel_id);
                    self.graph
                        .connections
                        .retain(|c| c.from_node != sel_id && c.to_node != sel_id);
                    self.selected_node = None;
                    self.status_message = Some(("Deleted node".to_string(), false));
                }
            }
        }

        // Top Toolbar
        self.render_toolbar(ui, &mut action, palette);

        ui.separator();

        // Main Canvas Interactive Area
        let (canvas_rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        // Canvas Pan & Zoom
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.graph.pan[0] += response.drag_delta().x;
            self.graph.pan[1] += response.drag_delta().y;
        }

        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_delta.abs() > 0.1 && response.hovered() {
            let steps = (scroll_delta / 50.0).clamp(-4.0, 4.0);
            let zoom_factor = (1.055f32).powf(steps);
            self.graph.zoom = (self.graph.zoom * zoom_factor).clamp(0.4, 2.2);
        }

        // Clip painter to canvas bounds
        let painter = ui.painter_at(canvas_rect);

        // 1. Draw Background Grid
        render_grid(
            &painter,
            canvas_rect,
            self.graph.pan,
            self.graph.zoom,
            palette,
        );

        // 2. Compute Node Screen Rectangles
        let mut node_screen_rects: HashMap<Uuid, Rect> = HashMap::new();
        for node in &self.graph.nodes {
            let screen_pos = world_to_screen(
                canvas_rect,
                self.graph.pan,
                self.graph.zoom,
                Pos2::new(node.pos[0], node.pos[1]),
            );
            let width = 250.0 * self.graph.zoom;
            let height = estimate_node_height(node) * self.graph.zoom;
            node_screen_rects.insert(
                node.id,
                Rect::from_min_size(screen_pos, Vec2::new(width, height)),
            );
        }

        // 3. Draw Existing Connection Cables
        for conn in &self.graph.connections {
            if let (Some(&from_rect), Some(&to_rect)) = (
                node_screen_rects.get(&conn.from_node),
                node_screen_rects.get(&conn.to_node),
            ) {
                let from_pos = get_pin_pos(
                    from_rect,
                    &self.graph.nodes,
                    conn.from_node,
                    &conn.from_pin,
                    true,
                    self.graph.zoom,
                );
                let to_pos = get_pin_pos(
                    to_rect,
                    &self.graph.nodes,
                    conn.to_node,
                    &conn.to_pin,
                    false,
                    self.graph.zoom,
                );
                let cable_color = hex_to_c32(&palette.harmonies.base);
                draw_bezier_cable(&painter, from_pos, to_pos, cable_color, self.graph.zoom);
            }
        }

        // 4. Render Node Cards & Process Interactions
        let mut node_to_delete = None;
        let mut connection_to_add = None;
        let mut socket_to_disconnect = None;
        let mut script_to_open = None;
        let mut node_drag_started = false;

        let zoom = self.graph.zoom;
        for node in &mut self.graph.nodes {
            let node_id = node.id;
            let screen_rect = *node_screen_rects.get(&node_id).unwrap();

            // Check if node is dragged
            let node_resp = ui.interact(
                screen_rect,
                ui.make_persistent_id(node_id),
                egui::Sense::click_and_drag(),
            );
            if node_resp.drag_started() {
                node_drag_started = true;
            }
            if node_resp.dragged_by(egui::PointerButton::Primary) && self.dragging_from.is_none() {
                node.pos[0] += node_resp.drag_delta().x / zoom;
                node.pos[1] += node_resp.drag_delta().y / zoom;
                self.selected_node = Some(node_id);
            }
            if node_resp.clicked() {
                self.selected_node = Some(node_id);
            }
            if matches!(node.kind, NodeKind::PythonScript | NodeKind::BashCommand)
                && node_resp.double_clicked()
            {
                let script_file = node.props.get("script_file").cloned().unwrap_or_else(|| {
                    let is_py = matches!(node.kind, NodeKind::PythonScript);
                    let ext = if is_py { "py" } else { "sh" };
                    format!("script_{}.{}", &node.id.to_string()[..8], ext)
                });
                script_to_open = Some(script_file);
            }

            // Render Card Frame
            let is_selected = self.selected_node == Some(node_id);
            render_node_card(
                zoom,
                &self.node_previews,
                &mut self.dragging_from,
                &mut socket_to_disconnect,
                &mut script_to_open,
                ui,
                &painter,
                node,
                screen_rect,
                is_selected,
                &mut node_to_delete,
                palette,
            );
        }

        // 5. Active Dragging Cable, Magnetic Snapping & Visual Feedback (Drawn ON TOP of Node Cards)
        // Cancel dragging if Escape is pressed
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.dragging_from = None;
        }

        let mut snapped_candidate: Option<SnappedTargetInfo> = None;
        if let Some((from_node, ref from_pin, is_out)) = self.dragging_from {
            // Request continuous repaint while dragging so animations run at smooth 60 FPS
            ui.ctx().request_repaint();

            let from_node_inst = self.graph.nodes.iter().find(|n| n.id == from_node);
            let from_port_type = from_node_inst.and_then(|n| {
                let pins = if is_out { &n.outputs } else { &n.inputs };
                pins.iter()
                    .find(|p| p.name.eq_ignore_ascii_case(from_pin))
                    .map(|p| p.port_type)
            });

            let mut min_dist = (32.0 * zoom).clamp(22.0, 48.0);
            if let Some(mouse_pos) = ui.input(|i| i.pointer.hover_pos()) {
                for (to_node, &to_rect) in &node_screen_rects {
                    if *to_node != from_node {
                        if let Some(node) = self.graph.nodes.iter().find(|n| n.id == *to_node) {
                            let target_pins = if is_out { &node.inputs } else { &node.outputs };
                            for (p_idx, target_pin) in target_pins.iter().enumerate() {
                                let pin_pos = get_pin_pos_by_idx(
                                    to_rect,
                                    target_pins.len(),
                                    p_idx,
                                    !is_out,
                                    zoom,
                                );
                                let dist = mouse_pos.distance(pin_pos);
                                if dist < min_dist {
                                    min_dist = dist;
                                    let is_compat = match from_port_type {
                                        Some(fpt) => fpt.is_compatible_with(&target_pin.port_type),
                                        None => true,
                                    };
                                    snapped_candidate = Some(SnappedTargetInfo {
                                        node_id: *to_node,
                                        node_title: node.title.clone(),
                                        pin_name: target_pin.name.clone(),
                                        port_type: target_pin.port_type,
                                        pin_pos,
                                        is_compatible: is_compat,
                                        from_port_type,
                                    });
                                }
                            }
                        }
                    }
                }

                if let Some(&node_rect) = node_screen_rects.get(&from_node) {
                    let source_pin_pos = get_pin_pos(
                        node_rect,
                        &self.graph.nodes,
                        from_node,
                        from_pin,
                        is_out,
                        zoom,
                    );
                    let time = ui.input(|i| i.time);
                    draw_active_cable(
                        &painter,
                        source_pin_pos,
                        mouse_pos,
                        is_out,
                        snapped_candidate.as_ref(),
                        time,
                        zoom,
                        palette,
                    );
                }
            }
        }

        // 6. Handle Pin Drop Connection
        if ui.input(|i| i.pointer.any_released()) {
            if let Some((from_node, from_pin, is_out)) = self.dragging_from.take() {
                if let Some(target) = snapped_candidate {
                    if is_out {
                        connection_to_add = Some(Connection {
                            from_node,
                            from_pin,
                            to_node: target.node_id,
                            to_pin: target.pin_name,
                        });
                    } else {
                        connection_to_add = Some(Connection {
                            from_node: target.node_id,
                            from_pin: target.pin_name,
                            to_node: from_node,
                            to_pin: from_pin,
                        });
                    }
                }
            }
        }

        // Apply mutations
        if node_drag_started {
            self.snapshot_undo();
        }

        if let Some(c) = connection_to_add {
            self.snapshot_undo();
            // Inputs have single incoming connection: replace existing if present
            self.graph
                .connections
                .retain(|existing| !(existing.to_node == c.to_node && existing.to_pin == c.to_pin));
            self.graph.connections.push(c);
        }

        if let Some((node_id, ref pin_name, is_output)) = socket_to_disconnect {
            let had_connections = if is_output {
                self.graph
                    .connections
                    .iter()
                    .any(|c| c.from_node == node_id && c.from_pin == *pin_name)
            } else {
                self.graph
                    .connections
                    .iter()
                    .any(|c| c.to_node == node_id && c.to_pin == *pin_name)
            };
            if had_connections {
                self.snapshot_undo();
                if is_output {
                    self.graph
                        .connections
                        .retain(|c| !(c.from_node == node_id && c.from_pin == *pin_name));
                } else {
                    self.graph
                        .connections
                        .retain(|c| !(c.to_node == node_id && c.to_pin == *pin_name));
                }
                self.status_message = Some(("Deleted wire".to_string(), false));
            }
        }

        if let Some(del_id) = node_to_delete {
            self.snapshot_undo();
            self.graph.nodes.retain(|n| n.id != del_id);
            self.graph
                .connections
                .retain(|c| c.from_node != del_id && c.to_node != del_id);
            if self.selected_node == Some(del_id) {
                self.selected_node = None;
            }
        }

        // Context Menu / Catalog on Right Click in empty space
        if response.secondary_clicked() && socket_to_disconnect.is_none() {
            let mouse_pos = ui
                .input(|i| i.pointer.hover_pos())
                .unwrap_or(canvas_rect.center());
            let clicked_on_node = node_screen_rects.values().any(|r| r.contains(mouse_pos));
            if !clicked_on_node {
                let world_pos =
                    screen_to_world(canvas_rect, self.graph.pan, self.graph.zoom, mouse_pos);
                self.spawn_node_pos = Some([world_pos.x, world_pos.y]);
                self.catalog_open = true;
                self.catalog_search.clear();
            }
        }

        // Open external script in editor if requested
        if let Some(ref fname) = script_to_open {
            let editor = self.config.workbench.resolve_editor();
            let recipes_dir = get_recipes_dir();
            let script_path = recipes_dir.join(fname);

            if !script_path.exists() {
                let is_py = fname.ends_with(".py");
                let _ = create_script_file_scaffold(&self.selected_recipe_name, is_py);
            }

            match std::process::Command::new(&editor)
                .arg(&script_path)
                .spawn()
            {
                Ok(_) => {
                    self.status_message = Some((format!("Opened {} in {}", fname, editor), false));
                }
                Err(e) => {
                    if editor != "xdg-open" {
                        let _ = std::process::Command::new("xdg-open")
                            .arg(&script_path)
                            .spawn();
                        self.status_message = Some((
                            format!(
                                "Opened {} via xdg-open (failed to launch '{}': {})",
                                fname, editor, e
                            ),
                            false,
                        ));
                    } else {
                        self.status_message =
                            Some((format!("Error launching editor '{}': {}", editor, e), true));
                    }
                }
            }
        }

        // Catalog Modal / Popup
        if self.catalog_open {
            self.render_catalog_window(ui, canvas_rect, palette);
        }

        // Save Recipe Modal
        if self.save_dialog_open {
            self.render_save_window(ui, palette);
        }

        action
    }

    fn render_toolbar(
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
                if self.current_recipe_file.is_some()
                    && self.selected_recipe_name != "Untitled Recipe"
                {
                    let _ = sync_recipe_script_headers(
                        &mut self.graph,
                        &self.selected_recipe_name,
                        false,
                    );
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

    fn render_catalog_window(
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

    fn render_save_window(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        self.render_save_window_in_dir(ui, palette, &get_recipes_dir());
    }

    fn render_save_window_in_dir(
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
                        let new_name = self.save_name.trim().to_string();
                        let duplicate_scripts = self.save_is_save_as;
                        let _ = sync_recipe_script_headers_in_dir(
                            &mut self.graph,
                            &new_name,
                            duplicate_scripts,
                            recipes_dir,
                        );

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

fn render_grid(
    painter: &egui::Painter,
    rect: Rect,
    pan: [f32; 2],
    zoom: f32,
    palette: &AppPalette,
) {
    // Dark blueprint background from theme
    painter.rect_filled(rect, Rounding::ZERO, hex_to_c32(&palette.colors.canvas));

    let grid_size = 32.0 * zoom;
    if grid_size > 8.0 {
        let offset_x = (pan[0] % grid_size + grid_size) % grid_size;
        let offset_y = (pan[1] % grid_size + grid_size) % grid_size;

        let dot_color = hex_to_c32_alpha(&palette.colors.surface_panel, 120);

        let mut x = rect.min.x + offset_x;
        while x < rect.max.x {
            let mut y = rect.min.y + offset_y;
            while y < rect.max.y {
                painter.circle_filled(Pos2::new(x, y), 1.0, dot_color);
                y += grid_size;
            }
            x += grid_size;
        }
    }
}

fn eval_cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;

    let x = uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x;
    let y = uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y;
    Pos2::new(x, y)
}

struct SnappedTargetInfo {
    node_id: Uuid,
    node_title: String,
    pin_name: String,
    port_type: PortType,
    pin_pos: Pos2,
    is_compatible: bool,
    from_port_type: Option<PortType>,
}

fn draw_active_cable(
    painter: &egui::Painter,
    source_pos: Pos2,
    mouse_pos: Pos2,
    is_output: bool,
    snapped: Option<&SnappedTargetInfo>,
    time: f64,
    zoom: f32,
    palette: &AppPalette,
) {
    let end_pos = if let Some(target) = snapped {
        target.pin_pos
    } else {
        mouse_pos
    };

    // p0 is always the output end, p3 is always the input end
    let (p0, p3) = if is_output {
        (source_pos, end_pos)
    } else {
        (end_pos, source_pos)
    };

    let dx = ((p3.x - p0.x).abs() * 0.5).max(40.0 * zoom);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);

    if let Some(target) = snapped {
        let is_compat = target.is_compatible;
        let main_color = if is_compat {
            hex_to_c32(&palette.diff_add)
        } else {
            hex_to_c32(&palette.diff_mod)
        };
        let core_color = if is_compat {
            hex_to_c32(&palette.harmonies.triadic_1)
        } else {
            hex_to_c32(&palette.harmonies.triadic_2)
        };

        let pulse = ((time * 6.0).sin() * 0.5 + 0.5) as f32;

        // 1. Wide pulsating outer halo
        let glow_alpha = (50.0 + 35.0 * pulse) as u8;
        let glow_stroke = (9.0 + 3.0 * pulse) * zoom;
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [p0, p1, p2, p3],
            false,
            Color32::TRANSPARENT,
            Stroke::new(
                glow_stroke,
                Color32::from_rgba_unmultiplied(
                    main_color.r(),
                    main_color.g(),
                    main_color.b(),
                    glow_alpha,
                ),
            ),
        ));

        // 2. High-energy wire body
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [p0, p1, p2, p3],
            false,
            Color32::TRANSPARENT,
            Stroke::new(3.5 * zoom, main_color),
        ));

        // 3. Bright core highlight
        painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
            [p0, p1, p2, p3],
            false,
            Color32::TRANSPARENT,
            Stroke::new(1.5 * zoom, core_color),
        ));

        // 4. Electric traveling spark packets flowing from output (p0) to input (p3)
        for i in 0..4 {
            let t = ((time * 1.5 + i as f64 * 0.25) % 1.0) as f32;
            let pt = eval_cubic_bezier(p0, p1, p2, p3, t);
            let spark_glow = Color32::from_rgba_unmultiplied(
                main_color.r(),
                main_color.g(),
                main_color.b(),
                120,
            );
            painter.circle_filled(pt, 5.0 * zoom, spark_glow);
            painter.circle_filled(pt, 2.5 * zoom, Color32::WHITE);
        }

        // 5. Sonar ripple waves expanding outward from target socket
        let target_pos = target.pin_pos;
        for ring_idx in 0..2 {
            let phase = ((time * 1.5 + ring_idx as f64 * 0.5) % 1.0) as f32;
            let r = (6.0 + phase * 22.0) * zoom;
            let alpha = ((1.0 - phase) * 220.0) as u8;
            painter.circle_stroke(
                target_pos,
                r,
                Stroke::new(
                    1.8 * zoom,
                    Color32::from_rgba_unmultiplied(
                        main_color.r(),
                        main_color.g(),
                        main_color.b(),
                        alpha,
                    ),
                ),
            );
        }

        // 6. Glowing socket core
        painter.circle_filled(
            target_pos,
            (7.5 + 2.5 * pulse) * zoom,
            Color32::from_rgba_unmultiplied(main_color.r(), main_color.g(), main_color.b(), 120),
        );
        painter.circle_filled(target_pos, 4.8 * zoom, Color32::WHITE);
        painter.circle_stroke(target_pos, 4.8 * zoom, Stroke::new(1.5 * zoom, main_color));

        // 7. Floating Connection HUD Badge
        let badge_text = if is_compat {
            format!(
                "Connect to {}.{} [{:?}]",
                target.node_title, target.pin_name, target.port_type
            )
        } else {
            let from_t_str = target
                .from_port_type
                .map(|t| format!("{:?}", t))
                .unwrap_or_else(|| "Any".into());
            format!(
                "Connect: {} -> {:?} ({}.{})",
                from_t_str, target.port_type, target.node_title, target.pin_name
            )
        };

        let font_id = egui::FontId::proportional((11.0 * zoom).max(10.0));
        let galley = painter.layout_no_wrap(
            badge_text,
            font_id,
            hex_to_c32(&palette.colors.text_primary),
        );
        let padding = Vec2::new(8.0 * zoom, 4.0 * zoom);
        let badge_size = galley.size() + padding * 2.0;

        // Position badge above the target pin, flipping below if too close to top
        let mut badge_center = target_pos + Vec2::new(0.0, -24.0 * zoom);
        if badge_center.y - badge_size.y * 0.5 < painter.clip_rect().min.y {
            badge_center.y = target_pos.y + 24.0 * zoom;
        }
        if badge_center.x - badge_size.x * 0.5 < painter.clip_rect().min.x {
            badge_center.x = painter.clip_rect().min.x + badge_size.x * 0.5 + 4.0;
        }
        let badge_rect = Rect::from_center_size(badge_center, badge_size);

        // Badge background
        painter.rect_filled(
            badge_rect,
            Rounding::same(6.0 * zoom),
            hex_to_c32_alpha(&palette.colors.surface_recessed, 235),
        );
        // Badge border
        painter.rect_stroke(
            badge_rect,
            Rounding::same(6.0 * zoom),
            Stroke::new(1.5 * zoom, main_color),
        );
        // Text inside badge
        painter.galley(
            badge_rect.min + padding,
            galley,
            hex_to_c32(&palette.colors.text_primary),
        );
    } else {
        // Not snapped: normal smooth dragging cable
        let cable_color = hex_to_c32(&palette.harmonies.base);
        draw_bezier_cable(painter, p0, p3, cable_color, zoom);

        // Glowing focal point at cursor
        painter.circle_filled(
            mouse_pos,
            5.0 * zoom,
            hex_to_c32_alpha(&palette.harmonies.base, 80),
        );
        painter.circle_filled(mouse_pos, 2.5 * zoom, hex_to_c32(&palette.harmonies.base));
    }
}

fn draw_bezier_cable(painter: &egui::Painter, p0: Pos2, p3: Pos2, color: Color32, zoom: f32) {
    let dx = ((p3.x - p0.x).abs() * 0.5).max(40.0 * zoom);
    let p1 = Pos2::new(p0.x + dx, p0.y);
    let p2 = Pos2::new(p3.x - dx, p3.y);

    // Glow shadow
    let glow_color = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40);
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [p0, p1, p2, p3],
        false,
        Color32::TRANSPARENT,
        Stroke::new(6.0 * zoom, glow_color),
    ));

    // Core line
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [p0, p1, p2, p3],
        false,
        Color32::TRANSPARENT,
        Stroke::new(2.5 * zoom, color),
    ));
}

fn render_node_card(
    zoom: f32,
    node_previews: &HashMap<Uuid, String>,
    dragging_from: &mut Option<(Uuid, String, bool)>,
    socket_to_disconnect: &mut Option<(Uuid, String, bool)>,
    script_to_open: &mut Option<String>,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    node: &mut NodeInstance,
    rect: Rect,
    is_selected: bool,
    node_to_delete: &mut Option<Uuid>,
    palette: &AppPalette,
) {
    let border_col = if is_selected {
        hex_to_c32(&palette.harmonies.base)
    } else {
        hex_to_c32(&palette.colors.surface_panel)
    };

    // Card body background
    painter.rect_filled(
        rect,
        Rounding::same(8.0 * zoom),
        hex_to_c32(&palette.colors.surface_recessed),
    );
    painter.rect_stroke(
        rect,
        Rounding::same(8.0 * zoom),
        Stroke::new(if is_selected { 2.0_f32 } else { 1.2_f32 }, border_col),
    );

    // Category Header banner
    let header_h = 26.0 * zoom;
    let header_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_h));
    let cat_color = get_node_category_color(&node.kind, palette);
    painter.rect_filled(
        header_rect,
        Rounding {
            nw: 8.0 * zoom,
            ne: 8.0 * zoom,
            sw: 0.0,
            se: 0.0,
        },
        cat_color,
    );
    let category_text = crate::ui_ext::contrast_text_c32_on(
        cat_color,
        cat_color,
        Some(hex_to_c32(&palette.colors.text_primary)),
        palette,
    );

    // Node Title
    painter.text(
        header_rect.left_center() + Vec2::new(8.0 * zoom, 0.0),
        egui::Align2::LEFT_CENTER,
        &node.title,
        egui::FontId::proportional(12.0 * zoom),
        category_text,
    );

    // Close / Delete button (×)
    let close_rect = Rect::from_min_size(
        header_rect.right_top() + Vec2::new(-20.0 * zoom, 2.0 * zoom),
        Vec2::new(18.0 * zoom, 18.0 * zoom),
    );
    let close_resp = ui.interact(
        close_rect,
        ui.make_persistent_id((node.id, "del")),
        egui::Sense::click(),
    );
    if close_resp.clicked() {
        *node_to_delete = Some(node.id);
    }
    let close_color = if close_resp.hovered() {
        hex_to_c32(&palette.diff_del)
    } else {
        hex_to_c32(&palette.colors.text_primary)
    };
    let close_col =
        crate::ui_ext::contrast_text_c32_on(cat_color, cat_color, Some(close_color), palette);
    painter.text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        "×",
        egui::FontId::monospace(14.0 * zoom),
        close_col,
    );

    // Pins Section
    let pin_row_h = 24.0 * zoom;
    let max_pins = node.inputs.len().max(node.outputs.len());

    // Render Input Pins on Left Edge
    for (idx, pin) in node.inputs.iter().enumerate() {
        let pin_pos = get_pin_pos_by_idx(rect, node.inputs.len(), idx, false, zoom);
        render_socket(
            ui,
            painter,
            pin_pos,
            node.id,
            &pin.name,
            pin.port_type,
            false,
            zoom,
            dragging_from,
            socket_to_disconnect,
            palette,
        );

        painter.text(
            pin_pos + Vec2::new(12.0 * zoom, 0.0),
            egui::Align2::LEFT_CENTER,
            &pin.name,
            egui::FontId::proportional(11.0 * zoom),
            hex_to_c32(&palette.colors.text_secondary),
        );
    }

    // Render Output Pins on Right Edge
    for (idx, pin) in node.outputs.iter().enumerate() {
        let pin_pos = get_pin_pos_by_idx(rect, node.outputs.len(), idx, true, zoom);
        render_socket(
            ui,
            painter,
            pin_pos,
            node.id,
            &pin.name,
            pin.port_type,
            true,
            zoom,
            dragging_from,
            socket_to_disconnect,
            palette,
        );

        painter.text(
            pin_pos + Vec2::new(-12.0 * zoom, 0.0),
            egui::Align2::RIGHT_CENTER,
            &pin.name,
            egui::FontId::proportional(11.0 * zoom),
            hex_to_c32(&palette.colors.text_secondary),
        );
    }

    // Calculate vertical offset where properties section begins (below all pins)
    let pins_total_h = if max_pins > 0 {
        8.0 * zoom + (max_pins as f32 * pin_row_h) + 6.0 * zoom
    } else {
        6.0 * zoom
    };
    let props_top = rect.min.y + header_h + pins_total_h;

    // Subtle divider between pins and properties
    let is_script_node = matches!(node.kind, NodeKind::PythonScript | NodeKind::BashCommand);
    if (!node.props.is_empty() || is_script_node) && max_pins > 0 {
        painter.line_segment(
            [
                Pos2::new(rect.min.x + 8.0 * zoom, props_top - 4.0 * zoom),
                Pos2::new(rect.max.x - 8.0 * zoom, props_top - 4.0 * zoom),
            ],
            Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
        );
    }

    // Inline editable properties inside card (strictly placed below pins, no overlap!)
    if is_script_node || !node.props.is_empty() {
        let content_min = Pos2::new(rect.min.x + 12.0 * zoom, props_top);
        let content_max = Pos2::new(rect.max.x - 12.0 * zoom, rect.max.y - 8.0 * zoom);
        let content_rect = Rect::from_min_max(content_min, content_max);

        let mut child_ui = ui.child_ui(content_rect, egui::Layout::top_down(egui::Align::Min));
        child_ui.set_clip_rect(content_rect);

        if is_script_node {
            let script_file = node.props.get("script_file").cloned().unwrap_or_else(|| {
                let is_py = matches!(node.kind, NodeKind::PythonScript);
                let ext = if is_py { "py" } else { "sh" };
                format!("script_{}.{}", &node.id.to_string()[..8], ext)
            });

            child_ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("file:")
                        .color(hex_to_c32(&palette.colors.text_muted))
                        .small(),
                );
                let file_fill = hex_to_c32(&palette.colors.surface_panel);
                let file_btn = crate::ui_ext::secondary_button_icon(
                    Some(Icon::Code),
                    &script_file,
                    file_fill,
                    palette,
                );

                if ui
                    .add(file_btn)
                    .on_hover_text("Click to open script in text editor")
                    .clicked()
                {
                    *script_to_open = Some(script_file.clone());
                }
            });

            child_ui.add_space(2.0);

            let edit_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Pencil),
                "Open in Editor",
                hex_to_c32(&palette.harmonies.base),
                palette,
            )
            .min_size(Vec2::new(child_ui.available_width(), 20.0 * zoom));

            if child_ui.add(edit_btn).clicked() {
                *script_to_open = Some(script_file);
            }
        } else {
            for (key, val) in node.props.iter_mut() {
                child_ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(key.replace('_', " "))
                            .color(hex_to_c32(&palette.colors.text_muted))
                            .small(),
                    );
                    let field_width = ui.available_width() * 0.95;
                    ui.add_sized(
                        [field_width, CONTROL_HEIGHT],
                        egui::TextEdit::singleline(val)
                            .desired_width(field_width)
                            .font(egui::TextStyle::Body),
                    );
                });
            }
        }
    }

    // Last Output Preview Badge at bottom
    if let Some(preview_str) = node_previews.get(&node.id) {
        let btm_pos = rect.left_bottom() + Vec2::new(10.0, -8.0);
        let preview_short = if preview_str.len() > 24 {
            format!("{}…", &preview_str[..23])
        } else {
            preview_str.clone()
        };
        painter.text(
            btm_pos,
            egui::Align2::LEFT_BOTTOM,
            format!("{}", preview_short),
            egui::FontId::proportional(9.5 * zoom),
            crate::ui_ext::contrast_text_c32_on(
                hex_to_c32(&palette.colors.surface_recessed),
                hex_to_c32(&palette.colors.surface_recessed),
                Some(hex_to_c32(&palette.diff_add)),
                palette,
            ),
        );
    }
}

fn render_socket(
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    pos: Pos2,
    node_id: Uuid,
    pin_name: &str,
    port_type: PortType,
    is_output: bool,
    zoom: f32,
    dragging_from: &mut Option<(Uuid, String, bool)>,
    socket_to_disconnect: &mut Option<(Uuid, String, bool)>,
    palette: &AppPalette,
) {
    let radius = 5.5 * zoom;
    let base_col = port_type.color_rgba();
    let base_col =
        Color32::from_rgba_unmultiplied(base_col[0], base_col[1], base_col[2], base_col[3]);

    let resp = ui.interact(
        Rect::from_center_size(pos, Vec2::splat(radius * 3.0)),
        ui.make_persistent_id((node_id, pin_name, is_output)),
        egui::Sense::click_and_drag(),
    );

    // Right-click socket to disconnect wires
    if resp.secondary_clicked() {
        *socket_to_disconnect = Some((node_id, pin_name.to_string(), is_output));
    }

    if resp.drag_started() {
        *dragging_from = Some((node_id, pin_name.to_string(), is_output));
    }

    if resp.hovered() {
        painter.circle_filled(pos, radius * 1.35, Color32::WHITE);
        painter.circle_stroke(pos, radius * 1.35, Stroke::new(2.0_f32, base_col));
    } else {
        painter.circle_filled(pos, radius, base_col);
        painter.circle_stroke(
            pos,
            radius,
            Stroke::new(1.2_f32, hex_to_c32(&palette.colors.canvas)),
        );
    }
}

fn world_to_screen(canvas_rect: Rect, pan: [f32; 2], zoom: f32, pt: Pos2) -> Pos2 {
    canvas_rect.min + Vec2::new(pan[0] + pt.x * zoom, pan[1] + pt.y * zoom)
}

fn screen_to_world(canvas_rect: Rect, pan: [f32; 2], zoom: f32, pt: Pos2) -> Pos2 {
    let rel = pt - canvas_rect.min;
    Pos2::new((rel.x - pan[0]) / zoom, (rel.y - pan[1]) / zoom)
}

fn estimate_node_height(node: &NodeInstance) -> f32 {
    let header_h = 28.0;
    let pin_row_h = 24.0;
    let max_pins = node.inputs.len().max(node.outputs.len());
    let pins_h = if max_pins > 0 {
        8.0 + (max_pins as f32 * pin_row_h) + 6.0
    } else {
        6.0
    };
    let is_script = matches!(node.kind, NodeKind::PythonScript | NodeKind::BashCommand);
    let props_h = if is_script {
        72.0
    } else if !node.props.is_empty() {
        node.props.len() as f32 * 30.0 + 8.0
    } else {
        0.0
    };
    (header_h + pins_h + props_h + 10.0).max(65.0)
}

fn get_pin_pos(
    node_rect: Rect,
    nodes: &[NodeInstance],
    node_id: Uuid,
    pin_name: &str,
    is_output: bool,
    zoom: f32,
) -> Pos2 {
    if let Some(node) = nodes.iter().find(|n| n.id == node_id) {
        let pins = if is_output {
            &node.outputs
        } else {
            &node.inputs
        };
        if let Some(idx) = pins
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(pin_name))
        {
            return get_pin_pos_by_idx(node_rect, pins.len(), idx, is_output, zoom);
        }
    }
    if is_output {
        node_rect.right_center()
    } else {
        node_rect.left_center()
    }
}

fn get_pin_pos_by_idx(
    node_rect: Rect,
    _total_pins: usize,
    idx: usize,
    is_output: bool,
    zoom: f32,
) -> Pos2 {
    let header_h = 28.0 * zoom;
    let pin_row_h = 24.0 * zoom;
    let y = node_rect.min.y + header_h + 8.0 * zoom + (idx as f32 * pin_row_h) + (pin_row_h * 0.5);
    let x = if is_output {
        node_rect.max.x
    } else {
        node_rect.min.x
    };
    Pos2::new(x, y)
}

fn get_node_category_color(kind: &NodeKind, palette: &AppPalette) -> Color32 {
    match kind {
        NodeKind::LeftHandSource
        | NodeKind::RightHandSource
        | NodeKind::SlotSource(_)
        | NodeKind::ConstantText
        | NodeKind::ConstantNumber
        | NodeKind::PromptOrHandText => hex_to_c32(&palette.harmonies.triadic_1),

        NodeKind::TextTrim
        | NodeKind::TextSlugify
        | NodeKind::TextCase
        | NodeKind::TextRegexReplace
        | NodeKind::TextSplitLines
        | NodeKind::TextJoin
        | NodeKind::TextJsonQuery
        | NodeKind::TextBase64 => hex_to_c32(&palette.harmonies.triadic_2),

        NodeKind::ImagePadFrame
        | NodeKind::ImageAddTitle
        | NodeKind::ImageCrop
        | NodeKind::ImageResize
        | NodeKind::ImageConvertWebp
        | NodeKind::ImageRotate
        | NodeKind::ImageStitch
        | NodeKind::ImageStripExif
        | NodeKind::ImageDiff => hex_to_c32(&palette.harmonies.split_comp_1),

        NodeKind::AudioNormalize
        | NodeKind::AudioTrimSilence
        | NodeKind::AudioAdjustGain
        | NodeKind::AudioConcat => hex_to_c32(&palette.harmonies.base),

        NodeKind::FileSha256 | NodeKind::FileZip | NodeKind::FileExtractName => {
            hex_to_c32(&palette.diff_mod)
        }

        NodeKind::PythonScript | NodeKind::BashCommand => hex_to_c32(&palette.diff_add),

        NodeKind::SetLeftHand
        | NodeKind::SetRightHand
        | NodeKind::SaveToDisk
        | NodeKind::PasteToApp => hex_to_c32(&palette.diff_del),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_pipeline_emits_graph_for_single_daemon_execution() {
        let mut canvas = NodeCanvasState::new();
        let source = instantiate_template(
            little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap(),
            [0.0, 0.0],
        );
        let mut source = source;
        source.props.insert("text".to_owned(), "once".to_owned());
        let sink = instantiate_template(
            little_clip_ops::pipeline::catalog::find_template("sink_lh").unwrap(),
            [300.0, 0.0],
        );
        canvas.graph.nodes = vec![source.clone(), sink.clone()];
        canvas.graph.connections = vec![Connection {
            from_node: source.id,
            from_pin: "Text".to_owned(),
            to_node: sink.id,
            to_pin: "In".to_owned(),
        }];

        let action = canvas.run_pipeline_action();
        let Some(CanvasAction::RunPipeline { yaml }) = action else {
            panic!("run action must send the graph to its single execution owner");
        };
        let sent_graph: NodeGraph = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(sent_graph.nodes.len(), 2);
        assert!(
            canvas.node_previews.is_empty(),
            "UI must not execute locally"
        );
        assert!(canvas.last_run_duration_ms.is_none());
        assert!(matches!(
            canvas.status_message.as_ref(),
            Some((message, false)) if message == "Running pipeline…"
        ));

        // Exercise the daemon's production graph executor and slot-result behavior in-process.
        // This graph contains only a constant text node and the Left Hand sink, so no external
        // command, daemon, clipboard, or display refresh is involved.
        let mut slots = little_clip_core::SlotManager::new();
        slots.push_left(little_clip_core::ClipItem::from_text("previous item"));
        let execution = little_clip_ops::PipelineExecutor::new(&sent_graph, &slots)
            .execute()
            .expect("constant-text Left Hand graph should execute");
        let output = execution
            .left_hand_item
            .expect("SetLeftHand should declare a Left Hand output");
        assert_eq!(output.title, "once");
        assert_eq!(output.to_clipboard_text().as_deref(), Some("once"));
        slots.apply_op_result(output);
        assert_eq!(
            slots.left_hand().and_then(|item| item.to_clipboard_text()),
            Some("once".to_owned())
        );
        assert_eq!(
            slots.undo_slot().and_then(|item| item.to_clipboard_text()),
            Some("previous item".to_owned())
        );
    }

    #[test]
    fn daemon_pipeline_response_restores_previews_and_output_status() {
        let mut canvas = NodeCanvasState::new();
        let node_id = canvas.graph.nodes[0].id;
        canvas.apply_pipeline_response(little_clip_core::ipc::IpcResponse::PipelineExecuted {
            node_previews: [(node_id, "preview".to_owned())].into_iter().collect(),
            output_title: Some("once".to_owned()),
            duration_ms: 17,
        });

        assert_eq!(
            canvas.node_previews.get(&node_id).map(String::as_str),
            Some("preview")
        );
        assert_eq!(canvas.last_run_duration_ms, Some(17));
        assert!(matches!(
            canvas.status_message.as_ref(),
            Some((message, false)) if message == "Executed in 17ms. Output: once"
        ));
    }

    #[test]
    fn node_machine_history_and_delete_shortcuts_yield_to_focused_text_input() {
        fn test_canvas() -> NodeCanvasState {
            let mut canvas = NodeCanvasState::with_config(AppConfig::default());
            let node = instantiate_template(
                little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap(),
                [0.0, 0.0],
            );
            let mut previous = node.clone();
            previous.title = "Previous title".to_owned();
            let mut next = node.clone();
            next.title = "Next title".to_owned();
            canvas.graph.nodes = vec![node.clone()];
            canvas.selected_node = Some(node.id);
            let mut previous_graph = canvas.graph.clone();
            previous_graph.nodes[0] = previous;
            let mut next_graph = canvas.graph.clone();
            next_graph.nodes[0] = next;
            canvas.undo_stack.push(previous_graph);
            canvas.redo_stack.push(next_graph);
            canvas
        }

        fn input(events: Vec<egui::Event>) -> egui::RawInput {
            let modifiers = events
                .iter()
                .find_map(|event| match event {
                    egui::Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default();
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 760.0))),
                events,
                modifiers,
                ..Default::default()
            }
        }

        fn key(key: egui::Key, command: bool) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers {
                    command,
                    ctrl: command,
                    ..Default::default()
                },
            }
        }

        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let cases = [
            (egui::Key::Z, true, "Previous title"),
            (egui::Key::Y, true, "Next title"),
            (egui::Key::Delete, false, "Deleted node"),
        ];

        for (shortcut, command, unfocused_title) in cases {
            let ctx = egui::Context::default();
            let mut focused_canvas = test_canvas();
            focused_canvas.catalog_open = true;
            let _ = ctx.run(input(Vec::new()), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    focused_canvas.ui(ui, &palette);
                });
            });
            let node_id = focused_canvas.selected_node.unwrap();
            let _ = ctx.run(input(vec![key(shortcut, command)]), |ctx| {
                assert!(
                    ctx.wants_keyboard_input(),
                    "catalog search owns keyboard focus"
                );
                egui::CentralPanel::default().show(ctx, |ui| {
                    focused_canvas.ui(ui, &palette);
                });
            });
            assert!(
                focused_canvas
                    .graph
                    .nodes
                    .iter()
                    .any(|node| node.id == node_id),
                "focused text input must prevent {shortcut:?} from changing the canvas"
            );
            assert_eq!(
                focused_canvas.graph.nodes[0].title,
                test_canvas().graph.nodes[0].title,
                "focused text input must prevent {shortcut:?} from changing history state"
            );

            let ctx = egui::Context::default();
            let mut unfocused_canvas = test_canvas();
            let _ = ctx.run(input(Vec::new()), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    unfocused_canvas.ui(ui, &palette);
                });
            });
            let _ = ctx.run(input(vec![key(shortcut, command)]), |ctx| {
                assert!(!ctx.wants_keyboard_input());
                egui::CentralPanel::default().show(ctx, |ui| {
                    unfocused_canvas.ui(ui, &palette);
                });
            });
            if shortcut == egui::Key::Delete {
                assert!(unfocused_canvas.graph.nodes.is_empty());
            } else {
                assert_eq!(unfocused_canvas.graph.nodes[0].title, unfocused_title);
            }
        }
    }

    #[test]
    fn node_machine_toolbar_wraps_controls_and_status_at_compact_widths() {
        for width in [760.0, 1280.0, 1760.0] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let mut canvas = NodeCanvasState::with_config(AppConfig::default());
            canvas.status_message = Some(("Pipeline complete".to_string(), false));
            let mut action = None;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 120.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let host = ui.available_rect_before_wrap();
                        canvas.render_toolbar(ui, &mut action, &palette);
                        assert!(ui.min_rect().left() >= host.left() - 0.5);
                    });
                },
            );

            let required_labels = [
                "Add Node",
                "New Recipe",
                "Save",
                "Save As...",
                "Undo",
                "Redo",
                "Clear",
                "RUN PIPELINE",
                "Pipeline complete",
            ];
            let mut label_bounds = HashMap::new();
            for required in required_labels {
                let shape = output.shapes.iter().find_map(|clipped| {
                    let egui::epaint::Shape::Text(text) = &clipped.shape else {
                        return None;
                    };
                    text.galley
                        .job
                        .text
                        .contains(required)
                        .then_some((clipped.clip_rect, text))
                });
                let Some((clip_rect, text)) = shape else {
                    panic!("Node Machine toolbar omitted {required:?} at {width} points");
                };
                let bounds = text.galley.rect.translate(text.pos.to_vec2());
                assert!(!text.galley.elided, "{required:?} elided at {width} points");
                assert!(
                    bounds.left() >= clip_rect.left() - 0.5
                        && bounds.right() <= clip_rect.right() + 0.5,
                    "{required:?} exceeds {width}-point toolbar: {bounds:?} clipped to {clip_rect:?}"
                );
                assert!(
                    bounds.top() >= clip_rect.top() - 0.5
                        && bounds.bottom() <= clip_rect.bottom() + 0.5,
                    "{required:?} is vertically clipped at {width} points: {bounds:?} clipped to {clip_rect:?}"
                );
                label_bounds.insert(required, bounds);
            }
            let status = label_bounds["Pipeline complete"];
            for action in [
                "Add Node",
                "New Recipe",
                "Save",
                "Save As...",
                "Undo",
                "Redo",
                "Clear",
                "RUN PIPELINE",
            ] {
                let action_bounds = label_bounds[action];
                assert!(
                    status.top() >= action_bounds.bottom() - 0.5
                        || status.bottom() <= action_bounds.top() + 0.5,
                    "status overlaps {action} at {width} points: {status:?} vs {action_bounds:?}"
                );
            }
        }
    }

    #[test]
    fn compact_canvas_keeps_complete_python_script_controls_inside_its_viewport() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut canvas = NodeCanvasState::with_config(AppConfig::default());
        let mut script = canvas.graph.nodes[0].clone();
        script.kind = NodeKind::PythonScript;
        script.title = "Python Transform".to_owned();
        script.pos = [0.0, 0.0];
        script.props.clear();
        script
            .props
            .insert("script_file".to_owned(), "transform.py".to_owned());
        let script_id = script.id;
        canvas.graph.nodes = vec![script];
        canvas.graph.connections.clear();
        canvas.graph.pan = [4.0, 4.0];
        canvas.graph.zoom = 1.24;
        canvas.selected_node = Some(script_id);
        canvas.status_message = Some(("Saved recipe 'Pad and Caption'".to_owned(), false));
        let mut canvas_bounds = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1177.0, 662.0))),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Compact host after a reduced, still-interactive mirror and shared footer.
                    // The production canvas receives this remaining vertical budget.
                    let allocated = ui.allocate_ui(Vec2::new(ui.available_width(), 270.0), |ui| {
                        canvas.ui(ui, &palette);
                    });
                    canvas_bounds = Some(allocated.response.rect);
                });
            },
        );
        let footer_top = canvas_bounds.expect("compact canvas allocation").bottom();

        let required = ["Python Transform", "transform.py", "Open in Editor"];
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else {
                    return None;
                };
                Some((clipped.clip_rect, text))
            })
            .collect();
        for label in required {
            let (clip, text) = texts
                .iter()
                .find(|(_, text)| text.galley.job.text.contains(label))
                .unwrap_or_else(|| panic!("missing script property {label:?}"));
            let bounds = text.galley.rect.translate(text.pos.to_vec2());
            assert!(!text.galley.elided, "{label:?} was elided");
            assert!(
                bounds.left() >= clip.left() - 0.5
                    && bounds.right() <= clip.right() + 0.5
                    && bounds.top() >= clip.top() - 0.5
                    && bounds.bottom() <= clip.bottom() + 0.5,
                "{label:?} is clipped in compact canvas: {bounds:?} within {clip:?}"
            );
            assert!(
                bounds.bottom() <= footer_top + 0.5,
                "{label:?} overlaps the fixed footer beginning at {footer_top}: {bounds:?}"
            );
        }
    }

    #[test]
    fn node_canvas_custom_labels_meet_contrast_for_every_category() {
        let categories = [
            NodeKind::LeftHandSource,
            NodeKind::TextTrim,
            NodeKind::ImagePadFrame,
            NodeKind::AudioNormalize,
            NodeKind::FileSha256,
            NodeKind::PythonScript,
            NodeKind::SetLeftHand,
        ];
        for appearance in [
            little_clip_core::config::WorkbenchAppearance::Dark,
            little_clip_core::config::WorkbenchAppearance::Light,
        ] {
            let palette = crate::ui_appearance::palette(appearance);
            for kind in &categories {
                let background = get_node_category_color(kind, &palette);
                for preferred in [
                    hex_to_c32(&palette.colors.text_primary),
                    hex_to_c32(&palette.diff_del),
                ] {
                    let foreground = crate::ui_ext::contrast_text_c32_on(
                        background,
                        background,
                        Some(preferred),
                        &palette,
                    );
                    assert!(
                        little_clip_core::palette::contrast_ratio(
                            (background.r(), background.g(), background.b()),
                            (foreground.r(), foreground.g(), foreground.b()),
                        ) >= 4.5,
                        "{appearance:?} node category {kind:?} label lacks contrast"
                    );
                }
            }

            let recessed = hex_to_c32(&palette.colors.surface_recessed);
            let preview_text = crate::ui_ext::contrast_text_c32_on(
                recessed,
                recessed,
                Some(hex_to_c32(&palette.diff_add)),
                &palette,
            );
            assert!(
                little_clip_core::palette::contrast_ratio(
                    (recessed.r(), recessed.g(), recessed.b()),
                    (preview_text.r(), preview_text.g(), preview_text.b()),
                ) >= 4.5,
                "{appearance:?} node preview label lacks contrast"
            );
        }
    }

    #[test]
    fn test_canvas_undo_redo() {
        let mut state = NodeCanvasState::new();
        let initial_nodes_len = state.graph.nodes.len();
        let initial_conns_len = state.graph.connections.len();

        // 1. Mutate: add a node
        state.snapshot_undo();
        let t_const =
            little_clip_ops::pipeline::catalog::find_template("source_const_text").unwrap();
        let new_node = instantiate_template(t_const, [100.0, 100.0]);
        let new_node_id = new_node.id;
        state.graph.nodes.push(new_node);
        assert_eq!(state.graph.nodes.len(), initial_nodes_len + 1);

        // 2. Undo node addition
        state.undo();
        assert_eq!(state.graph.nodes.len(), initial_nodes_len);
        assert!(!state.graph.nodes.iter().any(|n| n.id == new_node_id));

        // 3. Redo node addition
        state.redo();
        assert_eq!(state.graph.nodes.len(), initial_nodes_len + 1);
        assert!(state.graph.nodes.iter().any(|n| n.id == new_node_id));

        // 4. Mutate: delete connection
        if !state.graph.connections.is_empty() {
            state.snapshot_undo();
            state.graph.connections.clear();
            assert_eq!(state.graph.connections.len(), 0);

            // Undo connection deletion
            state.undo();
            assert_eq!(state.graph.connections.len(), initial_conns_len);

            // Redo connection deletion
            state.redo();
            assert_eq!(state.graph.connections.len(), 0);
        }
    }

    #[test]
    fn test_eval_cubic_bezier() {
        let p0 = Pos2::new(10.0, 20.0);
        let p1 = Pos2::new(40.0, 20.0);
        let p2 = Pos2::new(60.0, 80.0);
        let p3 = Pos2::new(90.0, 80.0);

        // At t = 0.0, exact start point
        let start = eval_cubic_bezier(p0, p1, p2, p3, 0.0);
        assert!((start.x - p0.x).abs() < 1e-4);
        assert!((start.y - p0.y).abs() < 1e-4);

        // At t = 1.0, exact end point
        let end = eval_cubic_bezier(p0, p1, p2, p3, 1.0);
        assert!((end.x - p3.x).abs() < 1e-4);
        assert!((end.y - p3.y).abs() < 1e-4);

        // At t = 0.5, intermediate point between p0 and p3
        let mid = eval_cubic_bezier(p0, p1, p2, p3, 0.5);
        assert!(mid.x > p0.x && mid.x < p3.x);
        assert!(mid.y > p0.y && mid.y < p3.y);
    }

    #[test]
    fn test_magnetic_snap_distance_threshold() {
        let snap_threshold = (32.0 * 1.0_f32).clamp(22.0, 48.0);
        assert_eq!(snap_threshold, 32.0);

        let pin_pos = Pos2::new(200.0, 150.0);
        let close_mouse = Pos2::new(215.0, 160.0);
        let far_mouse = Pos2::new(250.0, 200.0);

        assert!(close_mouse.distance(pin_pos) < snap_threshold);
        assert!(far_mouse.distance(pin_pos) >= snap_threshold);
    }
}
