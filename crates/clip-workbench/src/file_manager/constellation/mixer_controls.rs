//! Pad-layer mixer and compact performance strip with independent speed and pitch.
use super::sample_pack::SlotSample;
use egui::{Pos2, Rect, Sense, Stroke, Vec2};

/// Places the marked level fader beside the waveform.
pub(super) fn mixer(ui: &mut egui::Ui, sample: &mut SlotSample) {
    ui.small("Gain");
    gain_fader(ui, &mut sample.volume);
}

/// Wraps whole sections in playback order, reserving each button's measured width.
pub(super) fn layer_strip(
    ui: &mut egui::Ui,
    sample: &mut SlotSample,
    retrigger: &mut bool,
) -> bool {
    let text_width = |label: &str, style: egui::TextStyle| {
        ui.painter()
            .layout_no_wrap(
                label.into(),
                style.resolve(ui.style()),
                ui.visuals().text_color(),
            )
            .size()
            .x
    };
    let padding = ui.spacing().button_padding.x * 2.0 + 4.0;
    let mut widths = [
        text_width("Retrigger", egui::TextStyle::Button) + padding,
        text_width("Pan", egui::TextStyle::Small) + 66.0,
        text_width("Speed", egui::TextStyle::Small) + 124.0,
        text_width("Pitch", egui::TextStyle::Small) + 124.0,
        text_width("Edit in Audio Studio", egui::TextStyle::Body) + padding + 10.0,
    ];
    let gap = 6.0;
    // Reserve rounding slack so native controls cannot force the final section onto another row.
    let spare =
        (ui.available_width() - widths.iter().sum::<f32>() - gap * 4.0 - 16.0).max(0.0) / 3.0;
    for width in &mut widths[1..4] {
        *width += spare;
    }
    let mut studio = false;
    let bounds = ui.available_rect_before_wrap();
    let row_height = ui.spacing().interact_size.y.max(
        ui.text_style_height(&ui.style().drag_value_text_style)
            .max(ui.text_style_height(&egui::TextStyle::Button))
            .max(ui.text_style_height(&egui::TextStyle::Body))
            + ui.spacing().button_padding.y * 2.0,
    );
    let mut cursor = bounds.min;
    for (section, width) in widths.into_iter().enumerate() {
        if cursor.x + width > bounds.right() && cursor.x > bounds.left() {
            cursor = Pos2::new(bounds.left(), cursor.y + row_height + 4.0);
        }
        if section == 4 {
            cursor.x = bounds.right() - width;
        }
        let rect = Rect::from_min_size(cursor, Vec2::new(width, row_height));
        let mut child = ui.child_ui(rect, egui::Layout::left_to_right(egui::Align::Center));
        // Native button outlines extend half a point beyond their layout bounds.
        child.set_clip_rect(ui.clip_rect().intersect(rect.expand(1.0)));
        child.spacing_mut().interact_size.y = row_height;
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 4.0;
        if section != 0 {
            ui.separator();
        }
        let width = (ui.available_width() - 4.0).max(0.0);
        match section {
            0 => {
                if super::ui::outlined_toggle(
                    ui,
                    "Retrigger",
                    *retrigger,
                    ui.visuals().hyperlink_color,
                )
                .on_hover_text("Restart this pad on every hit. Off allows overlapping hits.")
                .clicked()
                {
                    *retrigger = !*retrigger;
                }
            }
            1 => {
                bipolar_slider(ui, "Pan", &mut sample.pan, 1.0, 0.01, 0.0, width);
            }
            2 => {
                fader(
                    ui,
                    "Speed",
                    &mut sample.playback_speed,
                    2.0,
                    0.01,
                    1.0,
                    width,
                );
            }
            3 => {
                let mut cents = (sample.pitch_semitones * 100.0).round();
                fader(ui, "Pitch", &mut cents, 2400.0, 1.0, 0.0, width);
                sample.pitch_semitones = cents / 100.0;
            }
            _ => {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    studio = ui
                        .add(
                            egui::Button::new("Edit in Audio Studio")
                                .small()
                                .wrap(false),
                        )
                        .on_hover_text(&sample.filename)
                        .clicked();
                });
            }
        }
        cursor.x = rect.right() + gap;
    }
    ui.allocate_rect(
        Rect::from_min_max(
            bounds.min,
            Pos2::new(bounds.right(), cursor.y + row_height + 1.0),
        ),
        Sense::hover(),
    );

    studio
}

