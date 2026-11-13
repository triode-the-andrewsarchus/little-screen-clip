//! The scene's layout bounds: the pass over every outline, frame name and
//! painted placeholder that settles the layout origin every canvas placement
//! subtracts and the logical size of the artifact.

use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::portable_math::sin_cos;
use crate::relationships::ResolvedRelationshipGraph;
use crate::scene_surface::SurfaceFallback;

use super::super::frame_target::{
    relationship_frame_target, EffectiveFrameRendering, FrameViewport,
};
use super::super::geometry::{
    include_diamond_bounds, include_ellipse_bounds, include_transformed_bounds,
    linear_points_bounds, point_rotate_rads, reference_curve_bounds, source_layout_bounds,
    GeneratedGeometry, Point,
};
use super::super::text::{FrameName, FRAME_NAME_BOX_HEIGHT, FRAME_NAME_OFFSET_Y};

/// The scene's layout bounds as `(min_x, min_y, max_x, max_y)`: the frame
/// viewport of a frame export, or the rotated extents of every outline frame
/// clipping leaves visible, every frame name, and every painted fallback that
/// is not a clipped frame child. `None` when an element's normalized style is
/// outside the focused case this stage implements; infinite bounds mean the
/// scene has nothing to measure.
pub(super) fn scene_bounds(
    outlines: &[(NormalizedElement, Option<GeneratedGeometry>)],
    fallbacks: &[SurfaceFallback],
    frame_names: &[(usize, FrameName)],
    relationships: &ResolvedRelationshipGraph,
    frame_rendering: EffectiveFrameRendering,
    viewport: Option<FrameViewport>,
) -> Option<(f64, f64, f64, f64)> {
    let (mut min_x, mut min_y, mut max_x, mut max_y) = viewport.map_or(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |viewport| {
            (
                viewport.min_x,
                viewport.min_y,
                viewport.max_x,
                viewport.max_y,
            )
        },
    );
    for (source, geometry) in outlines {
        if source.stroke_width < 0.0
            || source.roughness < 0.0
            || !(0.0..=100.0).contains(&source.opacity)
            || source.seed == 0
        {
            return None;
        }
        let is_frame = matches!(
            source.kind,
            NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
        );
        if is_frame && !frame_rendering.enabled {
            continue;
        }
        if let Some((_, name)) = frame_names
            .iter()
            .find(|(index, _)| *index == source.source.source_index())
        {
            // The name is an unrotated text element at the frame's x, its
            // line box `nameOffsetY` above the frame's top edge. It belongs
            // to no frame, so it extends the bounds even when the frame it
            // names is itself a clipped child of another frame.
            include_transformed_bounds(
                source.x,
                source.y - FRAME_NAME_OFFSET_Y - FRAME_NAME_BOX_HEIGHT,
                name.width,
                FRAME_NAME_BOX_HEIGHT,
                0.0,
                &mut min_x,
                &mut min_y,
                &mut max_x,
                &mut max_y,
            );
        }
        if frame_rendering.enabled
            && frame_rendering.clip
            && relationship_frame_target(source.source.source_index(), relationships).is_some()
        {
            continue;
        }
        if viewport.is_some() {
            continue;
        }
        include_element_bounds(
            source,
            geometry.as_ref(),
            &mut min_x,
            &mut min_y,
            &mut max_x,
            &mut max_y,
        );
    }
    for fallback in fallbacks {
        if viewport.is_some() {
            continue;
        }
        let is_clipped_child = frame_rendering.enabled
            && frame_rendering.clip
            && fallback.element_id.as_deref().is_some_and(|id| {
                relationships
                    .nodes
                    .iter()
                    .find(|node| node.id == id)
                    .and_then(|node| relationship_frame_target(node.source_index, relationships))
                    .is_some()
            });
        if is_clipped_child {
            continue;
        }
        include_transformed_bounds(
            fallback.x,
            fallback.y,
            fallback.width,
            fallback.height,
            fallback.angle,
            &mut min_x,
            &mut min_y,
            &mut max_x,
            &mut max_y,
        );
    }
    Some((min_x, min_y, max_x, max_y))
}

