use eframe::egui::{self, Color32, Id, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::SlotManager;
use little_clip_ops::pipeline::catalog::{instantiate_template, search_templates};
use little_clip_ops::pipeline::recipes::{
    load_all_recipes, make_pad_and_caption_recipe, save_recipe,
};
use little_clip_ops::pipeline::types::{Connection, NodeGraph, NodeInstance, NodeKind, PortType, Recipe};
use little_clip_ops::PipelineExecutor;
use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

pub enum CanvasAction {
    RunPipeline,
    #[allow(dead_code)]
    AppliedResult { item_title: String },
}


pub struct NodeCanvasState {
    pub graph: NodeGraph,
    pub selected_node: Option<Uuid>,
    pub dragging_from: Option<(Uuid, String, bool)>, // (node_id, pin_name, is_output)
    pub node_previews: HashMap<Uuid, String>,
    pub catalog_open: bool,
    pub catalog_search: String,
    pub catalog_category: String,
    pub save_dialog_open: bool,
    pub save_name: String,
    pub status_message: Option<(String, bool)>, // (text, is_error)
    pub available_recipes: Vec<Recipe>,
    pub selected_recipe_name: String,
    pub last_run_duration_ms: Option<u128>,
}

impl NodeCanvasState {
    pub fn new() -> Self {
        let recipes = load_all_recipes();
        let default_recipe = recipes
            .iter()
            .find(|r| r.name.contains("Pad and Caption"))
            .cloned()
            .unwrap_or_else(make_pad_and_caption_recipe);

        let sel_name = default_recipe.name.clone();
        let graph = default_recipe.graph;

        Self {
            graph,
            selected_node: None,
            dragging_from: None,
            node_previews: HashMap::new(),
            catalog_open: false,
            catalog_search: String::new(),
            catalog_category: "All".to_string(),
            save_dialog_open: false,
            save_name: String::new(),
            status_message: None,
            available_recipes: recipes,
            selected_recipe_name: sel_name,
            last_run_duration_ms: None,
        }
    }

    pub fn reload_recipes(&mut self) {
        self.available_recipes = load_all_recipes();
    }

    pub fn run_current_graph(&mut self, slot_mgr: &SlotManager) -> Option<little_clip_ops::PipelineExecutionResult> {
        let start = Instant::now();
        let executor = PipelineExecutor::new(&self.graph, slot_mgr);
        match executor.execute() {
            Ok(res) => {
                let duration = start.elapsed().as_millis();
                self.last_run_duration_ms = Some(duration);
                self.node_previews = res.node_previews.clone();
                self.status_message = Some((
                    format!(
                        "✔ Executed in {}ms. Output: {}",
                        duration,
                        res.left_hand_item
                            .as_ref()
                            .map(|i| i.title.as_str())
                            .unwrap_or("Done")
                    ),
                    false,
                ));
                Some(res)
            }
            Err(e) => {
                self.status_message = Some((format!("Error: {}", e), true));
                None
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, slot_mgr: &SlotManager) -> Option<CanvasAction> {
        let mut action = None;

        // Top Toolbar
        self.render_toolbar(ui, slot_mgr, &mut action);

        ui.separator();

        // Main Canvas Interactive Area
        let (canvas_rect, response) = ui.allocate_exact_size(
            ui.available_size(),
            egui::Sense::click_and_drag(),
        );

        // Canvas Pan & Zoom
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.graph.pan[0] += response.drag_delta().x;
            self.graph.pan[1] += response.drag_delta().y;
        }

        let scroll_delta = ui.input(|i| i.raw_scroll_delta.y);
        if scroll_delta != 0.0 && response.hovered() {
            let zoom_factor = if scroll_delta > 0.0 { 1.08 } else { 0.92 };
            self.graph.zoom = (self.graph.zoom * zoom_factor).clamp(0.4, 2.2);
        }

        // Clip painter to canvas bounds
        let painter = ui.painter_at(canvas_rect);

        // 1. Draw Background Grid
        render_grid(&painter, canvas_rect, self.graph.pan, self.graph.zoom);

        // 2. Compute Node Screen Rectangles
        let mut node_screen_rects: HashMap<Uuid, Rect> = HashMap::new();
        for node in &self.graph.nodes {
            let screen_pos = world_to_screen(canvas_rect, self.graph.pan, self.graph.zoom, Pos2::new(node.pos[0], node.pos[1]));
            let width = 230.0 * self.graph.zoom;
            let height = estimate_node_height(node) * self.graph.zoom;
            node_screen_rects.insert(node.id, Rect::from_min_size(screen_pos, Vec2::new(width, height)));
        }

        // 3. Draw Existing Connection Cables
        for conn in &self.graph.connections {
            if let (Some(&from_rect), Some(&to_rect)) =
                (node_screen_rects.get(&conn.from_node), node_screen_rects.get(&conn.to_node))
            {
                let from_pos = get_pin_pos(from_rect, &self.graph.nodes, conn.from_node, &conn.from_pin, true, self.graph.zoom);
                let to_pos = get_pin_pos(to_rect, &self.graph.nodes, conn.to_node, &conn.to_pin, false, self.graph.zoom);
                let cable_color = Color32::from_rgb(168, 85, 247);
                draw_bezier_cable(&painter, from_pos, to_pos, cable_color, self.graph.zoom);
            }
        }

        // 4. Draw Active Dragging Cable
        if let Some((from_node, ref pin_name, is_output)) = self.dragging_from {
            if let Some(&node_rect) = node_screen_rects.get(&from_node) {
                let pin_pos = get_pin_pos(node_rect, &self.graph.nodes, from_node, pin_name, is_output, self.graph.zoom);
                if let Some(mouse_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    let cable_color = Color32::from_rgb(168, 85, 247);
                    draw_bezier_cable(&painter, pin_pos, mouse_pos, cable_color, self.graph.zoom);
                }
            }
        }

        // 5. Render Node Cards & Process Interactions
        let mut node_to_delete = None;
        let mut connection_to_add = None;


        let zoom = self.graph.zoom;
        for node in &mut self.graph.nodes {
            let node_id = node.id;
            let screen_rect = *node_screen_rects.get(&node_id).unwrap();

            // Check if node is dragged
            let node_resp = ui.interact(screen_rect, ui.make_persistent_id(node_id), egui::Sense::click_and_drag());
            if node_resp.dragged_by(egui::PointerButton::Primary) && self.dragging_from.is_none() {
                node.pos[0] += node_resp.drag_delta().x / zoom;
                node.pos[1] += node_resp.drag_delta().y / zoom;
                self.selected_node = Some(node_id);
            }
            if node_resp.clicked() {
                self.selected_node = Some(node_id);
            }

            // Render Card Frame
            let is_selected = self.selected_node == Some(node_id);
            render_node_card(
                zoom,
                &self.node_previews,
                &mut self.dragging_from,
                ui,
                &painter,
                node,
                screen_rect,
                is_selected,
                &mut node_to_delete,
            );
        }

        // Handle Pin Drop Connection
        if ui.input(|i| i.pointer.any_released()) {
            if let Some((from_node, from_pin, is_out)) = self.dragging_from.take() {
                if let Some(mouse_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    for (to_node, &to_rect) in &node_screen_rects {
                        if *to_node != from_node {
                            if let Some(node) = self.graph.nodes.iter().find(|n| n.id == *to_node) {
                                let pins = if is_out { &node.inputs } else { &node.outputs };
                                for (p_idx, pin) in pins.iter().enumerate() {
                                    let pin_pos = get_pin_pos_by_idx(to_rect, pins.len(), p_idx, !is_out, zoom);
                                    if mouse_pos.distance(pin_pos) < 14.0 {
                                        if is_out {
                                            connection_to_add = Some(Connection {
                                                from_node,
                                                from_pin: from_pin.clone(),
                                                to_node: *to_node,
                                                to_pin: pin.name.clone(),
                                            });
                                        } else {
                                            connection_to_add = Some(Connection {
                                                from_node: *to_node,
                                                from_pin: pin.name.clone(),
                                                to_node: from_node,
                                                to_pin: from_pin.clone(),
                                            });
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Apply mutations
        if let Some(c) = connection_to_add {
            // Inputs have single incoming connection: replace existing if present
            self.graph
                .connections
                .retain(|existing| !(existing.to_node == c.to_node && existing.to_pin == c.to_pin));
            self.graph.connections.push(c);
        }

        if let Some(del_id) = node_to_delete {

            self.graph.nodes.retain(|n| n.id != del_id);
            self.graph
                .connections
                .retain(|c| c.from_node != del_id && c.to_node != del_id);
            if self.selected_node == Some(del_id) {
                self.selected_node = None;
            }
        }

        // Catalog Modal / Popup
        if self.catalog_open {
            self.render_catalog_window(ui, canvas_rect);
        }

        // Save Recipe Modal
        if self.save_dialog_open {
            self.render_save_window(ui);
        }

        action
    }

    fn render_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        slot_mgr: &SlotManager,
        action: &mut Option<CanvasAction>,
    ) {
        ui.horizontal(|ui| {
            // Add Node button
            if ui
                .button(
                    egui::RichText::new("＋ Add Node")
                        .color(Color32::from_rgb(241, 245, 249))
                        .strong(),
                )
                .clicked()
            {
                self.catalog_open = true;
                self.catalog_search.clear();
            }

            ui.separator();

            // Recipe Selector Combobox
            ui.label(
                egui::RichText::new("Recipe:")
                    .color(Color32::from_rgb(148, 163, 184))
                    .small(),
            );

            egui::ComboBox::from_id_source("recipe_combobox")
                .selected_text(
                    egui::RichText::new(&self.selected_recipe_name)
                        .color(Color32::from_rgb(168, 85, 247))
                        .strong(),
                )
                .show_ui(ui, |ui| {
                    for r in &self.available_recipes {
                        let is_sel = r.name == self.selected_recipe_name;
                        if ui.selectable_label(is_sel, &r.name).clicked() {
                            self.selected_recipe_name = r.name.clone();
                            self.graph = r.graph.clone();
                            self.status_message =
                                Some((format!("Loaded recipe '{}'", r.name), false));
                        }
                    }
                });

            // Save Recipe button
            if ui
                .button(
                    egui::RichText::new("💾 Save Recipe")
                        .color(Color32::from_rgb(203, 213, 225)),
                )
                .clicked()
            {
                self.save_name = self.selected_recipe_name.clone();
                self.save_dialog_open = true;
            }

            // Clear button
            if ui
                .button(
                    egui::RichText::new("🗑 Clear")
                        .color(Color32::from_rgb(203, 213, 225)),
                )
                .clicked()
            {
                self.graph.nodes.clear();
                self.graph.connections.clear();
                self.node_previews.clear();
                self.status_message = Some(("Canvas cleared".to_string(), false));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // RUN PIPELINE BUTTON (Hero purple accent)
                let run_btn = egui::Button::new(
                    egui::RichText::new("▶  RUN PIPELINE")
                        .color(Color32::WHITE)
                        .strong(),
                )
                .fill(Color32::from_rgb(147, 51, 234))
                .min_size(Vec2::new(140.0, 26.0));

                if ui.add(run_btn).clicked() {
                    if let Some(res) = self.run_current_graph(slot_mgr) {
                        *action = Some(CanvasAction::RunPipeline);
                        if let Some(item) = res.left_hand_item {
                            *action = Some(CanvasAction::AppliedResult {
                                item_title: item.title,
                            });
                        }
                    }
                }

                // Status Message badge
                if let Some((ref msg, is_err)) = self.status_message {
                    let col = if is_err {
                        Color32::from_rgb(239, 68, 68)
                    } else {
                        Color32::from_rgb(34, 197, 94)
                    };
                    ui.label(egui::RichText::new(msg).color(col).small().strong());
                }
            });
        });
    }

    fn render_catalog_window(&mut self, ui: &mut egui::Ui, parent_rect: Rect) {
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
                    .fill(Color32::from_rgb(24, 32, 47))
                    .stroke(Stroke::new(1.5, Color32::from_rgb(99, 102, 241)))
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(14.0))
                    .show(ui, |ui| {
                        ui.set_width(modal_w);
                        ui.set_height(modal_h);

                        ui.horizontal(|ui| {
                            ui.heading(
                                egui::RichText::new("Add Node to Canvas")
                                    .color(Color32::from_rgb(168, 85, 247))
                                    .strong(),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("✕").clicked() {
                                    self.catalog_open = false;
                                }
                            });
                        });

                        ui.add_space(8.0);

                        // Search box (autofocused)
                        let resp = ui.add_sized(
                            [ui.available_width(), 28.0],
                            egui::TextEdit::singleline(&mut self.catalog_search)
                                .hint_text("Search nodes or tags (e.g. caption, frame, normalize, python)..."),
                        );
                        resp.request_focus();

                        ui.add_space(6.0);

                        // Category Pills
                        ui.horizontal(|ui| {
                            let categories = [
                                "All", "Source", "Text", "Image", "Audio", "Files", "Script", "Sink",
                            ];
                            for &cat in &categories {
                                let is_sel = self.catalog_category == cat;
                                if ui.selectable_label(is_sel, cat).clicked() {
                                    self.catalog_category = cat.to_string();
                                }
                            }
                        });

                        ui.add_space(8.0);
                        ui.separator();

                        // Filtered Node List
                        let filtered = search_templates(
                            &self.catalog_search,
                            Some(&self.catalog_category),
                        );

                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                if filtered.is_empty() {
                                    ui.label(
                                        egui::RichText::new("No matching nodes found.")
                                            .color(Color32::from_rgb(148, 163, 184))
                                            .italics(),
                                    );
                                } else {
                                    for template in filtered {
                                        let frame = egui::Frame::none()
                                            .fill(Color32::from_rgb(30, 41, 59))
                                            .rounding(4.0)
                                            .inner_margin(egui::Margin::symmetric(10.0, 6.0));

                                        frame.show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(template.name)
                                                        .color(Color32::WHITE)
                                                        .strong(),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!("[{}]", template.category))
                                                        .color(Color32::from_rgb(148, 163, 184))
                                                        .small(),
                                                );
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        if ui
                                                            .button(
                                                                egui::RichText::new("+ Add")
                                                                    .color(Color32::from_rgb(99, 102, 241)),
                                                            )
                                                            .clicked()
                                                        {
                                                            let spawn_pos = [
                                                                -self.graph.pan[0] + 200.0,
                                                                -self.graph.pan[1] + 150.0,
                                                            ];
                                                            let new_node = instantiate_template(
                                                                template, spawn_pos,
                                                            );
                                                            self.graph.nodes.push(new_node);
                                                            self.catalog_open = false;
                                                        }
                                                    },
                                                );
                                            });
                                            ui.label(
                                                egui::RichText::new(template.description)
                                                    .color(Color32::from_rgb(148, 163, 184))
                                                    .small(),
                                            );
                                        });
                                        ui.add_space(4.0);
                                    }
                                }
                            });
                    });
            });
    }

    fn render_save_window(&mut self, ui: &mut egui::Ui) {
        egui::Window::new("Save Pipeline Recipe")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ui.ctx(), |ui| {
                ui.set_width(320.0);
                ui.label("Enter recipe name:");
                ui.text_edit_singleline(&mut self.save_name);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() && !self.save_name.trim().is_empty() {
                        let recipe = Recipe {
                            name: self.save_name.trim().to_string(),
                            description: "Custom user recipe".to_string(),
                            tags: vec!["custom".to_string()],
                            graph: self.graph.clone(),
                        };
                        if let Ok(path) = save_recipe(&recipe) {
                            self.status_message = Some((
                                format!("Saved recipe to {}", path.display()),
                                false,
                            ));
                            self.reload_recipes();
                            self.selected_recipe_name = recipe.name;
                        }
                        self.save_dialog_open = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.save_dialog_open = false;
                    }
                });
            });
    }
}

