//! Transfer characteristic and live meters for the shared preview output bus.
use super::ConstellationState;
use egui::{Color32, Pos2, Rect, Stroke, Vec2};

#[cfg(test)]
/// Hard-knee reference used by the transfer test.
fn output_db(input: f32, threshold: f32, ratio: f32) -> f32 {
    super::output::compressor_db(input, threshold, ratio, 0.0)
}

impl ConstellationState {
    pub(super) fn render_compressor(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().interact_size.y = 24.0;
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().button_padding = Vec2::new(6.0, 2.0);
        ui.horizontal(|ui| {
            ui.strong("Compressor");
            if ui
                .add(
                    egui::Button::new("Bypass")
                        .selected(self.pack_bank.compressor_bypass)
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke),
                )
                .clicked()
            {
                self.pack_bank.compressor_bypass = !self.pack_bank.compressor_bypass;
            }
            if let Some(error) = self
                .midi
                .as_ref()
                .and_then(|controller| controller.status().error)
            {
                ui.weak("MIDI offline").on_hover_text(error);
            }
        });
        let (levels, reduction, running) = {
            let rack = self.playback.lock().unwrap();
            rack.output
                .as_ref()
                .map_or(([-120.0; 2], 0.0, false), |output| {
                    (output.levels_db(), output.reduction_db(), true)
                })
        };
        if running {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        let (outer, response) = ui.allocate_exact_size(
            Vec2::new(
                ui.available_width(),
                (ui.clip_rect().height() - 250.0)
                    .clamp(110.0, 260.0)
                    .min(ui.available_width()),
            ),
            egui::Sense::hover(),
        );
        response.on_hover_text("Stereo linked. Drag the threshold or ratio handle. The curve shows steady compression; the live dot includes attack/release, makeup gain and the safety ceiling. Fixed 5 ms attack, 120 ms release.");
        let plot = Rect::from_min_max(
            outer.min + Vec2::new(36.0, 24.0),
            outer.max - Vec2::new(24.0, 28.0),
        );
        let point = |input: f32, output: f32| {
            Pos2::new(
                plot.left() + (input + 48.0) / 54.0 * plot.width(),
                plot.bottom() - (output + 48.0) / 54.0 * plot.height(),
            )
        };
        let painter = ui.painter_at(outer);
        painter.rect_filled(outer, 6.0, ui.visuals().extreme_bg_color);
        for db in [-48.0, -36.0, -24.0, -12.0, 0.0] {
            let vertical = point(db, -48.0);
            let horizontal = point(-48.0, db);
            let grid = Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
            painter.line_segment([vertical, point(db, 6.0)], grid);
            painter.line_segment([horizontal, point(6.0, db)], grid);
            painter.text(
                horizontal - Vec2::new(4.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                format!("{db:.0}"),
                egui::FontId::monospace(9.0),
                ui.visuals().weak_text_color(),
            );
            painter.text(
                vertical + Vec2::new(0.0, 3.0),
                egui::Align2::CENTER_TOP,
                format!("{db:.0}"),
                egui::FontId::monospace(9.0),
                ui.visuals().weak_text_color(),
            );
        }
        painter.line_segment(
            [point(-48.0, -48.0), point(6.0, 6.0)],
            Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
        );
        let threshold = if self.pack_bank.compressor_threshold_db.is_finite() {
            self.pack_bank.compressor_threshold_db.clamp(-48.0, 0.0)
        } else {
            -1.4116215
        };
        let ratio = if self.pack_bank.compressor_ratio.is_finite() {
            self.pack_bank.compressor_ratio.clamp(1.0, 12.0)
        } else {
            2.0
        };
        let knee = if self.pack_bank.compressor_knee_db.is_finite() {
            self.pack_bank.compressor_knee_db.clamp(0.0, 24.0)
        } else {
            0.0
        };
        let makeup = if self.pack_bank.compressor_makeup_db.is_finite() {
            self.pack_bank.compressor_makeup_db.clamp(-12.0, 24.0)
        } else {
            0.0
        };
        let transfer = |input| super::output::compressor_db(input, threshold, ratio, knee) + makeup;
        let threshold_point = point(threshold, transfer(threshold).clamp(-48.0, 6.0));
        let ratio_point = point(6.0, transfer(6.0).clamp(-48.0, 6.0));
        let curve = (0..=108)
            .map(|step| {
                let input = -48.0 + step as f32 * 0.5;
                point(
                    input,
                    if self.pack_bank.compressor_bypass {
                        input
                    } else {
                        transfer(input)
                    }
                    .clamp(-48.0, 6.0),
                )
            })
            .collect();
        let accent = ui.visuals().selection.bg_fill;
        painter.add(egui::Shape::line(curve, Stroke::new(2.5_f32, accent)));
        painter.text(
            Pos2::new(plot.center().x, outer.bottom() - 1.0),
            egui::Align2::CENTER_BOTTOM,
            "Input dBFS →",
            egui::FontId::proportional(10.0),
            ui.visuals().text_color(),
        );
        painter.text(
            Pos2::new(plot.left(), outer.top() + 4.0),
            egui::Align2::LEFT_TOP,
            "Output dBFS",
            egui::FontId::proportional(10.0),
            ui.visuals().text_color(),
        );
        if levels[0] > -60.0 {
            painter.circle_filled(
                point(levels[0].clamp(-48.0, 6.0), levels[1].clamp(-48.0, 6.0)),
                4.0,
                Color32::from_rgb(110, 210, 130),
            );
        }
        for (handle, label) in [(threshold_point, "Threshold"), (ratio_point, "Ratio")] {
            painter.circle_filled(handle, 5.0, ui.visuals().extreme_bg_color);
            painter.circle_stroke(handle, 6.0, Stroke::new(2.0_f32, accent));
            let response = ui.interact(
                Rect::from_center_size(handle, Vec2::splat(22.0)),
                ui.id().with(label),
                egui::Sense::click_and_drag(),
            );
            if !self.pack_bank.compressor_bypass
                && response.dragged()
                && let Some(pointer) = response.interact_pointer_pos()
            {
                if label == "Threshold" {
                    self.pack_bank.compressor_threshold_db =
                        (-48.0 + (pointer.x - plot.left()) / plot.width() * 54.0).clamp(-48.0, 0.0);
                } else {
                    let output = -48.0 + (plot.bottom() - pointer.y) / plot.height() * 54.0;
                    let delta = 6.0 - threshold;
                    let amount = if knee > 0.0 && delta < knee * 0.5 {
                        (delta + knee * 0.5).powi(2) / (2.0 * knee)
                    } else {
                        delta
                    };
                    self.pack_bank.compressor_ratio = (1.0 + (output - makeup - 6.0) / amount)
                        .clamp(1.0 / 12.0, 1.0)
                        .recip();
                }
            }
            if response.double_clicked() && !self.pack_bank.compressor_bypass {
                let defaults = super::sample_pack::SamplePackBank::default();
                if label == "Threshold" {
                    self.pack_bank.compressor_threshold_db = defaults.compressor_threshold_db;
                } else {
                    self.pack_bank.compressor_ratio = defaults.compressor_ratio;
                }
            }
            response.on_hover_text(format!(
                "Drag to adjust {label}; editable values are below."
            ));
        }
        for (label, level, x) in [
            ("In", levels[0], outer.left() + 3.0),
            ("Out", levels[1], outer.right() - 11.0),
        ] {
            let meter =
                Rect::from_min_max(Pos2::new(x, plot.top()), Pos2::new(x + 8.0, plot.bottom()));
            painter.rect_filled(meter, 2.0, ui.visuals().widgets.noninteractive.bg_fill);
            let fill = Rect::from_min_max(
                Pos2::new(
                    x,
                    meter.bottom() - ((level + 60.0) / 66.0).clamp(0.0, 1.0) * meter.height(),
                ),
                meter.max,
            );
            let color = if level > 0.0 {
                Color32::from_rgb(225, 85, 75)
            } else if level > -3.0 {
                Color32::from_rgb(235, 175, 70)
            } else {
                Color32::from_rgb(100, 190, 120)
            };
            painter.rect_filled(fill, 2.0, color);
            painter.text(
                Pos2::new(x + 4.0, outer.top() + 4.0),
                egui::Align2::CENTER_TOP,
                label,
                egui::FontId::proportional(9.0),
                ui.visuals().text_color(),
            );
        }
        ui.spacing_mut().slider_width = (ui.available_width() - 180.0).clamp(50.0, 130.0);
        ui.add_enabled_ui(!self.pack_bank.compressor_bypass, |ui| {
            let defaults = super::sample_pack::SamplePackBank::default();
            for (label, value, range, default, suffix) in [
                (
                    "Threshold",
                    &mut self.pack_bank.compressor_threshold_db,
                    -48.0..=0.0,
                    defaults.compressor_threshold_db,
                    " dB",
                ),
                (
                    "Ratio",
                    &mut self.pack_bank.compressor_ratio,
                    1.0..=12.0,
                    defaults.compressor_ratio,
                    ":1",
                ),
                (
                    "Knee",
                    &mut self.pack_bank.compressor_knee_db,
                    0.0..=24.0,
                    defaults.compressor_knee_db,
                    " dB",
                ),
                (
                    "Makeup",
                    &mut self.pack_bank.compressor_makeup_db,
                    -12.0..=24.0,
                    defaults.compressor_makeup_db,
                    " dB",
                ),
            ] {
                let response = ui.add(egui::Slider::new(value, range).text(label).suffix(suffix));
                if crate::shell::studio_controls::slider_double_clicked(ui, &response) {
                    *value = default;
                }
            }
        });
        ui.label(format!(
            "Input {:5.1} dBFS · Output {:5.1} dBFS",
            levels[0].max(-60.0),
            levels[1].max(-60.0)
        ));
        ui.add(
            egui::ProgressBar::new((reduction / 24.0).clamp(0.0, 1.0))
                .text(
                    egui::RichText::new(format!("Gain reduction  {reduction:.1} dB"))
                        .color(ui.visuals().text_color()),
                )
                .fill(ui.visuals().widgets.hovered.bg_fill),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn double_click_resets_every_compressor_slider_to_its_saved_default() {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::new();
        state.pack_bank.compressor_threshold_db = -20.0;
        state.pack_bank.compressor_ratio = 8.0;
        state.pack_bank.compressor_knee_db = 12.0;
        state.pack_bank.compressor_makeup_db = 9.0;
        let render = |state: &mut ConstellationState, events, time| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 800.0))),
                    events,
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| state.render_compressor(ui));
                },
            )
        };
        let output = render(&mut state, vec![], 0.0);
        let labels: std::collections::HashMap<_, _> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some((text.galley.text().to_owned(), text.pos))
                } else {
                    None
                }
            })
            .collect();
        for (index, label) in ["Threshold", "Ratio", "Knee", "Makeup"]
            .into_iter()
            .enumerate()
        {
            let y = labels.get(label).expect("slider label rendered").y + 8.0;
            let pos = Pos2::new(70.0, y);
            for (step, pressed) in [true, false, true, false].into_iter().enumerate() {
                render(
                    &mut state,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    1.0 + index as f64 + step as f64 * 0.05,
                );
            }
        }
        let defaults = super::super::sample_pack::SamplePackBank::default();
        assert_eq!(
            state.pack_bank.compressor_threshold_db,
            defaults.compressor_threshold_db
        );
        assert_eq!(state.pack_bank.compressor_ratio, defaults.compressor_ratio);
        assert_eq!(
            state.pack_bank.compressor_knee_db,
            defaults.compressor_knee_db
        );
        assert_eq!(
            state.pack_bank.compressor_makeup_db,
            defaults.compressor_makeup_db
        );
    }

    #[test]
    fn transfer_curve_matches_the_output_bus_ratio() {
        assert_eq!(output_db(-30.0, -20.0, 4.0), -30.0);
        assert_eq!(output_db(0.0, -20.0, 4.0), -15.0);
        assert_eq!(output_db(6.0, -20.0, 1.0), 6.0);
        let gain = 10.0_f32.powf((output_db(-6.0, -20.0, 4.0) + 6.0) / 20.0);
        assert!(
            (gain - (10.0_f32.powf(-20.0 / 20.0) / 10.0_f32.powf(-6.0 / 20.0)).powf(0.75)).abs()
                < 0.00001
        );
    }
}
