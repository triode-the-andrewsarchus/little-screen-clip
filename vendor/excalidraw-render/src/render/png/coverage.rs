//! Conservative device-space coverage estimators for the §13.2 raster ledger:
//! the axis-aligned pixel bounds a Rough drawing, freedraw stroke, text
//! element, placeholder or frame clip set can touch on the canvas, clamped to
//! the surface. They read only the plan — never the painted pixels — so the
//! ledger stays a pure function of the scene, and they over-approximate by
//! construction so no charge is ever below the surface actually written.

use super::super::freedraw;
use super::super::geometry::{union_bounds, PathOperation, Point};
use super::super::rough::{Drawing, Fill, FillKind};
use super::super::text::{LabelPathOperation, ResolvedText};
use super::{PngFrameClip, PngLabelMask, PngTransform};

/// The clipped conservative device-space integer bounds of a frame clip set on
/// a `surface_width` x `surface_height` surface, or `None` when the set clips
/// everything away.
pub(super) fn frame_clip_coverage_bounds(
    clips: &[PngFrameClip],
    surface_width: u32,
    surface_height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let (mut left, mut top, mut right, mut bottom) = (0, 0, surface_width, surface_height);
    for clip in clips {
        let bounds = clip.path.cubic_bounds()?;
        let points = [
            Point {
                x: bounds.0,
                y: bounds.1,
            },
            Point {
                x: bounds.2,
                y: bounds.1,
            },
            Point {
                x: bounds.2,
                y: bounds.3,
            },
            Point {
                x: bounds.0,
                y: bounds.3,
            },
        ]
        .map(|point| clip.transform.map_point(point));
        let clip_left = points
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .clamp(0.0, f64::from(surface_width)) as u32;
        let clip_top = points
            .iter()
            .map(|point| point.y)
            .fold(f64::INFINITY, f64::min)
            .floor()
            .clamp(0.0, f64::from(surface_height)) as u32;
        let clip_right = points
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .clamp(0.0, f64::from(surface_width)) as u32;
        let clip_bottom = points
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            .clamp(0.0, f64::from(surface_height)) as u32;
        left = left.max(clip_left);
        top = top.max(clip_top);
        right = right.min(clip_right);
        bottom = bottom.min(clip_bottom);
    }
    (left < right && top < bottom).then_some((left, top, right, bottom))
}

/// The clipped conservative device-space area of a frame clip set in canvas
/// pixels.
pub(super) fn frame_clip_coverage_area(
    clips: &[PngFrameClip],
    surface_width: u32,
    surface_height: u32,
) -> u64 {
    frame_clip_coverage_bounds(clips, surface_width, surface_height).map_or(
        0,
        |(left, top, right, bottom)| {
            u64::from(right - left).saturating_mul(u64::from(bottom - top))
        },
    )
}

/// The clipped conservative device-space area of a bound-label exclusion in
/// canvas pixels.
pub(super) fn label_mask_coverage_area(
    label: PngLabelMask,
    surface_width: u32,
    surface_height: u32,
) -> u64 {
    let left = label.x.floor().clamp(0.0, f64::from(surface_width)) as u32;
    let top = label.y.floor().clamp(0.0, f64::from(surface_height)) as u32;
    let right = (label.x + label.width)
        .ceil()
        .clamp(0.0, f64::from(surface_width)) as u32;
    let bottom = (label.y + label.height)
        .ceil()
        .clamp(0.0, f64::from(surface_height)) as u32;
    u64::from(right.saturating_sub(left)).saturating_mul(u64::from(bottom.saturating_sub(top)))
}

