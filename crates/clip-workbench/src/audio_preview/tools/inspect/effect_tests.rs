
use super::*;
use little_clip_core::config::WorkbenchAppearance;

#[test]
fn inspect_play_and_pause_keep_the_same_button_width() {
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);
    let palette =
        crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
    let mut widths = Vec::new();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let width =
                crate::ui_ext::equal_button_width(ui, &["Play [Space]", "Pause [Space]"], true)
                    + 1.0;
            for (label, icon) in [
                ("Play [Space]", crate::icons::Icon::Play),
                ("Pause [Space]", crate::icons::Icon::Pause),
            ] {
                widths.push(
                    ui.add(
                        layout::AudioWorkspace::primary_button_icon(
                            Some(icon),
                            label,
                            hex_to_c32(&palette.diff_add),
                            &palette,
                        )
                        .min_size(Vec2::new(width, crate::ui_ext::CONTROL_HEIGHT)),
                    )
                    .rect
                    .size(),
                );
            }
        });
    });
    assert_eq!(widths.len(), 2);
    assert_eq!(widths[0], widths[1]);
    assert_eq!(widths[0].y, crate::ui_ext::CONTROL_HEIGHT);
}

#[test]
fn waveform_channel_labels_have_readable_contrast_in_both_appearances() {
    for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
        let palette = crate::ui_appearance::palette(appearance);
        let background = crate::hex_to_c32(&palette.colors.surface_recessed);
        let foreground = crate::hex_to_c32(&palette.harmonies.base);
        let contrast = little_clip_core::palette::contrast_ratio(
            (background.r(), background.g(), background.b()),
            (foreground.r(), foreground.g(), foreground.b()),
        );
        assert!(
            contrast >= 4.5,
            "{appearance:?} waveform channel labels have only {contrast:.2}:1 contrast"
        );
    }
}

#[test]
fn wheel_effects_use_waveform_and_respective_spectrum_halves() {
    let wave = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
    let spec = Rect::from_min_max(Pos2::new(0.0, 110.0), Pos2::new(100.0, 210.0));
    assert_eq!(
        effect_at_position(Pos2::new(50.0, 50.0), wave, spec),
        Some(CanvasEffect::Gain)
    );
    assert_eq!(
        effect_at_position(Pos2::new(50.0, 130.0), wave, spec),
        Some(CanvasEffect::LowPass)
    );
    assert_eq!(
        effect_at_position(Pos2::new(50.0, 190.0), wave, spec),
        Some(CanvasEffect::HighPass)
    );
    assert_eq!(effect_at_position(Pos2::new(50.0, 230.0), wave, spec), None);
    assert_eq!(effect_at_position(Pos2::new(110.0, 50.0), wave, spec), None);
}

#[test]
fn control_wheel_uses_raw_mouse_wheel_events() {
    let events = [
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: Vec2::new(0.0, 1.0),
            modifiers: egui::Modifiers {
                ctrl: true,
                ..Default::default()
            },
        },
        egui::Event::Zoom(1.25),
    ];

    assert_eq!(control_wheel_delta(&events, 300.0), 50.0);
}

#[test]
fn control_wheel_ignores_unmodified_wheel_events() {
    let events = [egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: Vec2::new(0.0, 20.0),
        modifiers: egui::Modifiers::default(),
    }];

    assert_eq!(control_wheel_delta(&events, 300.0), 0.0);
}
