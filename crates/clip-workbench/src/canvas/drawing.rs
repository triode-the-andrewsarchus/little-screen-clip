use super::*;

pub(super) fn render_grid(
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

pub(super) fn eval_cubic_bezier(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;

    let x = uuu * p0.x + 3.0 * uu * t * p1.x + 3.0 * u * tt * p2.x + ttt * p3.x;
    let y = uuu * p0.y + 3.0 * uu * t * p1.y + 3.0 * u * tt * p2.y + ttt * p3.y;
    Pos2::new(x, y)
}

pub(super) struct SnappedTargetInfo {
    pub(super) node_id: Uuid,
    pub(super) node_title: String,
    pub(super) pin_name: String,
    pub(super) port_type: PortType,
    pub(super) pin_pos: Pos2,
    pub(super) is_compatible: bool,
    pub(super) from_port_type: Option<PortType>,
}

pub(super) fn draw_active_cable(
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

pub(super) fn draw_bezier_cable(
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

pub(super) fn render_node_card(
    zoom: f32,
    node_previews: &HashMap<Uuid, String>,
    dragging_from: &mut Option<(Uuid, String, bool)>,
    socket_to_disconnect: &mut Option<(Uuid, String, bool)>,
    script_to_edit: &mut Option<Uuid>,
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
                if is_py {
                    "Inline Python"
                } else {
                    "Inline Bash"
                }
                .to_owned()
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
                    .on_hover_text("Edit script in the side panel")
                    .clicked()
                {
                    *script_to_edit = Some(node.id);
                }
            });

            child_ui.add_space(2.0);

            let edit_btn = crate::ui_ext::action_button_icon(
                Some(Icon::Pencil),
                "Edit Script",
                hex_to_c32(&palette.harmonies.base),
                palette,
            )
            .min_size(Vec2::new(child_ui.available_width(), 20.0 * zoom));

            if child_ui.add(edit_btn).clicked() {
                *script_to_edit = Some(node.id);
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

pub(super) fn render_socket(
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

pub(super) fn world_to_screen(canvas_rect: Rect, pan: [f32; 2], zoom: f32, pt: Pos2) -> Pos2 {
    canvas_rect.min + Vec2::new(pan[0] + pt.x * zoom, pan[1] + pt.y * zoom)
}

pub(super) fn screen_to_world(canvas_rect: Rect, pan: [f32; 2], zoom: f32, pt: Pos2) -> Pos2 {
    let rel = pt - canvas_rect.min;
    Pos2::new((rel.x - pan[0]) / zoom, (rel.y - pan[1]) / zoom)
}

pub(super) fn estimate_node_height(node: &NodeInstance) -> f32 {
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

pub(super) fn get_pin_pos(
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

pub(super) fn get_pin_pos_by_idx(
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

pub(super) fn get_node_category_color(kind: &NodeKind, palette: &AppPalette) -> Color32 {
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
