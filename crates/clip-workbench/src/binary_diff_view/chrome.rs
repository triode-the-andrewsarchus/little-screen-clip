use super::*;

impl BinaryDiffViewState {
    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        let now = ctx.input(|i| i.time);
        self.poll_handoff(now, ctx);

        // Check async receiver
        if self.is_computing {
            ctx.request_repaint();
            if let Some(ref rx) = self.receiver {
                match rx.try_recv() {
                    Ok(Ok((req_gen, res))) if req_gen == self.current_generation => {
                        self.result = Some(res);
                        self.is_computing = false;
                        self.receiver = None;
                        self.compute_ticket = None;
                        if !self.search_query.is_empty() {
                            self.update_search();
                        }
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        self.status_toast
                            .set(format!("❌ Binary diff failed: {error}"), now);
                        self.is_computing = false;
                        self.receiver = None;
                        self.compute_ticket = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.status_toast
                            .set("❌ Binary diff worker disconnected", now);
                        self.is_computing = false;
                        self.receiver = None;
                        self.compute_ticket = None;
                    }
                }
            }
        }

        self.handle_global_shortcuts(ctx, close_requested);

        let l_size_str = format_bytes_size(self.left_bytes.len());
        let r_size_str = format_bytes_size(self.right_bytes.len());
        let hatch = EscapeHatch::new("ImHex", "imhex", "Open in ImHex Hex Editor")
            .with_icon(Icon::BinaryDiff);

        let mut shell = StudioShell::new("Binary Diff", Icon::BinaryDiff, palette, close_requested)
            .with_badge(format!("Left: {} ({})", self.left_label, l_size_str), None)
            .with_badge(format!("Right: {} ({})", self.right_label, r_size_str), None)
            .with_escape_hatch(hatch)
            .with_footer_hints("Navigation: [Esc] Close • [Tab] Mode • [S] Swap • [J/K/←/↑/→/↓] Diff Nav • [0/1/2] Width • [Search] Hex/ASCII");

