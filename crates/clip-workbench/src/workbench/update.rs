use super::*;

impl eframe::App for WorkbenchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply the fixed appearance to egui on the first frame or mode changes.
        if !self.appearance_applied {
            ui_appearance::apply_visuals(ctx, &self.ui_palette);
            self.appearance_applied = true;
        }

        self.handle_appearance_shortcut(ctx);

        while let Ok(result) = self.state_rx.try_recv() {
            match result {
                Ok(state) => {
                    self.state = state;
                    self.dirty_mirror = true;
                    self.mirror_error = None;
                    self.selected_action_index = 0;
                }
                Err(error) => self.set_status_message(format!("Could not load clipboard: {error}")),
            }
            self.state_loaded = true;
            if let Some(binary) = self.startup_comparison.take() {
                if binary {
                    self.open_binary_diff_viewer();
                } else {
                    self.open_diff_viewer();
                }
                if self.viewer.is_none() {
                    self.advance_startup();
                }
            }
            ctx.request_repaint();
        }
        self.poll_palette_ops(ctx);

        self.appearance_sync.poll(ctx);
        self.appearance_sync.render(ctx, &self.ui_palette);

        self.poll_rolling_editor_load(ctx);
        self.render_rolling_editor_status(ctx);
        if let Some((_, state)) = self.retained_image_session.as_mut()
            && state.poll_cached_jobs()
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        self.poll_image_load(ctx);
        self.poll_text_load(ctx);
        self.poll_office_document_load(ctx);
        if self.render_modal_viewers(ctx) {
            self.palette_search_needs_focus = true;
            return;
        }

        // Hold table-load jobs/results while another viewer is active. The receiver
        // stays available, but polling waits until the foreground viewer closes.
        self.poll_spreadsheet_load(ctx);
        if self.render_modal_viewers(ctx) {
            self.palette_search_needs_focus = true;
            return;
        }
        self.poll_midi_load(ctx);
        if self.render_modal_viewers(ctx) {
            self.palette_search_needs_focus = true;
            return;
        }

        if self.mirror_animated {
            self.dirty_mirror = true;
            ctx.request_repaint_after(Duration::from_millis(80));
        } else {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        self.handle_global_shortcuts(ctx);

        self.render_workbench_header(ctx);

        let mut host_action = None;

        if self.view == WorkbenchView::Palette
            && let Some(strip) = &mut self.rolling_audio_strip
        {
            egui::TopBottomPanel::bottom("rolling_audio_strip_panel")
                .min_height(100.0)
                .frame(
                    egui::Frame::none()
                        .fill(crate::hex_to_c32(&self.ui_palette.colors.canvas))
                        .inner_margin(egui::Margin::symmetric(
                            crate::ui_ext::PANEL_INSET,
                            crate::ui_ext::FOOTER_INSET_Y,
                        ))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            crate::hex_to_c32(&self.ui_palette.colors.surface_panel),
                        )),
                )
                .show(ctx, |ui| {
                    strip.render_ui(ui, ctx, &self.ui_palette, &mut host_action);
                });
        }

        if let Some(host_action) = host_action {
            self.apply_rolling_audio_action(host_action, ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(hex_to_c32(&self.ui_palette.colors.text_primary));

            show_workbench_footer(
                ui,
                self.view,
                self.status_msg.as_deref(),
                &self.ui_palette,
            );

            if self.view != WorkbenchView::Palette {
                self.palette_search_needs_focus = true;
            }

            match self.view {
                WorkbenchView::Palette => {
                    let filtered = self.filtered_actions();
                    if self.palette_ops_job.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("Preparing actions…"); }); }
                    let text_preview_action = self.check_inspector_text_preview();

                    // Selected slots banner & shortcuts
                    let slot_labels: Vec<&str> = self.selected_slots.iter().map(|s| s.label()).collect();
                    let first_audio_item = self.selected_slots.iter().find_map(|s| {
                        self.state.get(*s).and_then(|it| (!is_large_text_payload(it)).then(|| it.audio_path()).flatten().map(|p| (p, it.title.clone())))
                    });
                    let first_image_slot = self.selected_slots.iter().copied().find(|slot| {
                        self.state
                            .get(*slot)
                            .is_some_and(is_image_preview_action_item)
                    });

                    let mut open_diff_requested = false;
                    let mut open_spreadsheet_requested = None;
                    let mut open_midi_requested = None;

                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Selected ({}): {}", self.selected_slots.len(), slot_labels.join(" + ")))
                                .color(hex_to_c32(&self.ui_palette.harmonies.base))
                                .strong(),
                        );
                        ui.label(
                                egui::RichText::new(" • [Space] Hands  |  [1] L+L1  |  [2] L+L2  |  [3] 3 Left  |  [0] L only")
                                    .color(hex_to_c32(&self.ui_palette.colors.text_muted))
                                    .size(BODY_FONT_SIZE),
                        );

                        if let Some((path, title)) = first_audio_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let audio_btn = contextual_palette_action_button(
                                Some(Icon::Audio),
                                "Preview & Slice Audio",
                                &self.ui_palette,
                            );
                            if ui.add(audio_btn).clicked() {
                                self.replace_viewer(Some(ViewerSession::Audio(AudioPreviewState::new(path, title).with_editor(self.config.audio.editor.clone()))));
                            }
                        }

                        if let Some(slot) = first_image_slot {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let img_btn = contextual_palette_action_button(
                                Some(Icon::Image),
                                "Preview & Edit Image",
                                &self.ui_palette,
                            );
                            if ui.add(img_btn).clicked() {
                                let item = self.state.get(slot).unwrap().clone();
                                self.start_image_item_load(&item);
                            }
                        }

                        let has_two_images = if self.selected_slots.len() >= 2 {
                            let img_count = self
                                .selected_slots
                                .iter()
                                .filter_map(|s| self.state.get(*s))
                                .filter(|it| !is_large_text_payload(it))
                                .filter(|it| resolve_diff_target(it).map(|t| t.is_image()).unwrap_or(false))
                                .count();
                            img_count >= 2
                                || self.selected_slots.iter().any(|s| {
                                    self.state
                                        .get(*s)
                                        .filter(|it| !is_large_text_payload(it))
                                        .and_then(|it| resolve_diff_pair(it))
                                        .map(|(t1, t2)| t1.is_image() && t2.is_image())
                                        .unwrap_or(false)
                                  })
                        } else if self.selected_slots.len() == 1 {
                            self.state
                                .get(self.selected_slots[0])
                                .filter(|it| !is_large_text_payload(it))
                                .and_then(|it| resolve_diff_pair(it))
                                .map(|(t1, t2)| t1.is_image() && t2.is_image())
                                .unwrap_or(false)
                        } else {
                            false
                        };
                        if has_two_images {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let diff_btn = contextual_palette_action_button(
                                Some(Icon::Diff),
                                "Compare Images",
                                &self.ui_palette,
                            );
                            if ui.add(diff_btn).on_hover_text("Open interactive visual diff comparing the 2 images").clicked() {
                                open_diff_requested = true;
                            }
                        }

                    let first_pdf_item = self.selected_slots.iter().find_map(|s| {
                            self.state
                                .get(*s)
                                .filter(|item| is_pdf_clip_item(item))
                                .and_then(bounded_item_file_path)
                        });
                        let first_spreadsheet_slot = self
                            .inspector_text_preview_cache
                            .as_ref()
                            .and_then(|cache| cache.spreadsheet_slot);
                        let first_midi_item = self.selected_slots.iter().find_map(|s| {
                            let it = self.state.get(*s)?;
                            if is_large_text_payload(it) {
                                return None;
                            }
                            if let Some(p) = bounded_item_file_path(it).filter(|p| {
                                p.extension().and_then(|e| e.to_str()).map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi")).unwrap_or(false)
                            }) {
                                Some(p)
                            } else if let Some(txt) = it.to_clipboard_text() {
                                let p = std::path::Path::new(txt.trim());
                                if p.is_file() && p.extension().and_then(|e| e.to_str()).map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi")).unwrap_or(false) {
                                    Some(p.to_path_buf())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });
                        let first_3d_model_item = self.selected_slots.iter().find_map(|slot| {
                            bounded_item_file_path(self.state.get(*slot)?)
                                .filter(|path| is_3d_model_file(path) || little_clip_scene::is_splat_file(path))
                        });

                        if let Some(pdf_path) = first_pdf_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let pdf_btn = contextual_palette_action_button(
                                Some(Icon::Pdf),
                                "Edit PDF",
                                &self.ui_palette,
                            );
                            if ui.add(pdf_btn).on_hover_text("Open PDF Page Editor (Delete / Rotate pages)").clicked() {
                                self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(pdf_path))));
                            }
                        }

                        if let Some(slot) = first_spreadsheet_slot {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let sheet_btn = contextual_palette_action_button(
                                Some(Icon::Spreadsheet),
                                if self.spreadsheet_load_job.is_some() { "Opening Spreadsheet…" } else { "View Spreadsheet" },
                                &self.ui_palette,
                            );
                            let sheet_response = ui_ext::on_hover_text_with_disabled_reason(
                                ui.add_enabled(self.spreadsheet_load_job.is_none(), sheet_btn),
                                "Open full multi-sheet X/Y scrollable table",
                                "Wait for the current spreadsheet load to finish.",
                            );
                            if sheet_response.clicked() {
                                open_spreadsheet_requested = Some(slot);
                            }
                        }

                        if let Some(midi_path) = first_midi_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let midi_btn = contextual_palette_action_button(
                                Some(Icon::Midi),
                                if self.midi_load_job.is_some() { "Opening MIDI…" } else { "MIDI Piano Roll" },
                                &self.ui_palette,
                            );
                            let midi_response = ui_ext::on_hover_text_with_disabled_reason(
                                ui.add_enabled(self.midi_load_job.is_none(), midi_btn),
                                "Open interactive MIDI piano roll visualizer & synth audio",
                                "Wait for the current MIDI load to finish.",
                            );
                            if midi_response.clicked() {
                                open_midi_requested = Some(midi_path);
                            }
                        }

                        if let Some(model_path) = first_3d_model_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let ext = model_path.extension().and_then(|s| s.to_str()).unwrap_or("3D").to_ascii_uppercase();
                            let btn_text = format!("3D {} Model", ext);
                            let model_btn = contextual_palette_action_button(
                                Some(Icon::Model3d),
                                &btn_text,
                                &self.ui_palette,
                            );
                            if ui.add(model_btn).on_hover_text("Open interactive 3D orbit viewer (pan, zoom, rotate)").clicked() {
                                self.open_slot_file_preview(&model_path, false);
                            }
                        }

                        // Smart text previews: Unit conversions, Network (IP/MAC), and Timestamps
                        self.render_text_smart_preview_actions(ui);

                        if let Some((slot, file_p, is_md)) = text_preview_action.clone() {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let (label, txt_icon) = if is_md {
                                ("Preview Markdown", Icon::Book)
                            } else {
                                ("View Text / Code", Icon::Text)
                            };
                            let txt_btn = contextual_palette_action_button(
                                Some(txt_icon),
                                label,
                                &self.ui_palette,
                            );
                            if ui.add(txt_btn).on_hover_text("Open text or Markdown preview (syntax highlighting, line wrapping, search, and editor export)").clicked() {
                                if let Some(p) = file_p {
                                    self.start_text_file_load(p);
                                } else if let Some(txt) = self.state.get(slot).and_then(ClipItem::to_clipboard_text) {
                                    self.start_text_preview_load(txt, None);
                                }
                            }
                        }

                    });

                    if open_diff_requested {
                        self.open_diff_viewer();
                    }
                    if let Some(slot) = open_spreadsheet_requested {
                        self.open_spreadsheet_item(slot);
                    }
                    if let Some(path) = open_midi_requested {
                        self.start_midi_load(path);
                    }

                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                    // Focus on entry, then leave keyboard focus under the user's control.
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Find action")
                                .color(hex_to_c32(&self.ui_palette.harmonies.base))
                                .strong(),
                        );
                        let audio_region_active = self.is_audio_region_active();
                        let mut text_edit = egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Search actions or type / for recipes…");
                        if audio_region_active {
                            text_edit = text_edit.interactive(false);
                        }
                        let response = ui.add_sized(
                            [ui.available_width() - 20.0, ui_ext::CONTROL_HEIGHT],
                            text_edit,
                        );
                        let activate_from_audio = response.clicked()
                            || (response.hovered()
                                && ctx.input(|i| i.pointer.primary_clicked()));
                        if audio_region_active && activate_from_audio {
                                if let Some(strip) = &mut self.rolling_audio_strip {
                                    strip.selected_region = None;
                                    strip.stop();
                                }
                        }
                        update_palette_search_focus(
                            ctx,
                            &response,
                            &mut self.palette_search_needs_focus,
                            audio_region_active,
                            activate_from_audio,
                        );
                    });

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Filtered Action List filling available vertical height
                    let avail_h = ui.available_height().max(0.0);
                    egui::ScrollArea::vertical().max_height(avail_h).show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new(if !self.state_loaded { "Loading clipboard…" } else if self.palette_ops_job.is_some() { "Preparing actions…" } else { "No matching actions or recipes found." })
                                    .color(hex_to_c32(&self.ui_palette.colors.text_muted))
                                    .italics(),
                            );
                        } else {
                            for (idx, act) in filtered.iter().enumerate() {
                                let is_selected = idx == self.selected_action_index;
                                let bg = if is_selected {
                                    hex_to_c32(&self.ui_palette.colors.surface_panel)
                                } else {
                                    hex_to_c32(&self.ui_palette.colors.surface_recessed)
                                };

                                let frame = egui::Frame::none()
                                    .fill(bg)
                                    .inner_margin(egui::Margin::symmetric(
                                        crate::ui_ext::BUTTON_PADDING_X,
                                        crate::ui_ext::BUTTON_PADDING_Y,
                                    ))
                                    .rounding(crate::ui_ext::CONTROL_CORNER_RADIUS)
                                    .stroke(egui::Stroke::new(1.0_f32, if is_selected {
                                        hex_to_c32(&self.ui_palette.harmonies.base)
                                    } else { egui::Color32::TRANSPARENT }));

                                frame.show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if is_selected {
                                            Icon::Play.ui(ui, ui_ext::CONTROL_ICON_SIZE,
                                                hex_to_c32(&self.ui_palette.harmonies.base));
                                        }
                                        let preferred_fg = if is_selected {
                                            hex_to_c32(&self.ui_palette.colors.text_primary)
                                        } else {
                                            hex_to_c32(&self.ui_palette.colors.text_secondary)
                                        };
                                        let label_color = contrast_text_c32(bg, Some(preferred_fg), &self.ui_palette);

                                        ui.label(egui::RichText::new(act.label()).color(label_color).strong());
                                        let cat_color = if act.category() == "Recipe" {
                                            hex_to_c32(&self.ui_palette.harmonies.triadic_1)
                                        } else {
                                            hex_to_c32(&self.ui_palette.colors.text_secondary)
                                        };
                                        ui.label(
                                            egui::RichText::new(format!("[{}]", act.category()))
                                                .color(cat_color)
                                                .size(BODY_FONT_SIZE)
                                                .strong(),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(
                                                egui::RichText::new(act.description())
                                                    .color(hex_to_c32(&self.ui_palette.colors.text_muted))
                                                    .size(BODY_FONT_SIZE),
                                            );
                                        });
                                    });
                                });
                                ui.add_space(2.0);
                            }
                        }
                    });

                }
                WorkbenchView::NodeMachine => {
                    let action = self.canvas_state.ui(ui, &self.ui_palette);
                    if let Some(CanvasAction::RunPipeline { yaml }) = action {
                        match send_ipc_request(&IpcRequest::ExecuteGraphYaml { yaml }) {
                            Ok(response) => self.canvas_state.apply_pipeline_response(response),
                            Err(error) => self.canvas_state.apply_pipeline_response(
                                IpcResponse::Error {
                                    error: error.to_string(),
                                },
                            ),
                        }
                        self.dirty_mirror = true;
                        ctx.request_repaint();
                    }
                }
                WorkbenchView::AutoActions => {
                    let previous_enabled = self.config.auto_actions.trim_link_tracking;
                    if let Some(enabled) = render_auto_actions(ui, &mut self.config, &self.ui_palette).0 {
                        self.execute_auto_action_change_with(
                            ctx,
                            previous_enabled,
                            enabled,
                            &mut |request| {
                                send_ipc_request(request).map_err(|error| error.to_string())
                            },
                        );
                    }
                }
            }
        });
    }
}
