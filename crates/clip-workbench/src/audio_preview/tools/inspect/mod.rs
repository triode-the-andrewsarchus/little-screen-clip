mod selection;
mod studio;
mod waveform;

use super::super::*;
use little_clip_ops::hf_audio::HfSpectrogramConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CanvasEffect {
    Gain,
    LowPass,
    HighPass,
}

const INSPECT_MIN_CANVAS_HEIGHT: f32 = 226.0;
const INSPECT_HELP_TEXT: &str = "Wheel / +/-: Zoom • Drag: Select • Ctrl+C: Copy • Ctrl+S: Save";

fn inspect_footer_galley(
    ui: &egui::Ui,
    palette: &AppPalette,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let font_id = egui::FontId::proportional(crate::ui_ext::BODY_FONT_SIZE);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "Navigation:",
        0.0,
        egui::TextFormat {
            font_id: font_id.clone(),
            color: hex_to_c32(&palette.colors.text_secondary),
            ..Default::default()
        },
    );
    job.append(
        INSPECT_HELP_TEXT,
        8.0,
        egui::TextFormat {
            font_id,
            color: hex_to_c32(&palette.colors.text_muted),
            ..Default::default()
        },
    );
    job.wrap.max_width = width.max(1.0);
    ui.fonts(|fonts| fonts.layout_job(job))
}

#[cfg(test)]
mod effect_tests;

fn control_wheel_delta(events: &[egui::Event], page_size: f32) -> f32 {
    events
        .iter()
        .filter_map(|event| match event {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } if modifiers.ctrl => {
                let points_per_unit = match unit {
                    egui::MouseWheelUnit::Point => 1.0,
                    egui::MouseWheelUnit::Line => 50.0,
                    egui::MouseWheelUnit::Page => page_size,
                };
                Some((delta.x + delta.y) * points_per_unit)
            }
            _ => None,
        })
        .sum()
}

fn effect_at_position(pos: Pos2, wave_rect: Rect, spec_rect: Rect) -> Option<CanvasEffect> {
    if wave_rect.contains(pos) {
        Some(CanvasEffect::Gain)
    } else if spec_rect.contains(pos) {
        Some(if pos.y < spec_rect.center().y {
            CanvasEffect::LowPass
        } else {
            CanvasEffect::HighPass
        })
    } else {
        None
    }
}