pub(super) fn conservative_coverage_area(
    drawing: &Drawing,
    transform: PngTransform,
    stroke_width: f64,
    surface_width: u32,
    surface_height: u32,
) -> u64 {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut include = |point| {
        let point = transform.map_point(point);
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    };
    for operation in drawing
        .fill
        .iter()
        .flat_map(|fill| &fill.path.operations)
        .chain(&drawing.outline.operations)
    {
        match operation {
            PathOperation::Move(point) => include(*point),
            PathOperation::Line(point) => include(*point),
            PathOperation::Cubic {
                control_1,
                control_2,
                end,
            } => {
                for point in [*control_1, *control_2, *end] {
                    include(point);
                }
            }
        }
    }
    for decoration in &drawing.decorations {
        for operation in decoration
            .drawing
            .fill
            .iter()
            .flat_map(|fill| &fill.path.operations)
            .chain(&decoration.drawing.outline.operations)
        {
            match operation {
                PathOperation::Move(point) | PathOperation::Line(point) => include(*point),
                PathOperation::Cubic {
                    control_1,
                    control_2,
                    end,
                } => {
                    for point in [*control_1, *control_2, *end] {
                        include(point);
                    }
                }
            }
        }
    }
    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return 0;
    }
    let outset = stroke_width * transform.scale / 2.0;
    let left = (min_x - outset)
        .floor()
        .clamp(0.0, f64::from(surface_width));
    let top = (min_y - outset)
        .floor()
        .clamp(0.0, f64::from(surface_height));
    let right = (max_x + outset).ceil().clamp(0.0, f64::from(surface_width));
    let bottom = (max_y + outset)
        .ceil()
        .clamp(0.0, f64::from(surface_height));
    ((right - left) as u64).saturating_mul((bottom - top) as u64)
}

pub(super) fn conservative_freedraw_coverage_area(
    path: &freedraw::ProjectedPath,
    background_fill: Option<&Fill>,
    transform: PngTransform,
    source_stroke_width: f64,
    surface_width: u32,
    surface_height: u32,
) -> u64 {
    let fill_bounds = background_fill.and_then(|fill| fill.path.cubic_bounds());
    let Some((min_x, min_y, max_x, max_y)) = union_bounds(path.bounds(), fill_bounds) else {
        return 0;
    };
    let points = [
        Point { x: min_x, y: min_y },
        Point { x: max_x, y: min_y },
        Point { x: max_x, y: max_y },
        Point { x: min_x, y: max_y },
    ]
    .map(|point| transform.map_point(point));
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let outset = background_fill
        .filter(|fill| matches!(fill.kind, FillKind::Sketch))
        .map_or(0.0, |_| source_stroke_width * transform.scale / 4.0);
    let left = (min_x - outset)
        .floor()
        .clamp(0.0, f64::from(surface_width));
    let top = (min_y - outset)
        .floor()
        .clamp(0.0, f64::from(surface_height));
    let right = (max_x + outset).ceil().clamp(0.0, f64::from(surface_width));
    let bottom = (max_y + outset)
        .ceil()
        .clamp(0.0, f64::from(surface_height));
    ((right - left) as u64).saturating_mul((bottom - top) as u64)
}

pub(super) fn conservative_text_coverage_area(
    text: &ResolvedText,
    clips: &[PngFrameClip],
    transform: PngTransform,
    surface_width: u32,
    surface_height: u32,
) -> u64 {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut include = |x: f64, y: f64| {
        let point = transform.map_point(Point { x, y });
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    };
    for operation in text.lines.iter().flat_map(|line| &line.path.operations) {
        match *operation {
            LabelPathOperation::Move { x, y } | LabelPathOperation::Line { x, y } => include(x, y),
            LabelPathOperation::Quadratic {
                control_x,
                control_y,
                x,
                y,
            } => {
                include(control_x, control_y);
                include(x, y);
            }
            LabelPathOperation::Close => {}
        }
    }
    if !min_x.is_finite() || !min_y.is_finite() || !max_x.is_finite() || !max_y.is_finite() {
        return 0;
    }
    for clip in clips {
        let Some((clip_min_x, clip_min_y, clip_max_x, clip_max_y)) = clip.path.cubic_bounds()
        else {
            return 0;
        };
        let corners = [
            Point {
                x: clip_min_x,
                y: clip_min_y,
            },
            Point {
                x: clip_max_x,
                y: clip_min_y,
            },
            Point {
                x: clip_max_x,
                y: clip_max_y,
            },
            Point {
                x: clip_min_x,
                y: clip_max_y,
            },
        ]
        .map(|point| clip.transform.map_point(point));
        let clip_left = corners
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min);
        let clip_top = corners
            .iter()
            .map(|point| point.y)
            .fold(f64::INFINITY, f64::min);
        let clip_right = corners
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let clip_bottom = corners
            .iter()
            .map(|point| point.y)
            .fold(f64::NEG_INFINITY, f64::max);
        min_x = min_x.max(clip_left);
        min_y = min_y.max(clip_top);
        max_x = max_x.min(clip_right);
        max_y = max_y.min(clip_bottom);
    }
    if min_x >= max_x || min_y >= max_y {
        return 0;
    }
    let left = min_x.floor().clamp(0.0, f64::from(surface_width));
    let top = min_y.floor().clamp(0.0, f64::from(surface_height));
    let right = max_x.ceil().clamp(0.0, f64::from(surface_width));
    let bottom = max_y.ceil().clamp(0.0, f64::from(surface_height));
    ((right - left) as u64).saturating_mul((bottom - top) as u64)
}