fn render_grid(painter: &egui::Painter, rect: Rect, pan: [f32; 2], zoom: f32) {
    // Dark blueprint background
    painter.rect_filled(rect, Rounding::ZERO, Color32::from_rgb(15, 23, 42));

    let grid_size = 32.0 * zoom;
    if grid_size > 8.0 {
        let offset_x = (pan[0] % grid_size + grid_size) % grid_size;
        let offset_y = (pan[1] % grid_size + grid_size) % grid_size;

        let dot_color = Color32::from_rgba_unmultiplied(51, 65, 85, 120);

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

fn draw_bezier_cable(
    painter: &egui::Painter,
    p0: Pos2,
    p3: Pos2,
    color: Color32,
    zoom: f32,
) {
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
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    node: &mut NodeInstance,
    rect: Rect,
    is_selected: bool,
    node_to_delete: &mut Option<Uuid>,
) {
    let border_col = if is_selected {
        Color32::from_rgb(192, 132, 252)
    } else {
        Color32::from_rgb(51, 65, 85)
    };

    // Card body background
    painter.rect_filled(
        rect,
        Rounding::same(8.0 * zoom),
        Color32::from_rgb(30, 41, 59),
    );
    painter.rect_stroke(
        rect,
        Rounding::same(8.0 * zoom),
        Stroke::new(if is_selected { 2.0 } else { 1.2 }, border_col),
    );

    // Category Header banner
    let header_h = 26.0 * zoom;
    let header_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_h));
    let cat_color = get_node_category_color(&node.kind);
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

    // Node Title
    painter.text(
        header_rect.left_center() + Vec2::new(8.0 * zoom, 0.0),
        egui::Align2::LEFT_CENTER,
        &node.title,
        egui::FontId::proportional(12.0 * zoom),
        Color32::WHITE,
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
    let close_col = if close_resp.hovered() {
        Color32::from_rgb(239, 68, 68)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 180)
    };
    painter.text(
        close_rect.center(),
        egui::Align2::CENTER_CENTER,
        "×",
        egui::FontId::monospace(14.0 * zoom),
        close_col,
    );

    // Render Input Pins on Left Edge
    for (idx, pin) in node.inputs.iter().enumerate() {
        let pin_pos = get_pin_pos_by_idx(rect, node.inputs.len(), idx, false, zoom);
        render_socket(ui, painter, pin_pos, node.id, &pin.name, pin.port_type, false, zoom, dragging_from);

        painter.text(
            pin_pos + Vec2::new(10.0 * zoom, 0.0),
            egui::Align2::LEFT_CENTER,
            &pin.name,
            egui::FontId::proportional(10.5 * zoom),
            Color32::from_rgb(203, 213, 225),
        );
    }

    // Render Output Pins on Right Edge
    for (idx, pin) in node.outputs.iter().enumerate() {
        let pin_pos = get_pin_pos_by_idx(rect, node.outputs.len(), idx, true, zoom);
        render_socket(ui, painter, pin_pos, node.id, &pin.name, pin.port_type, true, zoom, dragging_from);

        painter.text(
            pin_pos + Vec2::new(-10.0 * zoom, 0.0),
            egui::Align2::RIGHT_CENTER,
            &pin.name,
            egui::FontId::proportional(10.5 * zoom),
            Color32::from_rgb(203, 213, 225),
        );
    }

    // Inline editable properties inside card
    if !node.props.is_empty() {
        let content_min = rect.min + Vec2::new(12.0, header_h + 8.0);
        let content_max = rect.max - Vec2::new(12.0, 10.0);
        let content_rect = Rect::from_min_max(content_min, content_max);

        let mut child_ui = ui.child_ui(content_rect, egui::Layout::top_down(egui::Align::Min));
        child_ui.set_clip_rect(content_rect);

        for (key, val) in node.props.iter_mut() {
            child_ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(key.replace('_', " "))
                        .color(Color32::from_rgb(148, 163, 184))
                        .small(),
                );
                if key == "script" || key == "command" {
                    ui.add(
                        egui::TextEdit::multiline(val)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(2)
                            .desired_width(120.0 * zoom),
                    );
                } else {
                    ui.add(
                        egui::TextEdit::singleline(val)
                            .desired_width(90.0 * zoom),
                    );
                }
            });
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
            format!("✔ {}", preview_short),
            egui::FontId::proportional(9.5 * zoom),
            Color32::from_rgb(74, 222, 128),
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
) {
    let radius = 5.5 * zoom;
    let socket_rect = Rect::from_center_size(pos, Vec2::splat(radius * 2.5));
    let resp = ui.interact(
        socket_rect,
        ui.make_persistent_id((node_id, pin_name, is_output)),
        egui::Sense::click_and_drag(),
    );

    if resp.drag_started() {
        *dragging_from = Some((node_id, pin_name.to_string(), is_output));
    }

    let rgba = port_type.color_rgba();
    let base_col = Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);

    if resp.hovered() {
        painter.circle_filled(pos, radius * 1.3, Color32::WHITE);
        painter.circle_stroke(pos, radius * 1.5, Stroke::new(2.0, base_col));
    } else {
        painter.circle_filled(pos, radius, base_col);
        painter.circle_stroke(
            pos,
            radius,
            Stroke::new(1.2, Color32::from_rgb(15, 23, 42)),
        );
    }
}

