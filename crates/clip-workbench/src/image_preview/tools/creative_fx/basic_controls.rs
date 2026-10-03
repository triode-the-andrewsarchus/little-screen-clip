use super::*;
use little_clip_ops::creative_fx::{CurvePoint, tone_curve_value};

impl ImagePreviewState {
    pub(super) fn render_basic_controls(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let p = &mut self.creative_fx_state.basic_params;
        let mut changed = false;
        for (label, value) in [
            ("Contrast", &mut p.contrast),
            ("Saturation", &mut p.saturation),
            ("Highlights", &mut p.highlights),
            ("Shadows", &mut p.shadows),
        ] {
            changed |=
                render_context_slider(ui, label, value, -100.0..=100.0, 0.0, "", slider_w, palette);
            ui.add_space(crate::ui_ext::SPACE_XS);
        }
        ui.label("RGB tone curve");
        changed |= render_tone_curve(ui, &mut p.curve, palette);
        ui.small("Click to add · drag to move · right-click to delete");
        if ui.button("Reset curve").clicked() {
            p.curve = BasicControlsParams::default().curve;
            changed = true;
        }
        if changed {
            self.creative_fx_state.mark_custom();
        }
    }

    pub(super) fn render_basic_masked_zoom(
        &mut self,
        ui: &mut egui::Ui,
        palette: &AppPalette,
        slider_w: f32,
    ) {
        let p = &mut self.creative_fx_state.basic_params;
        if !p.depth_mask.enabled {
            return;
        }
        ui.add_space(crate::ui_ext::SPACE_XS);
        let mut changed = render_context_slider(
            ui,
            "Masked zoom",
            &mut p.masked_zoom,
            1.0..=4.0,
            1.0,
            "×",
            slider_w,
            palette,
        );
        if p.depth_mask.source == CreativeFxMaskSource::DepthMap {
            ui.label("Depth slices");
            ui.spacing_mut().slider_width = slider_w;
            let slices = ui.add(egui::Slider::new(&mut p.depth_slices, 1..=32).integer());
            changed |= slices.changed();
            if slider_double_clicked(ui, &slices) {
                p.depth_slices = 8;
                changed = true;
            }
            changed |= render_context_slider(
                ui,
                "Zoom falloff",
                &mut p.zoom_falloff,
                0.1..=4.0,
                1.0,
                "",
                slider_w,
                palette,
            );
            ui.small("Higher falloff keeps far slices smaller.");
        }
        if changed {
            self.creative_fx_state.mark_custom();
        }
    }
}

/// A curve widget sharing exactly the interpolation used by the pixel renderer.
fn render_tone_curve(
    ui: &mut egui::Ui,
    points: &mut Vec<CurvePoint>,
    palette: &AppPalette,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 150.0),
        egui::Sense::click_and_drag(),
    );
    let plot = rect.shrink(8.0);
    let to_screen = |p: &CurvePoint| {
        Pos2::new(
            plot.left() + p.input * plot.width(),
            plot.bottom() - p.output * plot.height(),
        )
    };
    let changed = edit_tone_curve(&response, plot, points);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, hex_to_c32(&palette.colors.surface_recessed));
    let grid = Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel));
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let x = plot.left() + t * plot.width();
        let y = plot.bottom() - t * plot.height();
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            grid,
        );
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            grid,
        );
    }
    painter.line_segment([plot.left_bottom(), plot.right_top()], grid);
    let line: Vec<_> = (0..=128)
        .map(|i| {
            let input = i as f32 / 128.0;
            to_screen(&CurvePoint {
                input,
                output: tone_curve_value(points, input),
            })
        })
        .collect();
    let accent = hex_to_c32(&palette.harmonies.base);
    painter.add(egui::Shape::line(line, Stroke::new(2.0_f32, accent)));
    for point in points {
        painter.circle_filled(to_screen(point), 4.5, accent);
        painter.circle_stroke(
            to_screen(point),
            4.5,
            Stroke::new(1.0_f32, hex_to_c32(&palette.colors.text_primary)),
        );
    }
    changed
}

fn edit_tone_curve(response: &egui::Response, plot: Rect, points: &mut Vec<CurvePoint>) -> bool {
    let id = response.id.with("drag_point");
    let mut changed = false;
    if let Some(pos) = response.interact_pointer_pos() {
        let pick_pos = if response.drag_started() {
            response
                .ctx
                .input(|i| i.pointer.press_origin())
                .unwrap_or(pos)
        } else {
            pos
        };
        let nearest = points.iter().position(|p| {
            Pos2::new(
                plot.left() + p.input * plot.width(),
                plot.bottom() - p.output * plot.height(),
            )
            .distance(pick_pos)
                <= 10.0
        });
        if response.secondary_clicked() {
            if let Some(i) = nearest.filter(|i| *i > 0 && *i + 1 < points.len()) {
                points.remove(i);
                changed = true;
            }
        } else if response.drag_started() || response.clicked() {
            let (index, added) = select_curve_point(points, plot, pick_pos, nearest);
            changed |= added;
            response.ctx.data_mut(|data| data.insert_temp(id, index));
        }
        if response.dragged()
            && let Some(i) = response
                .ctx
                .data(|data| data.get_temp::<usize>(id))
                .filter(|i| *i < points.len())
        {
            changed |= move_curve_point(points, plot, pos, i);
        }
    }
    if response.drag_stopped() || response.clicked() || response.secondary_clicked() {
        response.ctx.data_mut(|data| data.remove::<usize>(id));
    }
    changed
}

