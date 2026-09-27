//! Shared vertical audio workspace. Tool controls sit above a full-width timeline.
use super::*;

pub(super) struct AudioWorkspace;

impl AudioWorkspace {
    const BUTTON_HEIGHT: f32 = crate::ui_ext::CONTROL_HEIGHT;
    const TIME_READOUT_WIDTH: f32 = 242.0;

    /// Keep timeline painting and hit regions inside even a short canvas.
    pub(super) fn inspect_timeline_rects(track: Rect) -> [Rect; 5] {
        let height = track.height().max(0.0);
        // Normal geometry needs 126 points of fixed rows/gaps and 100 of plots.
        let compression = (height / 226.0).min(1.0);
        let gap = 4.0 * compression;
        let plot = (height - 126.0 * compression).max(0.0) / 2.0;
        let heights = [
            24.0 * compression,
            plot,
            plot,
            72.0 * compression,
            14.0 * compression,
        ];
        let mut y = track.min.y;
        heights.map(|height| {
            let rect = Rect::from_min_max(
                Pos2::new(track.min.x, y),
                Pos2::new(track.max.x, y + height),
            );
            y += height + gap;
            rect
        })
    }

    fn fixed_readout(ui: &mut egui::Ui, text: &str, width: f32, color: Color32) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(width, Self::BUTTON_HEIGHT), egui::Sense::hover());
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::monospace(14.0),
            color,
        );
    }

    pub(super) fn clock_time(seconds: f32) -> String {
        let centiseconds = (seconds.max(0.0) as f64 * 100.0).round() as u64;
        let hours = centiseconds / 360_000;
        let minutes = (centiseconds / 6_000) % 60;
        let seconds = (centiseconds / 100) % 60;
        format!(
            "{hours:02}:{minutes:02}:{seconds:02}.{:02}",
            centiseconds % 100
        )
    }

    pub(super) fn time_readout(
        ui: &mut egui::Ui,
        current: f32,
        duration: f32,
        palette: &AppPalette,
    ) {
        let text = format!(
            "{} / {}",
            Self::clock_time(current),
            Self::clock_time(duration)
        );
        Self::fixed_readout(
            ui,
            &text,
            Self::TIME_READOUT_WIDTH,
            hex_to_c32(&palette.harmonies.base),
        );
    }

    pub(super) fn primary_button(
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> egui::Button<'static> {
        Self::primary_button_icon(None, label, fill, palette)
    }

    pub(super) fn primary_button_icon(
        icon: Option<crate::icons::Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> egui::Button<'static> {
        crate::action_button_icon(icon, label, fill, palette)
            .min_size(Vec2::new(0.0, Self::BUTTON_HEIGHT))
    }

    pub(super) fn secondary_button(
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> egui::Button<'static> {
        Self::secondary_button_icon(None, label, fill, palette)
    }

    pub(super) fn secondary_button_icon(
        icon: Option<crate::icons::Icon>,
        label: impl Into<String>,
        fill: Color32,
        palette: &AppPalette,
    ) -> egui::Button<'static> {
        crate::secondary_button_icon(icon, label, fill, palette)
            .min_size(Vec2::new(0.0, Self::BUTTON_HEIGHT))
    }

    pub(super) fn toggle_button_icon(
        icon: Option<crate::icons::Icon>,
        label: impl Into<String>,
        is_active: bool,
        palette: &AppPalette,
    ) -> egui::Button<'static> {
        crate::mode_tab_button_icon(icon, label, is_active, palette)
            .min_size(Vec2::new(0.0, Self::BUTTON_HEIGHT))
    }

    /// One wrapping band for transport, selection, and export controls.
    pub(super) fn control_bar(
        ui: &mut egui::Ui,
        palette: &AppPalette,
        controls: impl FnOnce(&mut egui::Ui),
    ) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none()
            .fill(colors.recessed)
            .stroke(Stroke::new(1.0_f32, colors.panel))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET_X,
                crate::ui_ext::CONTROL_ROW_INSET_Y,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(controls);
            });
        ui.add_space(crate::ui_ext::SPACE_XS);
    }

    pub(super) fn info_bar(
        ui: &mut egui::Ui,
        palette: &AppPalette,
        contents: impl FnOnce(&mut egui::Ui),
    ) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none()
            .fill(colors.panel)
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32_alpha(&palette.harmonies.base, 100),
            ))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET_X,
                crate::ui_ext::FOOTER_INSET_Y,
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                contents(ui);
            });
        ui.add_space(crate::ui_ext::SPACE_XS);
    }

    pub(super) fn control_group(
        ui: &mut egui::Ui,
        palette: &AppPalette,
        label: &str,
        controls: impl FnOnce(&mut egui::Ui),
    ) {
        Self::section_title(ui, palette, label);
        controls(ui);
    }

    pub(super) fn section_divider(ui: &mut egui::Ui, palette: &AppPalette) {
        ui.add_space(crate::ui_ext::SPACE_SM);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(2.0, Self::BUTTON_HEIGHT), egui::Sense::hover());
        ui.painter().line_segment(
            [rect.center_top(), rect.center_bottom()],
            Stroke::new(1.5_f32, hex_to_c32_alpha(&palette.harmonies.base, 140)),
        );
        ui.add_space(crate::ui_ext::SPACE_SM);
    }

    fn section_title(ui: &mut egui::Ui, palette: &AppPalette, label: &str) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none()
            .fill(colors.panel)
            .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
            .inner_margin(egui::Margin::symmetric(crate::ui_ext::SPACE_SM, 0.0))
            .show(ui, |ui| {
                Self::inline_label(ui, &format!("{label}:"), colors.text_primary);
            });
    }

    /// A readable label centered against the shared control row height.
    pub(super) fn inline_label(ui: &mut egui::Ui, label: &str, color: Color32) {
        let font = egui::TextStyle::Body.resolve(ui.style());
        let width = ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font, color).size().x);
        ui.add_sized(
            [width + 4.0, Self::BUTTON_HEIGHT],
            egui::Label::new(egui::RichText::new(label).strong().color(color)).wrap(false),
        );
    }

    /// A full-width control band above the timeline. The label stays with its controls.
    pub(super) fn control_row(
        ui: &mut egui::Ui,
        palette: &AppPalette,
        label: &str,
        controls: impl FnOnce(&mut egui::Ui),
    ) {
        Self::control_bar(ui, palette, |ui| {
            Self::section_title(ui, palette, label);
            Self::section_divider(ui, palette);
            controls(ui);
        });
    }

    pub(super) fn toggle_button(
        label: &str,
        active: bool,
        palette: &AppPalette,
    ) -> egui::Button<'static> {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        let background = if active {
            hex_to_c32_alpha(&palette.harmonies.base, 40)
        } else {
            colors.recessed
        };
        let border = if active {
            hex_to_c32(&palette.harmonies.base)
        } else {
            colors.panel
        };
        egui::Button::new(
            egui::RichText::new(label.to_owned())
                .strong()
                .color(crate::contrast_text_c32(background, Some(border), palette)),
        )
        .fill(background)
        .stroke(Stroke::new(1.0_f32, border))
        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
        .min_size(Vec2::new(0.0, Self::BUTTON_HEIGHT))
    }

    pub(super) fn zoom_control_bar(
        ui: &mut egui::Ui,
        palette: &AppPalette,
    ) -> Option<crate::ui_ext::ZoomAction> {
        use crate::icons::Icon;
        use crate::ui_ext::ZoomAction;
        let fill = crate::ui_appearance::WorkbenchColors::from_palette(palette).recessed;
        let mut action = None;
        if ui
            .add(crate::ui_ext::icon_button(
                Icon::ZoomIn,
                14.0,
                fill,
                None,
                palette,
            ))
            .on_hover_text("Zoom In (+ / Ctrl+Wheel)")
            .clicked()
        {
            action = Some(ZoomAction::ZoomIn);
        }
        if ui
            .add(crate::ui_ext::icon_button(
                Icon::ZoomOut,
                14.0,
                fill,
                None,
                palette,
            ))
            .on_hover_text("Zoom Out (- / Ctrl+Wheel)")
            .clicked()
        {
            action = Some(ZoomAction::ZoomOut);
        }
        if ui
            .add(crate::ui_ext::secondary_button_icon(
                Some(Icon::ZoomFit),
                "Fit",
                fill,
                palette,
            ))
            .on_hover_text("Fit Track / Window (Ctrl+0)")
            .clicked()
        {
            action = Some(ZoomAction::Fit);
        }
        action
    }

    pub(super) fn show(ui: &mut egui::Ui, palette: &AppPalette, body: impl FnOnce(&mut egui::Ui)) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        let rect = ui.available_rect_before_wrap();
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.painter()
                .rect_filled(rect, Rounding::ZERO, colors.canvas);
            ui.add_space(crate::ui_ext::SPACE_SM);
            body(ui);
        });
    }
}

