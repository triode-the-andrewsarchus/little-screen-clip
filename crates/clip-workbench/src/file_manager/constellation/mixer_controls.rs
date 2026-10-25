//! Pad-layer mixer and compact performance strip with independent speed and pitch.
use super::sample_pack::SlotSample;
use egui::{Pos2, Rect, Sense, Stroke, Vec2};

/// A bipolar rotary control with a small zero detent and double-click reset.
pub(super) fn knob(ui: &mut egui::Ui, label: &str, value: &mut f32, limit: f32) {
    ui.allocate_ui_with_layout(
        Vec2::new(40.0, 52.0),
        egui::Layout::top_down(egui::Align::Center),
        |ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::splat(34.0), Sense::click_and_drag());
            let drag_id = response.id.with("unsnapped");
            if response.drag_started() {
                ui.data_mut(|data| data.insert_temp(drag_id, *value));
            }
            if response.dragged() {
                let delta = ui.input(|input| input.pointer.delta());
                let raw = (ui
                    .data(|data| data.get_temp::<f32>(drag_id))
                    .unwrap_or(*value)
                    + (delta.x - delta.y) * limit / 100.0)
                    .clamp(-limit, limit);
                ui.data_mut(|data| data.insert_temp(drag_id, raw));
                *value = if raw.abs() < limit * 0.025 { 0.0 } else { raw };
            }
            if response.double_clicked() {
                *value = 0.0;
            }
            response.clone().on_hover_text(format!(
                "{label}: {value:+.2}\nDrag to adjust; double-click to reset"
            ));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, label));
            let center = rect.center();
            let radius = 13.0;
            let start = std::f32::consts::PI * 0.75;
            let sweep = std::f32::consts::PI * 1.5;
            let angle = start + (*value / limit + 1.0) * 0.5 * sweep;
            let painter = ui.painter();
            painter.circle(
                center,
                radius,
                ui.visuals().widgets.inactive.weak_bg_fill,
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
            let arc = (0..=32)
                .map(|i| center + Vec2::angled(start + sweep * i as f32 / 32.0) * (radius + 2.0))
                .collect();
            painter.add(egui::Shape::line(
                arc,
                Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
            ));
            painter.line_segment(
                [center, center + Vec2::angled(angle) * 10.0],
                Stroke::new(2.0_f32, ui.visuals().text_color()),
            );
            painter.line_segment(
                [
                    center + Vec2::new(0.0, -18.0),
                    center + Vec2::new(0.0, -15.0),
                ],
                Stroke::new(1.5_f32, ui.visuals().hyperlink_color),
            );
            ui.small(label);
        },
    );
}

/// Places only level and stereo balance beside the waveform.
pub(super) fn mixer(ui: &mut egui::Ui, sample: &mut SlotSample) {
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.small("Gain");
            gain_fader(ui, &mut sample.volume);
        });
        ui.add_space(4.0);
        knob(ui, "Pan", &mut sample.pan, 1.0);
    });
}

/// Keeps performance controls together and the studio action at the waveform's right edge.
pub(super) fn layer_strip(
    ui: &mut egui::Ui,
    sample: &mut SlotSample,
    retrigger: &mut bool,
) -> bool {
    let width = ui.available_width();
    let compact = width < 660.0;
    let mut studio = false;
    ui.horizontal(|ui| {
        if super::ui::outlined_toggle(ui, "Retrigger", *retrigger, ui.visuals().hyperlink_color)
            .on_hover_text("Restart this pad on every hit. Off allows overlapping hits.")
            .clicked()
        {
            *retrigger = !*retrigger;
        }
        ui.separator();
        if !compact {
            tuning_faders(ui, sample, (width - 240.0).max(320.0));
            ui.separator();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            studio = ui
                .small_button("Edit in Audio Studio")
                .on_hover_text(&sample.filename)
                .clicked();
        });
    });
    if compact {
        ui.horizontal(|ui| tuning_faders(ui, sample, width));
    }
    studio
}

fn tuning_faders(ui: &mut egui::Ui, sample: &mut SlotSample, width: f32) {
    let section = (width - ui.spacing().item_spacing.x * 2.0 - 1.0) * 0.5;
    fader(
        ui,
        "Speed",
        &mut sample.playback_speed,
        2.0,
        0.01,
        1.0,
        section,
    );
    ui.separator();
    let mut cents = (sample.pitch_semitones * 100.0).round();
    fader(ui, "Pitch", &mut cents, 2400.0, 1.0, 0.0, section);
    sample.pitch_semitones = cents / 100.0;
}