fn select_curve_point(
    points: &mut Vec<CurvePoint>,
    plot: Rect,
    pos: Pos2,
    nearest: Option<usize>,
) -> (usize, bool) {
    if let Some(i) = nearest {
        return (i, false);
    }
    let input = ((pos.x - plot.left()) / plot.width()).clamp(0.001, 0.999);
    let i = points
        .partition_point(|p| p.input < input)
        .clamp(1, points.len() - 1);
    if input - points[i - 1].input <= 0.001 || points[i].input - input <= 0.001 {
        return (i, false);
    }
    points.insert(
        i,
        CurvePoint {
            input,
            output: ((plot.bottom() - pos.y) / plot.height()).clamp(0.0, 1.0),
        },
    );
    (i, true)
}

fn move_curve_point(points: &mut [CurvePoint], plot: Rect, pos: Pos2, index: usize) -> bool {
    let input = if index == 0 {
        0.0
    } else if index + 1 == points.len() {
        1.0
    } else {
        ((pos.x - plot.left()) / plot.width()).clamp(
            points[index - 1].input + 0.001,
            points[index + 1].input - 0.001,
        )
    };
    let point = CurvePoint {
        input,
        output: ((plot.bottom() - pos.y) / plot.height()).clamp(0.0, 1.0),
    };
    let changed = points[index] != point;
    points[index] = point;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, points: &mut Vec<CurvePoint>, events: Vec<egui::Event>) -> bool {
        ctx.begin_frame(egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 220.0))),
            events,
            ..Default::default()
        });
        let changed = egui::CentralPanel::default()
            .show(ctx, |ui| {
                render_tone_curve(ui, points, &AppPalette::default())
            })
            .inner;
        let _ = ctx.end_frame();
        changed
    }

    fn button(pos: Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn curve_widget_adds_drags_and_deletes_points_with_real_egui_input() {
        let ctx = egui::Context::default();
        let mut points = BasicControlsParams::default().curve;
        frame(&ctx, &mut points, vec![]);
        let pos = Pos2::new(150.0, 75.0);
        frame(
            &ctx,
            &mut points,
            vec![
                egui::Event::PointerMoved(pos),
                button(pos, egui::PointerButton::Primary, true),
            ],
        );
        assert!(frame(
            &ctx,
            &mut points,
            vec![button(pos, egui::PointerButton::Primary, false)]
        ));
        assert_eq!(points.len(), 3);
        let original = points[1];
        frame(
            &ctx,
            &mut points,
            vec![button(pos, egui::PointerButton::Primary, true)],
        );
        let end = Pos2::new(195.0, 42.0);
        assert!(frame(
            &ctx,
            &mut points,
            vec![egui::Event::PointerMoved(end)]
        ));
        frame(
            &ctx,
            &mut points,
            vec![button(end, egui::PointerButton::Primary, false)],
        );
        assert_eq!(points.len(), 3);
        assert!(points[1].input > original.input && points[1].output > original.output);
        frame(
            &ctx,
            &mut points,
            vec![button(end, egui::PointerButton::Secondary, true)],
        );
        assert!(frame(
            &ctx,
            &mut points,
            vec![button(end, egui::PointerButton::Secondary, false)]
        ));
        assert_eq!(points, BasicControlsParams::default().curve);
    }

    #[test]
    fn curve_point_drag_preserves_endpoints_and_order_when_crossing_neighbors() {
        let plot = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 150.0));
        let mut points = BasicControlsParams::default().curve;
        select_curve_point(&mut points, plot, Pos2::new(50.0, 60.0), None);
        select_curve_point(&mut points, plot, Pos2::new(150.0, 60.0), None);
        move_curve_point(&mut points, plot, Pos2::new(199.0, -100.0), 1);
        assert!(points[1].input < points[2].input);
        assert_eq!(points[1].output, 1.0);
        move_curve_point(&mut points, plot, Pos2::new(150.0, 80.0), 0);
        assert_eq!(points[0].input, 0.0);
        move_curve_point(&mut points, plot, Pos2::new(-150.0, 80.0), 3);
        assert_eq!(points[3].input, 1.0);
    }
}
