//! Fine adjustment within the window fixed by the original selection.
use super::{Selection, focus_id, preview_copy, unit_uv};
use crate::file_manager::video_timeline::selection::Edge;
use egui::{
    Color32, CursorIcon, PointerButton, Rect, Response, Sense, Stroke, TextureHandle, Ui, pos2,
    vec2,
};

#[derive(Clone, Copy)]
struct Drag {
    edge: Edge,
    grab_offset: f32,
}

pub(super) fn show(ui: &mut Ui, selection: &mut Selection, overview: Option<&TextureHandle>) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::drag());
    let width = (rect.width() * ui.ctx().pixels_per_point())
        .ceil()
        .clamp(1.0, 2048.0) as u32;
    selection.request_detail(ui.ctx(), width);
    paint_strip(ui, rect, selection, overview);

    let origin = selection.window.start;
    let length = (selection.window.end - origin) as f64;
    let fraction = |time: u64| ((time - origin) as f64 / length) as f32;
    let x_at = |time| egui::lerp(rect.x_range(), fraction(time));
    let handles = [x_at(selection.clip.start_ms), x_at(selection.clip.end_ms)];
    let dragging = response.dragged_by(PointerButton::Primary);
    let drag = drag_handle(ui, &response, handles, dragging);
    if let Some(drag) = drag
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let proportion =
            ((pointer.x - drag.grab_offset - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let time = origin + (f64::from(proportion) * length).round() as u64;
        selection.set_edge(drag.edge, time);
        ui.memory_mut(|memory| memory.request_focus(focus_id()));
    }
    if !dragging {
        selection.finish_adjustment(ui.ctx());
    }
    response
        .on_hover_cursor(CursorIcon::ResizeHorizontal)
        .on_hover_text("Drag either handle to adjust the clip");

    let (start, end) = (selection.clip.start_ms, selection.clip.end_ms);
    preview_copy::paint_range(ui, rect, (fraction(start), fraction(end)));
    for x in [x_at(start), x_at(end)] {
        paint_handle(ui, rect, x);
    }
}

fn drag_handle(
    ui: &Ui,
    response: &Response,
    [start, end]: [f32; 2],
    dragging: bool,
) -> Option<Drag> {
    let id = response.id.with("edge");
    if response.drag_started_by(PointerButton::Primary)
        && let Some(press) = ui.input(|input| input.pointer.press_origin())
    {
        let (edge, x) = if (press.x - start).abs() < (press.x - end).abs() {
            (Edge::Start, start)
        } else {
            (Edge::End, end)
        };
        ui.data_mut(|data| {
            data.insert_temp(
                id,
                Drag {
                    edge,
                    grab_offset: press.x - x,
                },
            )
        });
    }
    ui.data_mut(|data| {
        let drag = data.get_temp::<Drag>(id);
        if !dragging {
            data.remove::<Drag>(id);
        }
        drag
    })
}

fn paint_strip(ui: &Ui, rect: Rect, selection: &Selection, overview: Option<&TextureHandle>) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
    let duration = selection.clip.source.metadata.duration_ms as f64;
    let sampled = selection
        .detail_texture
        .as_ref()
        .map(|texture| (texture, unit_uv()))
        .or_else(|| {
            let uv = Rect::from_min_max(
                pos2((selection.window.start as f64 / duration) as f32, 0.0),
                pos2((selection.window.end as f64 / duration) as f32, 1.0),
            );
            overview.map(|texture| (texture, uv))
        });
    if let Some((texture, uv)) = sampled {
        painter.image(texture.id(), rect, uv, Color32::WHITE);
    }
}

fn paint_handle(ui: &Ui, rect: Rect, x: f32) {
    let painter = ui.painter_at(rect);
    let grip = Rect::from_center_size(pos2(x, rect.center().y), vec2(8.0, 22.0));
    painter.line_segment(
        [pos2(x, rect.top()), pos2(x, rect.bottom())],
        Stroke::new(2.0, Color32::WHITE),
    );
    painter.rect_filled(grip.expand(1.0), 2.0, Color32::BLACK);
    painter.rect_filled(grip, 2.0, Color32::WHITE);
}