/// Shares center-detent interaction across Pan, Speed and Pitch tracks.
fn bipolar_slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    limit: f32,
    step: f64,
    reset: f32,
    width: f32,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y.max(20.0)),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let title = ui.small(label);
            ui.spacing_mut().slider_width = (width - title.rect.width() - 4.0).max(12.0);
            let step = if label == "Pitch" {
                ui.input(|input| if input.modifiers.ctrl || input.modifiers.shift { 1.0 } else { 100.0 })
            } else {
                step
            };
            let response = ui.add(
                egui::Slider::new(value, -limit..=limit)
                    .step_by(step)
                    .show_value(false)
                    .clamp_to_range(false),
            );
            let double_clicked =
                crate::shell::studio_controls::slider_double_clicked(ui, &response);
            if response.changed() {
                *value = value.clamp(-limit, limit);
            }
            // A small physical detent aids pointer use; the readout still accepts single cents.
            if response.dragged() && value.abs() < step as f32 * 0.5 {
                *value = 0.0;
            }
            if double_clicked {
                *value = reset;
            }
            let center = response.rect.center();
            ui.painter().line_segment(
                [center + Vec2::new(0.0, 6.0), center + Vec2::new(0.0, 9.0)],
                Stroke::new(1.0_f32, ui.visuals().weak_text_color()),
            );
            response.clone().on_hover_text(if label == "Pan" {
                "Stereo balance · center detent · double-click to reset"
            } else if label == "Speed" {
                "−2× to +2×. Negative plays backwards; 0 stops. Double-click for +1×."
            } else {
                "Semitone steps; hold Ctrl or Shift for cents, or type an offset. Double-click to reset."
            });
            response
        },
    )
    .inner
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
) -> egui::Response {
    ui.allocate_ui_with_layout(
        Vec2::new(width, ui.spacing().interact_size.y.max(20.0)),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let response = bipolar_slider(ui, label, value, limit, step, reset, width - 70.0);
            // Limit user edits while retaining the exact rate of old Time-mode packs (up to 4×).
            let mut edited = *value;
            let readout = ui.add_sized(
                Vec2::new(66.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut edited)
                    .speed(step)
                    .custom_parser(move |text| {
                        if label == "Pitch" {
                            parse_pitch_offset(text)
                        } else {
                            text.trim()
                                .trim_end_matches('×')
                                .trim()
                                .parse::<f64>()
                                .ok()
                                .filter(|v| v.is_finite())
                        }
                    })
                    .custom_formatter(move |v, _| {
                        if label == "Speed" {
                            format!("{v:+.2}×")
                        } else {
                            format_pitch_offset(v)
                        }
                    }),
            );
            if readout.changed() {
                *value = edited.clamp(-limit, limit);
            }
            if readout.double_clicked() {
                *value = reset;
            }
            response
        },
    )
    .inner
}

/// Accepts cents by default, with explicit semitone and octave units.
fn parse_pitch_offset(text: &str) -> Option<f64> {
    let text = text.trim();
    let (number, scale) = [("oct", 1200.0), ("st", 100.0), ("ct", 1.0)]
        .into_iter()
        .find_map(|(suffix, scale)| text.strip_suffix(suffix).map(|number| (number, scale)))
        .unwrap_or((text, 1.0));
    number
        .trim()
        .parse::<f64>()
        .ok()
        .map(|v| v * scale)
        .filter(|v| v.is_finite())
}

fn format_pitch_offset(cents: f64) -> String {
    if cents % 1200.0 == 0.0 {
        format!("{:+.0} oct", cents / 1200.0)
    } else if cents % 100.0 == 0.0 {
        format!("{:+.0} st", cents / 100.0)
    } else {
        format!("{cents:+.0} ct")
    }
}

