use super::*;

impl WorkbenchApp {
    pub(crate) fn render_text_smart_preview_actions(&mut self, ui: &mut egui::Ui) {
        let first_text_item = self.selected_slots.iter().find_map(|s| {
            let item = self.state.get(*s)?;
            match &item.payload {
                ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                    Some(std::borrow::Cow::Borrowed(text.as_str()))
                }
                _ => item.to_clipboard_text().map(std::borrow::Cow::Owned),
            }
        });
        if let Some(text) =
            first_text_item.filter(|text| text.len() <= MAX_INLINE_TEXT_ROUTING_BYTES)
        {
            let trimmed = text.trim();
            if let Some(conv) = detect_unit_conversion(trimmed) {
                ui.add_space(crate::ui_ext::SPACE_SM);
                let label = conv.summary_line();
                let btn =
                    contextual_palette_action_button(Some(Icon::Swap), &label, &self.ui_palette);
                if ui
                    .add(btn)
                    .on_hover_text("Click to copy converted unit")
                    .clicked()
                {
                    ui.output_mut(|o| o.copied_text = conv.primary_conversion.clone());
                    self.status_msg = Some(format!("Copied: {}", conv.primary_conversion));
                }
            } else if let Some(net) = detect_network_preview(trimmed) {
                ui.add_space(crate::ui_ext::SPACE_SM);
                let (label, net_icon) = match &net {
                    NetworkPreview::Ip { ip, rfc_class, .. } => {
                        (format!("{} ({})", ip, rfc_class), Icon::Link)
                    }
                    NetworkPreview::Mac {
                        formatted, vendor, ..
                    } => (format!("{} ({})", formatted, vendor), Icon::Zap),
                };
                let btn =
                    contextual_palette_action_button(Some(net_icon), &label, &self.ui_palette);
                if ui
                    .add(btn)
                    .on_hover_text("Network address info (Click to copy summary)")
                    .clicked()
                {
                    let lines = net.summary_lines().join("\n");
                    ui.output_mut(|o| o.copied_text = lines.clone());
                    self.status_msg = Some("Copied network info".to_string());
                }
            } else if let Some(ts) = detect_timestamp(trimmed) {
                ui.add_space(crate::ui_ext::SPACE_SM);
                let label = format!("{} ({})", ts.human_readable, ts.relative);
                let btn = contextual_palette_action_button(
                    Some(Icon::Calendar),
                    &label,
                    &self.ui_palette,
                );
                if ui
                    .add(btn)
                    .on_hover_text(format!(
                        "UTC: {}\nEpoch: {}\nClick to copy ISO 8601",
                        ts.utc_iso, ts.epoch_sec
                    ))
                    .clicked()
                {
                    ui.output_mut(|o| o.copied_text = ts.utc_iso.clone());
                    self.status_msg = Some(format!("Copied ISO 8601: {}", ts.utc_iso));
                }
            }
        }
    }

    pub(crate) fn render_workbench_header(&mut self, ctx: &egui::Context) {
        // Top Header Panel: Mirror of Trofeo Screen + Tabs
        let header_bg = hex_to_c32(&self.ui_palette.colors.canvas);
        let brand_color = hex_to_c32(&self.ui_palette.left_hand_border);
        let sub_color = hex_to_c32(&self.ui_palette.colors.text_secondary);

        egui::TopBottomPanel::top("header_panel")
            .frame(
                egui::Frame::none()
                    .fill(header_bg)
                    .inner_margin(egui::Margin::symmetric(crate::ui_ext::PANEL_INSET, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(
                        egui::RichText::new("LITTLE SCREEN CLIP")
                            .color(brand_color)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("— Ambient Dual-Hand Live Mirror").color(sub_color),
                    );

                    let appearance_label = match self.config.workbench.appearance {
                        WorkbenchAppearance::Dark => "Light mode [F4/F6]",
                        WorkbenchAppearance::Light => "Dark mode [F4/F6]",
                    };
                    if ui
                        .add(ui_ext::secondary_button(
                            appearance_label,
                            hex_to_c32(&self.ui_palette.colors.surface_panel),
                            &self.ui_palette,
                        ))
                        .on_hover_text(
                            "Switch the workbench and little screen to the other color mode",
                        )
                        .clicked()
                    {
                        self.toggle_appearance(ctx);
                    }
                    if self.spreadsheet_load_job.is_some() {
                        ui.spinner();
                        ui.label("Opening spreadsheet…");
                    }
                });

                // All workbench sections remain one click away on compact screens.
                ui.horizontal_wrapped(|ui| {
                    let is_auto = self.view == WorkbenchView::AutoActions;
                    let auto_btn = ui_ext::mode_tab_button_icon(
                        Some(Icon::Settings),
                        "Auto-Actions [F3]",
                        is_auto,
                        &self.ui_palette,
                    );
                    if ui.add(auto_btn).clicked() {
                        self.view = if is_auto {
                            WorkbenchView::Palette
                        } else {
                            WorkbenchView::AutoActions
                        };
                    }

                    let is_pal = self.view == WorkbenchView::Palette;
                    let pal_tab = ui_ext::mode_tab_button_icon(
                        Some(Icon::Search),
                        "Quick Palette [F2]",
                        is_pal,
                        &self.ui_palette,
                    );
                    if ui.add(pal_tab).clicked() {
                        self.view = WorkbenchView::Palette;
                    }

                    let is_node = self.view == WorkbenchView::NodeMachine;
                    let node_tab = ui_ext::mode_tab_button_icon(
                        Some(Icon::NodeTree),
                        "Big Node Machine [F1]",
                        is_node,
                        &self.ui_palette,
                    );
                    if ui.add(node_tab).clicked() {
                        self.view = WorkbenchView::NodeMachine;
                    }
                });

                if self.view == WorkbenchView::Palette {
                    ui.add_space(crate::ui_ext::SPACE_SM);
                    if self.dirty_mirror || self.mirror_texture.is_none() {
                        self.update_mirror_texture(ctx);
                    }
                    let mirror_action = ScreenMirrorWidget::new(
                        self.mirror_texture.as_ref(),
                        self.config.slots.left_priors,
                        &mut self.dragging_slot,
                        &self.selected_slots,
                    )
                    .show(ui, &self.ui_palette);
                    self.handle_mirror_action(mirror_action);
                }
            });
    }

    pub(crate) fn update_mirror_texture(&mut self, ctx: &egui::Context) {
        let mut render_state = self.state.clone();
        render_state.selected = self.selected_slots.clone();

        if let Ok(pixmap) = self.renderer.render_pixmap(&render_state) {
            let pixels: Vec<egui::Color32> = pixmap
                .data()
                .chunks_exact(4)
                .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
                .collect();

            let color_image = egui::ColorImage {
                size: [SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize],
                pixels,
            };

            self.mirror_texture =
                Some(ctx.load_texture("ambient-mirror", color_image, egui::TextureOptions::LINEAR));
            self.dirty_mirror = false;
        }
    }
}
