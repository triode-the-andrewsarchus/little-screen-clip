//! Shared `tiny-skia` rasterizer for `eda_view::vector::Scene` primitives.

use super::super::{
    allocate_canvas_with_y_flip, check_cancelled, pixmap_to_thumbnail, update_bounds,
};
use crate::ImageThumbnail;
use eda_view::vector::{Layer, Prim, Scene};
use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

const MAX_SCENE_ITEMS: usize = 150_000;

/// Rasterizes an `eda_view::vector::Scene` onto a premultiplied RGBA thumbnail.
pub fn render_scene_thumbnail(
    scene: &Scene,
    max_size: [u32; 2],
    bg: Color,
    cancelled: &AtomicBool,
) -> Result<ImageThumbnail, String> {
    check_cancelled(cancelled, "KiCad scene render cancelled")?;
    let bounds = resolve_scene_bounds(scene);
    let (mut pixmap, transform, scale) =
        allocate_canvas_with_y_flip(bounds, max_size, bg, scene.y_down)
            .ok_or_else(|| "Could not allocate KiCad preview canvas".to_string())?;

    let layers: BTreeMap<u16, &Layer> = scene.layers.iter().map(|l| (l.id, l)).collect();
    let mut order: Vec<usize> = (0..scene.items.len().min(MAX_SCENE_ITEMS)).collect();
    order.sort_by_key(|&idx| layers.get(&scene.items[idx].layer).map_or(0, |l| l.z));

    for (step, item_idx) in order.into_iter().enumerate() {
        if step.is_multiple_of(1024) {
            check_cancelled(cancelled, "KiCad scene render cancelled")?;
        }
        let item = &scene.items[item_idx];
        if let Some(layer) = layers.get(&item.layer) {
            if !layer.visible {
                continue;
            }
            draw_scene_prim(&mut pixmap, &item.prim, layer.color, transform, scale);
        }
    }

    check_cancelled(cancelled, "KiCad scene render cancelled")?;
    Ok(pixmap_to_thumbnail(pixmap))
}

pub(super) fn resolve_scene_bounds(scene: &Scene) -> [f64; 4] {
    if !scene.bbox.is_empty()
        && scene.bbox.min[0].is_finite()
        && scene.bbox.min[1].is_finite()
        && scene.bbox.max[0].is_finite()
        && scene.bbox.max[1].is_finite()
    {
        return [
            scene.bbox.min[0],
            scene.bbox.min[1],
            scene.bbox.max[0],
            scene.bbox.max[1],
        ];
    }
    let mut bounds = None;
    for item in scene.items.iter().take(MAX_SCENE_ITEMS) {
        extend_prim_bounds(&mut bounds, &item.prim);
    }
    bounds.unwrap_or([0.0, 0.0, 100.0, 100.0])
}

fn extend_prim_bounds(bounds: &mut Option<[f64; 4]>, prim: &Prim) {
    match prim {
        Prim::Polyline { points, .. } => {
            for p in points {
                update_bounds(bounds, p[0], p[1]);
            }
        }
        Prim::Polygon { outer, .. } => {
            for p in outer {
                update_bounds(bounds, p[0], p[1]);
            }
        }
        Prim::Circle { center, radius, .. } => {
            update_bounds(bounds, center[0] - radius, center[1] - radius);
            update_bounds(bounds, center[0] + radius, center[1] + radius);
        }
        Prim::Arc { center, radius, .. } => {
            update_bounds(bounds, center[0] - radius, center[1] - radius);
            update_bounds(bounds, center[0] + radius, center[1] + radius);
        }
        Prim::Strokes { strokes, .. } => {
            for s in strokes {
                for p in s {
                    update_bounds(bounds, p[0], p[1]);
                }
            }
        }
        Prim::Hole { center, size, .. } => {
            let r = size[0].max(size[1]) * 0.5;
            update_bounds(bounds, center[0] - r, center[1] - r);
            update_bounds(bounds, center[0] + r, center[1] + r);
        }
    }
}

fn draw_scene_prim(
    pixmap: &mut Pixmap,
    prim: &Prim,
    rgba: [u8; 4],
    transform: Transform,
    scale: f32,
) {
    if rgba[3] == 0 {
        return;
    }
    let min_stroke = f64::from(1.0 / scale.max(0.01));
    match prim {
        Prim::Polyline { points, width } => {
            draw_open_polyline(pixmap, points, (*width).max(min_stroke), rgba, transform);
        }
        Prim::Polygon {
            outer,
            holes,
            fill,
            stroke,
        } => {
            draw_polygon(
                pixmap, outer, holes, *fill, *stroke, min_stroke, rgba, transform,
            );
        }
        Prim::Circle {
            center,
            radius,
            fill,
            stroke,
        } => {
            draw_circle(
                pixmap, *center, *radius, *fill, *stroke, min_stroke, rgba, transform,
            );
        }
        Prim::Arc {
            center,
            radius,
            start,
            end,
            width,
        } => {
            draw_arc(
                pixmap,
                *center,
                *radius,
                *start,
                *end,
                (*width).max(min_stroke),
                rgba,
                transform,
            );
        }
        Prim::Strokes { strokes, width } => {
            let w = (*width).max(min_stroke * 0.9);
            for poly in strokes {
                draw_open_polyline(pixmap, poly, w, rgba, transform);
            }
        }
        Prim::Hole {
            center,
            size,
            rotation,
        } => {
            draw_obround(
                pixmap, *center, *size, *rotation, min_stroke, rgba, transform,
            );
        }
    }
}