impl AudioPreviewState {
    fn render_volume_control(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        ui.horizontal(|ui| {
            crate::icons::Icon::Volume.ui(ui, 16.0, colors.text_secondary);
            let mut volume_percent = self.volume * 100.0;
            let changed = crate::shell::render_track_slider(
                ui,
                &mut volume_percent,
                0.0..=100.0,
                124.0,
                palette,
            )
            .on_hover_text(format!("Volume: {:.0}%", volume_percent))
            .changed();
            if changed {
                self.set_studio_volume(volume_percent / 100.0);
            }
            AudioWorkspace::fixed_readout(
                ui,
                &format!("{:>3.0}%", self.volume * 100.0),
                48.0,
                colors.text_secondary,
            );
        });
    }

    pub(crate) fn render_audio_header(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        close_requested: &mut bool,
    ) {
        let mins = (self.duration_sec / 60.0) as u32;
        let secs = self.duration_sec % 60.0;
        let editor = if self.audio_editor.is_empty() {
            "audacity"
        } else {
            &self.audio_editor
        };
        let has_separated_stems = self.active_mode == AudioToolMode::StemSeparation
            && !self.stem_separation.stems.is_empty();
        let editor_tooltip = if has_separated_stems {
            format!("Open all four stems as separate tracks in {editor}")
        } else {
            format!("Open the source audio in {editor}")
        };
        let hatch_name = if editor.eq_ignore_ascii_case("audacity") {
            "Audacity"
        } else {
            editor
        };
        let hatch = EscapeHatch::primary(hatch_name, editor, editor_tooltip)
            .with_icon(crate::icons::Icon::Volume);

        let mut shell = StudioShell::new(
            "Audio Studio",
            crate::icons::Icon::Audio,
            palette,
            close_requested,
        )
        .with_badge(format!("{:02}:{:04.1}", mins, secs), None)
        .with_escape_hatch(hatch);

        if let Some(ref meta) = self.metadata {
            shell = shell
                .with_badge(format!("{:.1} kHz", meta.sample_rate as f32 / 1000.0), None)
                .with_badge(&meta.format, Some(hex_to_c32(&palette.harmonies.base)));
        }

        let clicked = shell.render_header_with_title_actions(
            ui,
            |ui| self.render_volume_control(ui, palette),
            |_ui| {},
        );

        if clicked.is_some() {
            if has_separated_stems {
                self.launch_stems_in_external_editor(ctx, close_requested);
            } else {
                self.launch_external_audio_editor(ctx, close_requested);
            }
        }

        if *close_requested {
            self.stop();
            for stem in &mut self.stem_separation.stems {
                stem.preview.stop();
            }
        }
    }

