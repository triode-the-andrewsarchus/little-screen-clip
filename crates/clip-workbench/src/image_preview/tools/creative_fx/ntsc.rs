use super::*;

impl ImagePreviewState {
    pub(super) fn render_ntsc_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let base_c = hex_to_c32(&palette.harmonies.base);
        let f_med = hex_to_c32(&palette.colors.text_secondary);
        // ── 1. Broadcast RF & Filtering ──
        ui.label(
            egui::RichText::new("📡 Signal & Filtering")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(base_c),
        );
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "Luma Smear (Bandwidth)",
            &mut self.creative_fx_state.ntsc_params.luma_smear,
            0.0..=1.0,
            0.25,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "Composite Sharpness",
            &mut self.creative_fx_state.ntsc_params.composite_sharpening,
            0.0..=3.0,
            1.0,
            "×",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "Edge Ringing / Ghosting",
            &mut self.creative_fx_state.ntsc_params.ringing,
            0.0..=2.0,
            0.5,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(3.0);

        // Demodulation Filter
        ui.label(
            egui::RichText::new("Demodulation Filter")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for demo in NtscDemodulation::all() {
                let sel = self.creative_fx_state.ntsc_params.demodulation == *demo;
                if ui
                    .add(creative_fx_choice_button(demo.label(), sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.ntsc_params.demodulation = *demo;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(3.0);

        // Scanline Phase Shift
        ui.label(
            egui::RichText::new("Subcarrier Phase Shift (Dot Crawl)")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for ps in NtscPhaseShift::all() {
                let sel = self.creative_fx_state.ntsc_params.phase_shift == *ps;
                if ui
                    .add(creative_fx_choice_button(ps.label(), sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.ntsc_params.phase_shift = *ps;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(3.0);

        // Chroma Lowpass
        ui.label(
            egui::RichText::new("Chroma Lowpass Filter")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .color(f_med),
        );
        ui.horizontal_wrapped(|ui| {
            for cl in NtscChromaLowpass::all() {
                let sel = self.creative_fx_state.ntsc_params.chroma_lowpass == *cl;
                if ui
                    .add(creative_fx_choice_button(cl.label(), sel, palette))
                    .clicked()
                    && !sel
                {
                    self.creative_fx_state.ntsc_params.chroma_lowpass = *cl;
                    self.creative_fx_state.mark_custom();
                }
            }
        });
        ui.add_space(3.0);

        let mut scanlines = self.creative_fx_state.ntsc_params.scanlines;
        if ui
            .checkbox(
                &mut scanlines,
                egui::RichText::new("📺 CRT Interlaced Scanlines")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            )
            .changed()
        {
            self.creative_fx_state.ntsc_params.scanlines = scanlines;
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // ── 2. Chrominance & YIQ ──
        ui.separator();
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new("🎨 Chrominance & Color")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(base_c),
        );
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "↔ Chroma Horizontal Bleed",
            &mut self.creative_fx_state.ntsc_params.chroma_delay_x,
            -8.0..=8.0,
            1.0,
            "px",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        let mut delay_y = self.creative_fx_state.ntsc_params.chroma_delay_y as f32;
        if render_context_slider(
            ui,
            "↕ Chroma Vertical Shift",
            &mut delay_y,
            -4.0..=4.0,
            0.0,
            "lines",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.ntsc_params.chroma_delay_y = delay_y.round() as i32;
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        let mut vert_blend = self.creative_fx_state.ntsc_params.chroma_vert_blend;
        if ui
            .checkbox(
                &mut vert_blend,
                egui::RichText::new("Chroma Vertical Blending")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            )
            .changed()
        {
            self.creative_fx_state.ntsc_params.chroma_vert_blend = vert_blend;
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "🎨 Global Tint / Hue Shift (0: Normal)",
            &mut self.creative_fx_state.ntsc_params.chroma_phase_error,
            0.0..=1.0,
            0.0,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "〰 Chroma Phase Jitter",
            &mut self.creative_fx_state.ntsc_params.chroma_phase_noise,
            0.0..=0.10,
            0.005,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "🌈 Color Grain / Chroma Noise",
            &mut self.creative_fx_state.ntsc_params.chroma_noise,
            0.0..=0.50,
            0.05,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // ── 3. Noise & Static ──
        ui.separator();
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new("❄ Noise & Static")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(base_c),
        );
        ui.add_space(2.0);

        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("RF Static / Snow")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.spacing_mut().slider_width = slider_w;
            let mut snow = self.creative_fx_state.ntsc_params.snow;
            let slider_response = ui.add(egui::Slider::new(&mut snow, 0.0..=0.04).max_decimals(4));
            let default_snow = NtscParams::default().snow;
            let slider_response = slider_response.on_hover_text(format!(
                "Double-click to reset (default: {:.4})",
                default_snow
            ));
            let is_double_click = slider_double_clicked(ui, &slider_response);
            if is_double_click {
                snow = default_snow;
            }
            if slider_response.changed() || is_double_click {
                self.creative_fx_state.ntsc_params.snow = snow;
                self.creative_fx_state.mark_custom();
            }
        });
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "Snow Streaking (Anisotropy)",
            &mut self.creative_fx_state.ntsc_params.snow_anisotropy,
            0.0..=1.0,
            0.5,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "Luma Noise Grain",
            &mut self.creative_fx_state.ntsc_params.luma_noise,
            0.0..=0.50,
            0.01,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(2.0);

        if render_context_slider(
            ui,
            "Composite Signal Noise",
            &mut self.creative_fx_state.ntsc_params.composite_noise,
            0.0..=0.50,
            0.05,
            "",
            slider_w,
            palette,
        ) {
            self.creative_fx_state.mark_custom();
        }
        ui.add_space(crate::ui_ext::SPACE_XS);

        // ── 4. VHS Magnetic Tape Simulation ──
        ui.separator();
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new("📼 VHS Tape Emulation")
                .size(crate::ui_ext::BODY_FONT_SIZE)
                .strong()
                .color(base_c),
        );
        ui.add_space(2.0);

        let mut vhs_enabled = self.creative_fx_state.ntsc_params.vhs_enabled;
        if ui
            .checkbox(
                &mut vhs_enabled,
                egui::RichText::new("Enable VHS Tape Pass")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            )
            .changed()
        {
            self.creative_fx_state.ntsc_params.vhs_enabled = vhs_enabled;
            self.creative_fx_state.mark_custom();
        }

        if self.creative_fx_state.ntsc_params.vhs_enabled {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Tape Speed")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.horizontal_wrapped(|ui| {
                for speed in [VhsTapeSpeed::SP, VhsTapeSpeed::LP, VhsTapeSpeed::EP] {
                    let sel = self.creative_fx_state.ntsc_params.tape_speed == speed;
                    if ui
                        .add(creative_fx_choice_button(speed.label(), sel, palette))
                        .clicked()
                        && !sel
                    {
                        self.creative_fx_state.ntsc_params.tape_speed = speed;
                        self.creative_fx_state.mark_custom();
                    }
                }
            });
            ui.add_space(2.0);

            if render_context_slider(
                ui,
                "〰 Tape Wobble / Jitter",
                &mut self.creative_fx_state.ntsc_params.edge_wave,
                0.0..=2.0,
                0.5,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(2.0);

            if render_context_slider(
                ui,
                "📶 Tracking Noise Bar",
                &mut self.creative_fx_state.ntsc_params.tracking_noise,
                0.0..=1.0,
                0.0,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(2.0);

            if render_context_slider(
                ui,
                "Playhead Sharpening",
                &mut self.creative_fx_state.ntsc_params.vhs_sharpen,
                0.0..=1.0,
                0.25,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(2.0);

            if render_context_slider(
                ui,
                "Chroma Loss / Dropout",
                &mut self.creative_fx_state.ntsc_params.vhs_chroma_loss,
                0.0..=1.0,
                0.05,
                "",
                slider_w,
                palette,
            ) {
                self.creative_fx_state.mark_custom();
            }
            ui.add_space(2.0);

            let mut head_sw = self.creative_fx_state.ntsc_params.head_switching;
            if ui
                .checkbox(
                    &mut head_sw,
                    egui::RichText::new("Head-Switching Jitter (Bottom)")
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(f_med),
                )
                .changed()
            {
                self.creative_fx_state.ntsc_params.head_switching = head_sw;
                self.creative_fx_state.mark_custom();
            }
        }

        ui.add_space(3.0);
        ui.separator();
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new("🎲 Noise Random Seed")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(f_med),
            );
            ui.spacing_mut().slider_width = slider_w;
            let mut seed = self.creative_fx_state.ntsc_params.random_seed;
            let slider_response = ui.add(egui::Slider::new(&mut seed, 0..=1000));
            let default_seed = NtscParams::default().random_seed;
            let slider_response = slider_response
                .on_hover_text(format!("Double-click to reset (default: {})", default_seed));
            let is_double_click = slider_double_clicked(ui, &slider_response);
            if is_double_click {
                seed = default_seed;
            }
            if slider_response.changed() || is_double_click {
                self.creative_fx_state.ntsc_params.random_seed = seed;
                self.creative_fx_state.mark_custom();
            }
        });
    }
}