fn paint_from_rgba(rgba: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = true;
    paint
}

fn stroke_with_width(width: f64) -> Stroke {
    Stroke {
        width: width.max(0.01) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

fn draw_open_polyline(
    pixmap: &mut Pixmap,
    points: &[[f64; 2]],
    width: f64,
    rgba: [u8; 4],
    transform: Transform,
) {
    if points.len() < 2 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(points[0][0] as f32, points[0][1] as f32);
    for p in &points[1..] {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
    if let Some(path) = pb.finish() {
        let paint = paint_from_rgba(rgba);
        pixmap.stroke_path(&path, &paint, &stroke_with_width(width), transform, None);
    }
}

fn append_closed_ring(pb: &mut PathBuilder, ring: &[[f64; 2]]) {
    if ring.len() < 3 {
        return;
    }
    pb.move_to(ring[0][0] as f32, ring[0][1] as f32);
    for p in &ring[1..] {
        pb.line_to(p[0] as f32, p[1] as f32);
    }
    pb.close();
}

#[allow(clippy::too_many_arguments)]
fn draw_polygon(
    pixmap: &mut Pixmap,
    outer: &[[f64; 2]],
    holes: &[Vec<[f64; 2]>],
    fill: bool,
    stroke: f64,
    min_stroke: f64,
    rgba: [u8; 4],
    transform: Transform,
) {
    if outer.len() < 3 {
        return;
    }
    let mut pb = PathBuilder::new();
    append_closed_ring(&mut pb, outer);
    for hole in holes {
        append_closed_ring(&mut pb, hole);
    }
    let Some(path) = pb.finish() else {
        return;
    };
    let paint = paint_from_rgba(rgba);
    if fill {
        pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
    }
    if stroke > 0.0 || !fill {
        let w = if stroke > 0.0 {
            stroke.max(min_stroke)
        } else {
            min_stroke
        };
        pixmap.stroke_path(&path, &paint, &stroke_with_width(w), transform, None);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_circle(
    pixmap: &mut Pixmap,
    center: [f64; 2],
    radius: f64,
    fill: bool,
    stroke: f64,
    min_stroke: f64,
    rgba: [u8; 4],
    transform: Transform,
) {
    let r = radius.abs().max(min_stroke * 0.5) as f32;
    let mut pb = PathBuilder::new();
    pb.push_circle(center[0] as f32, center[1] as f32, r);
    let Some(path) = pb.finish() else {
        return;
    };
    let paint = paint_from_rgba(rgba);
    if fill {
        pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
    }
    if stroke > 0.0 || !fill {
        let w = if stroke > 0.0 {
            stroke.max(min_stroke)
        } else {
            min_stroke
        };
        pixmap.stroke_path(&path, &paint, &stroke_with_width(w), transform, None);
    }
}

/// Appends line segments along an arc from `start_rad` to `end_rad`.
pub(crate) fn append_arc_points(
    pb: &mut PathBuilder,
    cx: f64,
    cy: f64,
    radius: f64,
    start_rad: f64,
    end_rad: f64,
    move_start: bool,
) {
    let mut sweep = end_rad - start_rad;
    if sweep.abs() > 2.0 * PI {
        sweep = sweep.rem_euclid(2.0 * PI);
    }
    let steps = ((sweep.abs() / (PI / 24.0)).ceil() as usize).clamp(8, 128);
    for step in 0..=steps {
        let theta = start_rad + sweep * (step as f64 / steps as f64);
        let x = (cx + radius * theta.cos()) as f32;
        let y = (cy + radius * theta.sin()) as f32;
        if step == 0 && move_start {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_arc(
    pixmap: &mut Pixmap,
    center: [f64; 2],
    radius: f64,
    start: f64,
    end: f64,
    width: f64,
    rgba: [u8; 4],
    transform: Transform,
) {
    if radius <= 1e-9 || !radius.is_finite() {
        return;
    }
    let mut pb = PathBuilder::new();
    append_arc_points(&mut pb, center[0], center[1], radius, start, end, true);
    if let Some(path) = pb.finish() {
        let paint = paint_from_rgba(rgba);
        pixmap.stroke_path(&path, &paint, &stroke_with_width(width), transform, None);
    }
}

/// Centerline and diameter of an obround in the given coordinate frame.
pub(super) fn obround_axis(
    center: [f64; 2],
    size: [f64; 2],
    rotation: f64,
) -> ([[f64; 2]; 2], f64) {
    let [w, h] = size.map(f64::abs);
    let (s, c) = rotation.sin_cos();
    let half_span = (w - h).abs() * 0.5;
    let (dx, dy) = if w >= h {
        (half_span * c, half_span * s)
    } else {
        (-half_span * s, half_span * c)
    };
    (
        [
            [center[0] - dx, center[1] - dy],
            [center[0] + dx, center[1] + dy],
        ],
        w.min(h),
    )
}

pub(super) fn draw_obround(
    pixmap: &mut Pixmap,
    center: [f64; 2],
    size: [f64; 2],
    rotation: f64,
    min_stroke: f64,
    rgba: [u8; 4],
    transform: Transform,
) {
    let [w, h] = size.map(f64::abs);
    if (w - h).abs() < 1e-6 {
        draw_circle(
            pixmap,
            center,
            w * 0.5,
            true,
            0.0,
            min_stroke,
            rgba,
            transform,
        );
        return;
    }
    let (axis, diameter) = obround_axis(center, size, rotation);
    draw_open_polyline(pixmap, &axis, diameter.max(min_stroke), rgba, transform);
}