        if self.is_computing {
            shell = shell.with_badge("Computing...", Some(hex_to_c32(&palette.harmonies.base)));
        } else if let Some(ref res) = self.result {
            let pct_color = if res.stats.similarity_pct >= 90.0 {
                hex_to_c32(&palette.diff_add)
            } else if res.stats.similarity_pct >= 50.0 {
                hex_to_c32(&palette.harmonies.warm_attention)
            } else {
                hex_to_c32(&palette.diff_del)
            };
            shell = shell
                .with_badge(
                    format!("{:.1}% similar", res.stats.similarity_pct),
                    Some(pct_color),
                )
                .with_badge(
                    format!("{} diffs", res.diff_blocks.len()),
                    Some(hex_to_c32(&palette.diff_del)),
                )
                .with_footer_readout(format!(
                    "{} vs {} • Block count: {}",
                    res.left_label,
                    res.right_label,
                    res.diff_blocks.len()
                ));
        }

        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.canvas))
            .inner_margin(egui::Margin::same(crate::ui_ext::SPACE_MD))
            .show(ui, |ui| {
                // Zone 1: Header
                let clicked_hatch = shell.render_header(ui, |ui| {
                    let swap_btn = crate::ui_ext::secondary_button_icon(Some(Icon::Swap), "Swap [S]", hex_to_c32(&palette.colors.surface_panel), palette);
                    if ui.add(swap_btn).on_hover_text("Swap operands (S)").clicked() {
                        self.swap_operands();
                    }

                    let copy_btn = crate::ui_ext::secondary_button_icon(Some(Icon::Copy), "Summary", hex_to_c32(&palette.colors.surface_panel), palette);
                    if ui.add(copy_btn).clicked() {
                        if let Some(ref res) = self.result {
                            let summary = format!(
                                "Binary Diff Summary:\nLeft: {} ({} bytes)\nRight: {} ({} bytes)\nSimilarity: {:.2}%\nMatches: {} B | Mismatches: {} B | Insertions: {} B | Deletions: {} B\nMode: {:?}",
                                res.left_label, res.stats.left_total_bytes,
                                res.right_label, res.stats.right_total_bytes,
                                res.stats.similarity_pct,
                                res.stats.match_count,
                                res.stats.mismatch_count,
                                res.stats.right_only_count,
                                res.stats.left_only_count,
                                res.mode
                            );
                            ctx.output_mut(|o| o.copied_text = summary);
                            self.status_toast.set("Summary copied to clipboard!", now);
                        }
                    }

                    let diff_panel_label = if self.panel_state.right_collapsed { "Diffs (Show)" } else { "Diffs (Hide)" };
                    let diff_toggle_btn = crate::mode_tab_button(diff_panel_label, !self.panel_state.right_collapsed, palette);
                    if ui.add(diff_toggle_btn).on_hover_text("Toggle Differences Panel").clicked() {
                        self.panel_state.toggle_right();
                    }
                });

                if clicked_hatch.is_some() {
                    self.trigger_escape_hatch(now);
                }

                if shell.is_closed() {
                    return;
                }

                ui.add_space(crate::ui_ext::SPACE_XS);

                // Zone 2: Mode & Filter Bar
                self.render_mode_and_stats_bar(ui, palette);

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                // Zone 3: Main Layout
                let footer_h = if self.hovered_cell.is_some() { 64.0 } else { 32.0 };
                let avail_h = (ui.available_height() - footer_h - 4.0).max(100.0);

                ui.horizontal(|ui| {
                    let diff_panel_w = if self.panel_state.right_collapsed {
                        0.0
                    } else {
                        self.panel_state.effective_right_width().min(280.0)
                    };
                    let minimap_w = 16.0;
                    let table_w = if diff_panel_w > 0.0 {
                        (ui.available_width() - diff_panel_w - minimap_w - 36.0).max(350.0)
                    } else {
                        (ui.available_width() - minimap_w - 16.0).max(400.0)
                    };

                    // 1. Hex Table
                    ui.push_id("binary_hex_table_section", |ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(table_w, avail_h),
                            egui::Layout::top_down(egui::Align::LEFT),
                            |ui| {
                                self.render_hex_table(ui, palette, avail_h);
                            },
                        );
                    });

                    // 2. Minimap Track
                    ui.push_id("binary_minimap_section", |ui| {
                        self.render_minimap(ui, palette, avail_h);
                    });

                    // 3. Differences Table on the right
                    if diff_panel_w > 0.0 {
                        ui.separator();
                        ui.push_id("binary_diff_panel_section", |ui| {
                            self.render_differences_panel(ui, palette, avail_h, diff_panel_w);
                        });
                    }
                });

                // Zone 5: Footer & Cell Inspection
                self.render_footer_inspection(ui, palette);
                let toast_tuple = self.status_toast.message(now).map(|m| (m.to_string(), now));
                shell.render_footer(ui, &toast_tuple, now);
            });
    }

    pub(super) fn render_mode_and_stats_bar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let b_low = hex_to_c32(&palette.colors.surface_recessed);
        let b_med = hex_to_c32(&palette.colors.surface_panel);
        let f_low = hex_to_c32(&palette.colors.text_muted);

        let Some(ref res) = self.result else {
            return;
        };

        let match_count = res.stats.match_count;
        let mismatch_count = res.stats.mismatch_count;
        let left_only_count = res.stats.left_only_count;
        let right_only_count = res.stats.right_only_count;
        let diff_blocks_len = res.diff_blocks.len();
        let search_len = self.search_matches.len();
        let search_idx = self.current_search_idx;

        let mut next_diff = false;
        let mut prev_diff = false;
        let mut next_search = false;
        let mut prev_search = false;
        let mut do_search = false;

        egui::Frame::none()
            .fill(b_low)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::SPACE_MD,
                crate::ui_ext::CONTROL_GROUP_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, b_med))
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let tabs = [
                        crate::ui_tabs::Tab::new(BinaryDiffMode::Aligned, "Aligned [Tab]"),
                        crate::ui_tabs::Tab::new(BinaryDiffMode::Raw, "Raw"),
                    ];
                    if let Some(mode) =
                        crate::ui_tabs::TabBar::new("binary_diff_views", self.mode, &tabs, palette)
                            .show(ui)
                    {
                        self.mode = mode;
                        self.recompute();
                    }

                    // Width selector
                    ui.label(
                        egui::RichText::new("Width:")
                            .font(FontId::proportional(crate::ui_ext::BODY_FONT_SIZE))
                            .color(f_low),
                    );
                    for &w in &[8, 16, 32] {
                        let is_active = self.bytes_per_row == w;
                        let btn = crate::mode_tab_button(format!("{}B", w), is_active, palette);
                        if ui.add(btn).clicked() {
                            self.set_bytes_per_row(w);
                        }
                    }

                    let color_tabs = [
                        crate::ui_tabs::Tab::new(ByteColorMode::Digits, "Digits"),
                        crate::ui_tabs::Tab::new(ByteColorMode::Background, "Bg"),
                    ];
                    if let Some(mode) = crate::ui_tabs::TabBar::new(
                        "binary_color_modes",
                        self.color_mode,
                        &color_tabs,
                        palette,
                    )
                    .show(ui)
                    {
                        self.color_mode = mode;
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.separator();
                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Stat pills
                    crate::ui_ext::stat_pill(
                        ui,
                        "Match",
                        match_count,
                        Some(hex_to_c32(&palette.colors.text_secondary)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Diff",
                        mismatch_count,
                        Some(hex_to_c32(&palette.diff_mod)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Del",
                        left_only_count,
                        Some(hex_to_c32(&palette.diff_del)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Ins",
                        right_only_count,
                        Some(hex_to_c32(&palette.diff_add)),
                        palette,
                    );
                    crate::ui_ext::stat_pill(
                        ui,
                        "Blocks",
                        diff_blocks_len,
                        Some(hex_to_c32(&palette.harmonies.warm_attention)),
                        palette,
                    );

                    // Right side of bar: Search & Diff nav
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let next_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::ArrowDown),
                            "Next [J]",
                            b_med,
                            palette,
                        );
                        if ui.add(next_btn).clicked() {
                            next_diff = true;
                        }
                        let prev_btn = crate::ui_ext::secondary_button_icon(
                            Some(Icon::ArrowUp),
                            "Prev [K]",
                            b_med,
                            palette,
                        );
                        if ui.add(prev_btn).clicked() {
                            prev_diff = true;
                        }

                        ui.add_space(crate::ui_ext::SPACE_SM);

                        if search_len > 0 {
                            let canvas = hex_to_c32(&palette.colors.canvas);
                            ui.label(
                                egui::RichText::new(format!("{}/{}", search_idx + 1, search_len))
                                    .font(FontId::monospace(crate::ui_ext::BODY_FONT_SIZE))
                                    .color(crate::ui_ext::contrast_text_c32_on(
                                        canvas,
                                        canvas,
                                        Some(hex_to_c32(&palette.harmonies.base)),
                                        palette,
                                    )),
                            );
                            let down_btn = crate::ui_ext::icon_button(
                                Icon::ArrowDown,
                                14.0,
                                b_med,
                                None,
                                palette,
                            );
                            if ui.add(down_btn).clicked() {
                                next_search = true;
                            }
                            let up_btn = crate::ui_ext::icon_button(
                                Icon::ArrowUp,
                                14.0,
                                b_med,
                                None,
                                palette,
                            );
                            if ui.add(up_btn).clicked() {
                                prev_search = true;
                            }
                        }

                        let search_edit = ui.add_sized(
                            [180.0, crate::ui_ext::CONTROL_HEIGHT],
                            egui::TextEdit::singleline(&mut self.search_query)
                                .id(egui::Id::new("binary_diff_search_field"))
                                .hint_text("Search Hex / ASCII..."),
                        );
                        if search_edit.changed() {
                            do_search = true;
                        }
                    });
                });
            });

        if next_diff {
            self.next_diff();
        }
        if prev_diff {
            self.prev_diff();
        }
        if next_search {
            self.next_search_match();
        }
        if prev_search {
            self.prev_search_match();
        }
        if do_search {
            self.update_search();
        }
    }
}