    pub(crate) fn render_audio_tool_bar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none()
            .fill(colors.recessed)
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::PANEL_INSET_X,
                crate::ui_ext::FOOTER_INSET_Y,
            ))
            .stroke(Stroke::new(1.0_f32, colors.panel))
            .show(ui, |ui| {
                render_audio_tool_tabs(ui, &mut self.active_mode, palette);
            });
    }
}

fn render_audio_tool_tabs(
    ui: &mut egui::Ui,
    active_mode: &mut AudioToolMode,
    palette: &AppPalette,
) {
    let labels = AudioToolMode::ALL
        .iter()
        .map(|mode| mode.label())
        .collect::<Vec<_>>();
    let width = crate::ui_ext::wrapped_equal_button_width(ui, &labels, true, labels.len());
    ui.horizontal_wrapped(|ui| {
        for &mode in AudioToolMode::ALL {
            let button = crate::ui_ext::mode_tab_button_icon(
                Some(mode.icon()),
                mode.label(),
                *active_mode == mode,
                palette,
            )
            .min_size(Vec2::new(width, AudioWorkspace::BUTTON_HEIGHT));
            if ui.add(button).clicked() {
                *active_mode = mode;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_timeline_regions_fit_short_and_normal_canvases() {
        for height in [0.0, 1.0, 64.0, 126.0, 180.0, 220.0, 226.0, 500.0] {
            let host = Rect::from_min_size(Pos2::new(48.0, 32.0), Vec2::new(600.0, height));
            let rows = AudioWorkspace::inspect_timeline_rects(host);
            for row in rows {
                assert!(row.min.y >= host.min.y - 0.001);
                assert!(row.max.y <= host.max.y + 0.001);
                assert!(row.height() >= 0.0);
            }
            for pair in rows.windows(2) {
                assert!(pair[0].max.y <= pair[1].min.y);
            }
            assert!((rows[4].max.y - host.max.y).abs() < 0.001);
            if height >= 226.0 {
                assert_eq!(rows[0].height(), 24.0);
                assert_eq!(rows[3].height(), 72.0);
                assert_eq!(rows[4].height(), 14.0);
                assert_eq!(rows[1].height(), rows[2].height());
            }
        }
    }

    #[test]
    fn audio_time_readout_keeps_hours() {
        assert_eq!(AudioWorkspace::clock_time(3661.25), "01:01:01.25");
    }

    #[test]
    fn mode_label_centers_against_transport_control() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut button_rect = None;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.horizontal(|ui| {
                    AudioWorkspace::inline_label(ui, "Mode:", Color32::WHITE);
                    button_rect = Some(
                        ui.add(AudioWorkspace::toggle_button_icon(
                            None, "Normal", true, &palette,
                        ))
                        .rect,
                    );
                });
            });
        });
        let control = button_rect.expect("mode button should render");
        let (text_pos, text_size) = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "Mode:" => {
                    Some((text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("mode label should render");
        assert_eq!(control.height(), crate::ui_ext::CONTROL_HEIGHT);
        assert!((text_pos.y + text_size.y / 2.0 - control.center().y).abs() < 1.0);
    }

    #[test]
    fn audio_transport_button_families_share_control_height() {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut heights = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for button in [
                    AudioWorkspace::primary_button("Play", Color32::GREEN, &palette),
                    AudioWorkspace::secondary_button("Stop", Color32::DARK_GRAY, &palette),
                    AudioWorkspace::toggle_button("Loop", true, &palette),
                ] {
                    heights.push(ui.add(button).rect.height());
                }
            });
        });

        assert_eq!(heights.len(), 3);
        assert!(
            heights
                .iter()
                .all(|height| (*height - crate::ui_ext::CONTROL_HEIGHT).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn audio_tool_tabs_wrap_and_keep_equal_sizing() {
        for width in [440.0, 1280.0] {
            let ctx = egui::Context::default();
            crate::ui_ext::configure_workbench_style(&ctx);
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let host = Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 180.0));
            let mut active_mode = AudioToolMode::Inspect;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(host),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        render_audio_tool_tabs(ui, &mut active_mode, &palette);
                    });
                },
            );
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            let button_rects = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Rect(rect)
                        if (rect.rect.height() - crate::ui_ext::CONTROL_HEIGHT).abs() < 0.1
                            && (rect.fill == colors.accent || rect.fill == colors.panel) =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();

            assert_eq!(button_rects.len(), AudioToolMode::ALL.len());
            assert!(button_rects.iter().all(|rect| {
                (rect.width() - button_rects[0].width()).abs() < 0.1
                    && rect.min.x >= host.min.x
                    && rect.max.x <= host.max.x
                    && rect.max.y <= host.max.y
            }));
            if width > 1000.0 {
                assert!(
                    button_rects
                        .iter()
                        .all(|rect| (rect.min.y - button_rects[0].min.y).abs() < 0.1)
                );
            }
        }
    }
}
