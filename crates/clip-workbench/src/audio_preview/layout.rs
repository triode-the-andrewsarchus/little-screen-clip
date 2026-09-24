//! Shared vertical audio workspace. Tool controls sit above a full-width timeline.
use super::*;

pub(super) struct AudioWorkspace;

impl AudioWorkspace {
    const BUTTON_HEIGHT: f32 = crate::ui_ext::CONTROL_HEIGHT;
    const TIME_READOUT_WIDTH: f32 = 242.0;

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
            .inner_margin(egui::Margin::symmetric(14.0, 5.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(controls);
            });
        ui.add_space(4.0);
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
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                contents(ui);
            });
        ui.add_space(4.0);
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
        ui.add_space(8.0);
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(2.0, Self::BUTTON_HEIGHT), egui::Sense::hover());
        ui.painter().line_segment(
            [rect.center_top(), rect.center_bottom()],
            Stroke::new(1.5_f32, hex_to_c32_alpha(&palette.harmonies.base, 140)),
        );
        ui.add_space(8.0);
    }

    fn section_title(ui: &mut egui::Ui, palette: &AppPalette, label: &str) {
        let colors = crate::ui_appearance::WorkbenchColors::from_palette(palette);
        egui::Frame::none()
            .fill(colors.panel)
            .rounding(Rounding::same(3.0))
            .inner_margin(egui::Margin::symmetric(8.0, 0.0))
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
        .rounding(Rounding::same(4.0))
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
            ui.add_space(8.0);
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
            .inner_margin(egui::Margin::symmetric(14.0, 6.0))
            .stroke(Stroke::new(1.0_f32, colors.panel))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for &mode in AudioToolMode::ALL {
                        if ui
                            .add(crate::ui_ext::mode_tab_button_icon(
                                Some(mode.icon()),
                                mode.label(),
                                self.active_mode == mode,
                                palette,
                            ))
                            .clicked()
                        {
                            self.active_mode = mode;
                        }
                    }
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