fn world_to_screen(canvas_rect: Rect, pan: [f32; 2], zoom: f32, pt: Pos2) -> Pos2 {
    canvas_rect.min + Vec2::new(pan[0] + pt.x * zoom, pan[1] + pt.y * zoom)
}

fn estimate_node_height(node: &NodeInstance) -> f32 {
    let max_pins = node.inputs.len().max(node.outputs.len());
    let pin_h = max_pins as f32 * 24.0;
    let props_h = node.props.len() as f32 * 32.0;
    (40.0 + pin_h + props_h).max(75.0)
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
        let pins = if is_output { &node.outputs } else { &node.inputs };
        if let Some(idx) = pins.iter().position(|p| p.name.eq_ignore_ascii_case(pin_name)) {
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
    total_pins: usize,
    idx: usize,
    is_output: bool,
    zoom: f32,
) -> Pos2 {
    let header_h = 26.0 * zoom;
    let available_h = (node_rect.height() - header_h).max(30.0);
    let step = available_h / (total_pins as f32 + 1.0);
    let y = node_rect.min.y + header_h + step * (idx as f32 + 1.0);
    let x = if is_output { node_rect.max.x } else { node_rect.min.x };
    Pos2::new(x, y)
}

fn get_node_category_color(kind: &NodeKind) -> Color32 {
    match kind {
        NodeKind::LeftHandSource
        | NodeKind::RightHandSource
        | NodeKind::SlotSource(_)
        | NodeKind::ConstantText
        | NodeKind::ConstantNumber => Color32::from_rgb(59, 130, 246), // Blue

        NodeKind::TextTrim
        | NodeKind::TextSlugify
        | NodeKind::TextCase
        | NodeKind::TextRegexReplace
        | NodeKind::TextSplitLines
        | NodeKind::TextJoin
        | NodeKind::TextJsonQuery
        | NodeKind::TextBase64 => Color32::from_rgb(245, 158, 11), // Amber

        NodeKind::ImagePadFrame
        | NodeKind::ImageAddTitle
        | NodeKind::ImageCrop
        | NodeKind::ImageResize
        | NodeKind::ImageConvertWebp
        | NodeKind::ImageRotate
        | NodeKind::ImageStitch
        | NodeKind::ImageStripExif => Color32::from_rgb(6, 182, 212), // Cyan

        NodeKind::AudioNormalize
        | NodeKind::AudioTrimSilence
        | NodeKind::AudioAdjustGain
        | NodeKind::AudioConcat => Color32::from_rgb(168, 85, 247), // Purple

        NodeKind::FileSha256 | NodeKind::FileZip | NodeKind::FileExtractName => {
            Color32::from_rgb(236, 72, 153) // Pink
        }

        NodeKind::PythonScript | NodeKind::BashCommand => Color32::from_rgb(16, 185, 129), // Emerald

        NodeKind::SetLeftHand
        | NodeKind::SetRightHand
        | NodeKind::SaveToDisk
        | NodeKind::PasteToApp => Color32::from_rgb(99, 102, 241), // Indigo
    }
}