/// A tall, marked dB fader; its silent lower stop does not require a numeric field.
fn gain_fader(ui: &mut egui::Ui, volume: &mut f32) -> egui::Response {
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
        let double_clicked = crate::shell::studio_controls::slider_double_clicked(ui, &response);
        if double_clicked {
            db = 0.0;
        }
        if response.changed() || double_clicked {
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
        response
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_units_round_trip_and_keep_cent_precision() {
        for (value, text) in [
            (0.0, "+0 oct"),
            (100.0, "+1 st"),
            (-700.0, "-7 st"),
            (2400.0, "+2 oct"),
            (137.0, "+137 ct"),
        ] {
            assert_eq!(format_pitch_offset(value), text);
            assert_eq!(parse_pitch_offset(text), Some(value));
        }
        assert_eq!(parse_pitch_offset("123"), Some(123.0));
        assert_eq!(parse_pitch_offset("1.25 st"), Some(125.0));
        assert_eq!(parse_pitch_offset("NaN"), None);
    }

    #[test]
    fn pitch_slider_steps_in_semitones_unless_a_precision_modifier_is_held() {
        for modifiers in [
            egui::Modifiers::NONE,
            egui::Modifiers::CTRL,
            egui::Modifiers::SHIFT,
        ] {
            let ctx = egui::Context::default();
            let mut value = 137.0;
            let mut rect = Rect::NOTHING;
            let mut draw = |events| {
                let _ = ctx.run(
                    egui::RawInput {
                        modifiers,
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            rect = bipolar_slider(ui, "Pitch", &mut value, 2400.0, 1.0, 0.0, 400.0)
                                .rect;
                        });
                    },
                );
            };
            draw(vec![]);
            drop(draw);
            assert_eq!(value, 137.0, "Idle rendering must preserve typed cents");
            let point = egui::pos2(rect.left() + rect.width() * 0.573, rect.center().y);
            let _ = ctx.run(
                egui::RawInput {
                    modifiers,
                    events: vec![
                        egui::Event::PointerMoved(point),
                        egui::Event::PointerButton {
                            pos: point,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        bipolar_slider(ui, "Pitch", &mut value, 2400.0, 1.0, 0.0, 400.0);
                    });
                },
            );
            assert_eq!(value % 100.0 == 0.0, modifiers == egui::Modifiers::NONE);
        }
    }

    #[test]
    fn double_click_resets_each_slider_to_its_own_default() {
        for (label, initial, limit, step, reset) in [
            ("Gain", 0.25, 0.0, 0.0, 1.0),
            ("Pan", -0.75, 1.0, 0.01, 0.0),
            ("Speed", -1.5, 2.0, 0.01, 1.0),
            ("Pitch", 1700.0, 2400.0, 1.0, 0.0),
        ] {
            let ctx = egui::Context::default();
            let mut value = initial;
            let mut slider = Rect::NOTHING;
            let mut render = |time, events| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 240.0))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            slider = if label == "Gain" {
                                gain_fader(ui, &mut value).rect
                            } else if label == "Pan" {
                                bipolar_slider(ui, label, &mut value, limit, step, reset, 360.0)
                                    .rect
                            } else {
                                fader(ui, label, &mut value, limit, step, reset, 360.0).rect
                            };
                        });
                    },
                );
                (slider.center(), value)
            };
            let (pos, _) = render(0.0, vec![]);
            for (time, pressed) in [(0.1, true), (0.15, false), (0.2, true), (0.25, false)] {
                let (_, current) = render(
                    time,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
                if time == 0.15 {
                    assert_ne!(
                        current, initial,
                        "{label} must still respond to ordinary clicks"
                    );
                }
            }
            assert!(
                (value - reset).abs() < 0.0001,
                "{label}: {value} must reset to {reset}"
            );
        }
    }

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
                        if ["Retrigger", "Pan", "Speed", "Pitch", "Edit in Audio Studio"]
                            .contains(&text.galley.text()) =>
                    {
                        Some((text.galley.text().to_owned(), text.pos))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(positions.len(), 5);
            if let Some(previous) = &previous {
                assert_eq!(&positions, previous);
            }
            previous = Some(positions);
            assert_eq!(sample.playback_speed, speed);
            assert_eq!((sample.pitch_semitones * 100.0).round(), cents);
        }
    }
}
