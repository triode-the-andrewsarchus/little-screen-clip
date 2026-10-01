use super::*;

impl NodeCanvasState {
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

        let canvas_size = self.render_script_editor(ui, palette);
        // Main Canvas Interactive Area
        let (canvas_rect, response) =
            ui.allocate_exact_size(canvas_size, egui::Sense::click_and_drag());

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
        let mut script_to_edit = None;
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
                script_to_edit = Some(node_id);
            }

            // Render Card Frame
            let is_selected = self.selected_node == Some(node_id);
            render_node_card(
                zoom,
                &self.node_previews,
                &mut self.dragging_from,
                &mut socket_to_disconnect,
                &mut script_to_edit,
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

        if let Some(id) = script_to_edit {
            self.selected_node = Some(id);
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

    /// Render the selected script beside the graph and return the remaining canvas size.
    pub(super) fn render_script_editor(&mut self, ui: &mut egui::Ui, palette: &AppPalette) -> Vec2 {
        // Reserve a side editor only for a selected script node.
        let editor_node = self.selected_node.and_then(|id| {
            self.graph.nodes.iter().position(|node| {
                node.id == id && matches!(node.kind, NodeKind::PythonScript | NodeKind::BashCommand)
            })
        });
        let mut canvas_size = ui.available_size();
        if let Some(index) = editor_node {
            let editor_width = (canvas_size.x * 0.46)
                .clamp(300.0, 650.0)
                .min(canvas_size.x * 0.6);
            canvas_size.x -= editor_width + 8.0;
            let full_rect = ui.available_rect_before_wrap();
            let editor_rect = Rect::from_min_max(
                Pos2::new(full_rect.min.x + canvas_size.x + 8.0, full_rect.min.y),
                full_rect.max,
            );
            let mut editor_ui = ui.child_ui(editor_rect, egui::Layout::top_down(egui::Align::Min));
            editor_ui.set_clip_rect(editor_rect.intersect(ui.clip_rect()));
            let node = &mut self.graph.nodes[index];
            if self
                .script_editors
                .get(&node.id)
                .is_some_and(|editor| !editor.matches_source(node))
            {
                self.script_editors.remove(&node.id);
            }
            let editor = self
                .script_editors
                .entry(node.id)
                .or_insert_with(|| crate::script_editor::ScriptEditor::load(node));
            editor.ui(&mut editor_ui, node, palette);
        }
        canvas_size
    }
}