pub(super) fn conservative_fallback_coverage_area(
    width: f64,
    height: f64,
    transform: PngTransform,
    surface_width: u32,
    surface_height: u32,
) -> u64 {
    let (left, top, right, bottom) =
        fallback_coverage_bounds(width, height, transform, surface_width, surface_height);
    u64::from(right.saturating_sub(left)).saturating_mul(u64::from(bottom.saturating_sub(top)))
}

pub(super) fn fallback_coverage_bounds(
    width: f64,
    height: f64,
    transform: PngTransform,
    surface_width: u32,
    surface_height: u32,
) -> (u32, u32, u32, u32) {
    let points = [
        Point { x: 0.0, y: 0.0 },
        Point { x: width, y: 0.0 },
        Point {
            x: width,
            y: height,
        },
        Point { x: 0.0, y: height },
    ]
    .map(|point| transform.map_point(point));
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let left = min_x.floor().clamp(0.0, f64::from(surface_width));
    let top = min_y.floor().clamp(0.0, f64::from(surface_height));
    let right = max_x.ceil().clamp(0.0, f64::from(surface_width));
    let bottom = max_y.ceil().clamp(0.0, f64::from(surface_height));
    (left as u32, top as u32, right as u32, bottom as u32)
}

#[cfg(test)]
mod tests {
    use super::super::PngTransform;
    use super::conservative_fallback_coverage_area;
    use crate::render::resolve::ResolvedTransform;

    #[test]
    fn a_fallback_surface_never_exceeds_the_canvas_that_admitted_it() {
        // `fallback_coverage_bounds` clamps the placeholder's rotated
        // rectangle to `[0, width] x [0, height]`, so its surface is at most
        // the canvas's own, which `max_raster_surface_pixels` has already
        // admitted or rejected fatally before any placeholder is charged.
        // The former local crossing of that ceiling was therefore unreachable
        // (#159); this holds the clamp for the placements that would reach it
        // if it were not: an element larger than the canvas, one straddling
        // every edge, one rotated so its box outgrows the canvas, one placed
        // far outside it, and one at an export scale that magnifies it.
        let transform = |x: f64, y: f64, angle: f64, width: f64, height: f64, scale: f64| {
            PngTransform::new(
                &ResolvedTransform {
                    translation_x: x,
                    translation_y: y,
                    angle_radians: angle,
                    center_x: width / 2.0,
                    center_y: height / 2.0,
                },
                scale,
            )
        };
        let canvas = (120_u32, 80_u32);
        let canvas_area = u64::from(canvas.0) * u64::from(canvas.1);
        for (x, y, angle, width, height, scale) in [
            (-1_000.0, -1_000.0, 0.0, 5_000.0, 5_000.0, 1.0),
            (-10.0, -10.0, 0.0, 140.0, 100.0, 1.0),
            (0.0, 0.0, 0.7, 120.0, 80.0, 1.0),
            (10_000.0, 10_000.0, 0.0, 10.0, 10.0, 1.0),
            (0.0, 0.0, 0.0, 120.0, 80.0, 8.0),
            (0.0, 0.0, 0.0, 120.0, 80.0, 1.0),
            (f64::MAX / 4.0, 0.0, 0.0, 10.0, 10.0, 1.0),
        ] {
            let area = conservative_fallback_coverage_area(
                width,
                height,
                transform(x, y, angle, width, height, scale),
                canvas.0,
                canvas.1,
            );
            assert!(
                area <= canvas_area,
                "({x}, {y}, {angle}, {width}x{height}, x{scale}) covers {area} of {canvas_area}"
            );
        }
        assert_eq!(
            conservative_fallback_coverage_area(
                120.0,
                80.0,
                transform(0.0, 0.0, 0.0, 120.0, 80.0, 1.0),
                canvas.0,
                canvas.1
            ),
            canvas_area,
            "a placeholder filling the canvas costs exactly the canvas"
        );
    }
}