/// Uses a fixed-width editable readout, so signs and units never move its neighbours.
fn fader(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    limit: f32,
    step: f64,
    reset: f32,
    width: f32,
) {
    ui.allocate_ui_with_layout(
        Vec2::new(width, 20.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.small(label);
            ui.spacing_mut().slider_width = (width - 106.0).max(24.0);
            let response = ui.add(
                egui::Slider::new(value, -limit..=limit)
                    .step_by(step)
                    .show_value(false)
                    .clamp_to_range(false),
            );
            if response.changed() {
                *value = value.clamp(-limit, limit);
            }
            // A small physical detent aids pointer use; the readout still accepts single cents.
            if response.dragged() && value.abs() < limit * 0.015 {
                *value = 0.0;
            }
            if response.double_clicked() {
                *value = reset;
            }
            let center = response.rect.center();
            ui.painter().line_segment(
                [center + Vec2::new(0.0, 6.0), center + Vec2::new(0.0, 9.0)],
                Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
            );
            response.on_hover_text(if label == "Speed" {
                "−2× to +2×. Negative plays backwards; 0 stops. Double-click for +1×."
            } else {
                "−2 to +2 octaves, in cents (1200 cents per octave). Double-click to reset."
            });
            // Limit user edits while retaining the exact rate of old Time-mode packs (up to 4×).
            let mut edited = *value;
            let readout = ui.add_sized(
                Vec2::new(66.0, 20.0),
                egui::DragValue::new(&mut edited)
                    .speed(step)
                    .custom_parser(move |text| {
                        let text = text.trim();
                        let octaves = text.ends_with("oct");
                        let number = text
                            .trim_end_matches("oct")
                            .trim_end_matches("ct")
                            .trim_end_matches('×')
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .filter(|number| number.is_finite())?;
                        Some(if label == "Pitch" && octaves {
                            number * 1200.0
                        } else {
                            number
                        })
                    })
                    .custom_formatter(move |v, _| {
                        if label == "Speed" {
                            format!("{v:+.2}×")
                        } else if v as i32 % 1200 == 0 {
                            format!("{:+.0} oct", v / 1200.0)
                        } else {
                            format!("{v:+.0} ct")
                        }
                    }),
            );
            if readout.changed() {
                *value = edited.clamp(-limit, limit);
            }
            if readout.double_clicked() {
                *value = reset;
            }
        },
    );
}

/// A tall, marked dB fader; its silent lower stop does not require a numeric field.
fn gain_fader(ui: &mut egui::Ui, volume: &mut f32) {
    let mut db = if *volume > 0.0 {
        20.0 * volume.log10()
    } else {
        -60.0
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = 150.0;
        let response = ui.add(
            egui::Slider::new(&mut db, -60.0..=6.0206)
                .vertical()
                .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 })
                .suffix(" dB")
                .show_value(false),
        );
        if response.double_clicked() {
            db = 0.0;
        }
        if response.changed() || response.double_clicked() {
            *volume = if db <= -60.0 {
                0.0
            } else {
                10.0_f32.powf(db / 20.0)
            };
        }
        let inset = response.rect.width() / 2.5 * 0.5;
        let scale = Rect::from_min_size(
            Pos2::new(response.rect.right() + 3.0, response.rect.top() + inset),
            Vec2::new(30.0, response.rect.height() - inset * 2.0),
        );
        for tick in [6.0, 0.0, -6.0, -12.0, -24.0, -48.0, -60.0] {
            let y = scale.bottom() - (tick + 60.0) / 66.0206 * scale.height();
            ui.painter().line_segment(
                [Pos2::new(scale.left(), y), Pos2::new(scale.left() + 3.0, y)],
                Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
            );
            ui.painter().text(
                Pos2::new(scale.left() + 5.0, y),
                egui::Align2::LEFT_CENTER,
                format!("{tick:.0}"),
                egui::FontId::monospace(8.0),
                ui.visuals().weak_text_color(),
            );
        }
        ui.allocate_space(Vec2::new(30.0, 150.0));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuning_readouts_do_not_move_controls_when_units_or_signs_change() {
        let ctx = egui::Context::default();
        let mut previous = None;
        for (speed, cents) in [(0.0, 0.0), (-2.0, -2400.0), (2.0, 2399.0), (1.0, -1.0)] {
            let mut sample = SlotSample::new("hit.wav".into(), "hit.wav".into(), None, 1.0);
            sample.playback_speed = speed;
            sample.pitch_semitones = cents / 100.0;
            let mut retrigger = true;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 80.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        layer_strip(ui, &mut sample, &mut retrigger);
                    });
                },
            );
            let positions: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text)
                        if ["Retrigger", "Speed", "Pitch", "Edit in Audio Studio"]
                            .contains(&text.galley.text()) =>
                    {
                        Some((text.galley.text().to_owned(), text.pos))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(positions.len(), 4);
            if let Some(previous) = &previous {
                assert_eq!(&positions, previous);
            }
            previous = Some(positions);
            assert_eq!(sample.playback_speed, speed);
            assert_eq!((sample.pitch_semitones * 100.0).round(), cents);
        }
    }
}
