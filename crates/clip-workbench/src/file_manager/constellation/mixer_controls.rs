//! Compact mixer controls shared by pad-layer level, balance and tuning.
use super::sample_pack::SlotSample;
use egui::{Pos2, Rect, Sense, Stroke, Vec2};

/// A bipolar rotary control with a small zero detent and double-click reset.
pub(super) fn knob(ui: &mut egui::Ui, label: &str, value: &mut f32, limit: f32) {
    ui.vertical(|ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click_and_drag());
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
    });
}

pub(super) fn mixer(ui: &mut egui::Ui, sample: &mut SlotSample, retrigger: &mut bool) {
    ui.horizontal(|ui| {
        ui.small("Gain");
        let color = if *retrigger {
            ui.visuals().hyperlink_color
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        if ui
            .add(
                egui::Button::new("R")
                    .min_size(Vec2::splat(22.0))
                    .rounding(2.0)
                    .stroke(Stroke::new(1.5_f32, color)),
            )
            .on_hover_text("Retrigger: stop the previous hit. Off allows overlapping hits.")
            .clicked()
        {
            *retrigger = !*retrigger;
        }
    });
    let mut db = if sample.volume > 0.0 {
        20.0 * sample.volume.log10()
    } else {
        -60.0
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = 84.0;
        let response = ui.add(
            egui::Slider::new(&mut db, -60.0..=6.0206)
                .vertical()
                .show_value(false),
        );
        if response.double_clicked() {
            db = 0.0;
        }
        if response.changed() || response.double_clicked() {
            sample.volume = if db <= -60.0 {
                0.0
            } else {
                10.0_f32.powf(db / 20.0)
            };
        }
        let scale = Rect::from_min_size(
            Pos2::new(response.rect.right() + 3.0, response.rect.top()),
            Vec2::new(30.0, response.rect.height()),
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
        ui.allocate_space(Vec2::new(30.0, 84.0));
    });
    knob(ui, "Pan", &mut sample.pan, 1.0);
}