/// Extends the bounds by one outline as the reference's `getElementBounds`
/// measures its kind (#172): a freedraw's rotated points, a linear element's
/// rotated curve (below), a diamond's rotated vertices, an ellipse's exact
/// extent, and the rotated box corners of everything else.
fn include_element_bounds(
    source: &NormalizedElement,
    geometry: Option<&GeneratedGeometry>,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    match &source.kind {
        NormalizedElementKind::Freedraw(freedraw) if !freedraw.points.is_empty() => {
            // `getFreeDrawElementAbsoluteCoords` centres the element on its
            // points' box, and `calculateBounds` rotates each point in local
            // coordinates about `(cx - x, cy - y)` before adding `x` and `y`.
            let (local_min_x, local_min_y, local_max_x, local_max_y) = source_layout_bounds(source);
            let (x1, y1, x2, y2) = (
                local_min_x + source.x,
                local_min_y + source.y,
                local_max_x + source.x,
                local_max_y + source.y,
            );
            let center = Point {
                x: (x1 + x2) / 2.0 - source.x,
                y: (y1 + y2) / 2.0 - source.y,
            };
            let sin_cos = sin_cos(source.angle);
            let (mut local_min_x, mut local_min_y, mut local_max_x, mut local_max_y) = (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            );
            for point in &freedraw.points {
                let rotated = point_rotate_rads(
                    Point {
                        x: point[0],
                        y: point[1],
                    },
                    center,
                    sin_cos,
                );
                local_min_x = local_min_x.min(rotated.x);
                local_min_y = local_min_y.min(rotated.y);
                local_max_x = local_max_x.max(rotated.x);
                local_max_y = local_max_y.max(rotated.y);
            }
            *min_x = min_x.min(local_min_x + source.x);
            *min_y = min_y.min(local_min_y + source.y);
            *max_x = max_x.max(local_max_x + source.x);
            *max_y = max_y.max(local_max_y + source.y);
        }
        NormalizedElementKind::Linear(linear) => {
            include_linear_bounds(source, linear, geometry, min_x, min_y, max_x, max_y);
        }
        NormalizedElementKind::Diamond => include_diamond_bounds(
            source.x,
            source.y,
            source.width,
            source.height,
            source.angle,
            min_x,
            min_y,
            max_x,
            max_y,
        ),
        NormalizedElementKind::Ellipse => include_ellipse_bounds(
            source.x,
            source.y,
            source.width,
            source.height,
            source.angle,
            min_x,
            min_y,
            max_x,
            max_y,
        ),
        _ => include_transformed_bounds(
            source.x,
            source.y,
            source.width,
            source.height,
            source.angle,
            min_x,
            min_y,
            max_x,
            max_y,
        ),
    }
}

pub(super) fn element_bounds(
    source: &NormalizedElement,
    geometry: Option<&GeneratedGeometry>,
) -> (f64, f64, f64, f64) {
    let (mut x1, mut y1, mut x2, mut y2) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    include_element_bounds(source, geometry, &mut x1, &mut y1, &mut x2, &mut y2);
    (x1, y1, x2, y2)
}

/// `getLinearElementRotatedBounds` as the export reaches it: the centre is
/// that of the element's points' box (`getElementAbsoluteCoords` reads the
/// points when the shape cache is cold, which it is while `getCanvasSize`
/// measures a freshly restored scene), and the extent is the analytic extent
/// of a Rough.js shape's cubics with every control point placed on the page
/// and rotated about that centre (#172, #173). The shape is
/// `ShapeCache.get(element)?.[0] ?? generateLinearElementShape(element)` on
/// that same cold cache: the drawn outline for a line or arrow, whose
/// `generateLinearElementShape` is the shape `renderElement` draws, and for
/// an elbow arrow the `linearPath` (or `curve`) over its route points that
/// the renderer generates for this measurement alone, jitter included, since
/// the drawn elbow path is not what the reference measures (#176). At
/// roughness 0 the elbow's shape has no jitter and its extrema are the route
/// points. A shape that draws no cubic, or an element with no geometry, falls
/// back to the rotated points.
fn include_linear_bounds(
    source: &NormalizedElement,
    linear: &crate::normalization::NormalizedLinear,
    geometry: Option<&GeneratedGeometry>,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    let (points_min_x, points_min_y, points_max_x, points_max_y) = linear_points_bounds(linear);
    let (x1, y1, x2, y2) = (
        points_min_x + source.x,
        points_min_y + source.y,
        points_max_x + source.x,
        points_max_y + source.y,
    );
    let center = Point {
        x: (x1 + x2) / 2.0,
        y: (y1 + y2) / 2.0,
    };
    let sin_cos = sin_cos(source.angle);
    let transform = |point: Point| {
        point_rotate_rads(
            Point {
                x: source.x + point.x,
                y: source.y + point.y,
            },
            center,
            sin_cos,
        )
    };
    let outline = match geometry {
        Some(GeneratedGeometry::Rough(rough)) => {
            let operations = match &rough.extent_outline {
                Some(extent_outline) if linear.elbowed => &extent_outline.operations,
                _ => &rough.drawing.outline.operations,
            };
            reference_curve_bounds(operations, transform)
        }
        _ => None,
    };
    match outline {
        Some((curve_min_x, curve_min_y, curve_max_x, curve_max_y)) => {
            *min_x = min_x.min(curve_min_x);
            *min_y = min_y.min(curve_min_y);
            *max_x = max_x.max(curve_max_x);
            *max_y = max_y.max(curve_max_y);
        }
        None => {
            for point in &linear.points {
                let rotated = transform(Point {
                    x: point[0],
                    y: point[1],
                });
                *min_x = min_x.min(rotated.x);
                *min_y = min_y.min(rotated.y);
                *max_x = max_x.max(rotated.x);
                *max_y = max_y.max(rotated.y);
            }
        }
    }
}
