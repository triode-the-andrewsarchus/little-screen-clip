//! Target-neutral geometry: the `Point`, `PathOperation` and `Path` primitives
//! every backend and the Rough.js port build on, the frame and image outline
//! shapes, element bounds, the arrowhead and elbow construction that feeds the
//! Rough generators, convex polygon clipping, and the ECMAScript rounding the
//! reference applies to coordinates. `rough` owns only the Rough.js 4.6.4
//! generators themselves.

use crate::color::CanonicalColor;
use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::portable_math::{
    atan2, cos, cube, cube_rounded_once, ecmascript_hypot, hypot, sin, sin_cos, square,
};
use crate::ColorMode;

use super::resolve::ResolveSceneError;
#[cfg(any(feature = "pdf", test))]
use super::FormatEncodeError;
use super::{freedraw, rough};

mod cubic;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Point {
    pub(super) x: f64,
    pub(super) y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PathOperation {
    Move(Point),
    Line(Point),
    Cubic {
        control_1: Point,
        control_2: Point,
        end: Point,
    },
}

#[derive(Debug, PartialEq)]
pub(super) struct Path {
    pub(super) operations: Vec<PathOperation>,
}

impl Path {
    pub(super) fn cubic_bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut current = None;
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for operation in &self.operations {
            match *operation {
                PathOperation::Move(point) | PathOperation::Line(point) => current = Some(point),
                PathOperation::Cubic {
                    control_1,
                    control_2,
                    end,
                } => {
                    let start = current?;
                    let (min_x, min_y, max_x, max_y) =
                        cubic::bounds(start, control_1, control_2, end);
                    bounds = Some(bounds.map_or((min_x, min_y, max_x, max_y), |current| {
                        (
                            current.0.min(min_x),
                            current.1.min(min_y),
                            current.2.max(max_x),
                            current.3.max(max_y),
                        )
                    }));
                    current = Some(end);
                }
            }
        }
        bounds
    }
}

pub(super) enum GeneratedGeometry {
    Rough(GeneratedRough),
    Freedraw(GeneratedFreedraw),
}

/// A Rough outline's generated geometry: the drawing every format paints and,
/// for an elbow arrow, the shape the reference measures its scene extent
/// with and never draws.
pub(super) struct GeneratedRough {
    pub(super) drawing: rough::Drawing,
    /// The Rough.js shape `getLinearElementRotatedBounds` generates over an
    /// elbow arrow's route points while the `ShapeCache` is cold, as it is
    /// when `getCanvasSize` measures a freshly restored scene:
    /// `generateLinearElementShape`, a `linearPath` over the points with the
    /// element's own `generateRoughOptions` (`preserveVertices` = raw
    /// `roughness < 2`, jitter included), or a `curve` over them when the
    /// element carries a roundness record. The drawn shape is the elbow path,
    /// so this is geometry the element generates for its extent alone; it is
    /// charged like every other generated command (#176). `None` for every
    /// other outline, whose drawn outline is what the reference measures.
    pub(super) extent_outline: Option<Path>,
}

pub(super) struct GeneratedFreedraw {
    pub(super) path: freedraw::Path,
    pub(super) background_fill: Option<rough::Fill>,
}

impl GeneratedGeometry {
    /// Every generated geometry command the element pays for: the drawing's
    /// outline, fill and decorations, a freedraw's path and background fill,
    /// and an elbow arrow's extent shape.
    pub(super) fn operation_count(&self) -> usize {
        match self {
            Self::Rough(rough) => {
                rough.drawing.operation_count()
                    + rough
                        .extent_outline
                        .as_ref()
                        .map_or(0, |outline| outline.operations.len())
            }
            Self::Freedraw(freedraw) => {
                freedraw.path.operation_count()
                    + freedraw
                        .background_fill
                        .as_ref()
                        .map_or(0, |fill| fill.path.operations.len())
            }
        }
    }

    /// The local box the canvas rotates the element about: the drawn Rough
    /// outline's cubic extent for a linear element (the box
    /// `getElementAbsoluteCoords` reads once `renderElement` has warmed the
    /// `ShapeCache`), the points' box of a freedraw, the element box otherwise.
    pub(super) fn canvas_bounds(&self, source: &NormalizedElement) -> (f64, f64, f64, f64) {
        match self {
            Self::Rough(rough) => local_outline_bounds(source, Some(&rough.drawing)),
            Self::Freedraw(_) => source_layout_bounds(source),
        }
    }
}

pub(super) fn source_layout_bounds(source: &NormalizedElement) -> (f64, f64, f64, f64) {
    match &source.kind {
        NormalizedElementKind::Freedraw(freedraw) if !freedraw.points.is_empty() => {
            freedraw.points.iter().fold(
                (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ),
                |(min_x, min_y, max_x, max_y), point| {
                    (
                        min_x.min(point[0]),
                        min_y.min(point[1]),
                        max_x.max(point[0]),
                        max_y.max(point[1]),
                    )
                },
            )
        }
        _ => (0.0, 0.0, source.width, source.height),
    }
}

/// `FRAME_STYLE.radius`: the corner radius of a frame's outline and of the clip
/// it applies to its children in a scene export.
pub(super) const FRAME_RADIUS: f64 = 8.0;

/// `FRAME_STYLE.strokeColor`: the stroke of a frame's outline in a scene export.
/// The reference's SVG paints every frame kind with it under its root dark-mode
/// filter; its canvas path recolors a magic frame's outline by theme
/// ([`magic_frame_canvas_outline_stroke_color`]) before the same filter.
pub(super) const FRAME_OUTLINE_STROKE_COLOR: CanonicalColor =
    CanonicalColor::from_rgb8(0xbb, 0xbb, 0xbb);

/// The stroke the reference's canvas path (`renderElement`) gives a magic
/// frame's outline: `#7affd7` in the light theme and `#1d8264` in the dark
/// theme, each then passing through the theme's filter like every other canvas
/// color. The PNG backend reproduces it; the reference's SVG does not recolor
/// magic frames.
pub(super) fn magic_frame_canvas_outline_stroke_color(mode: ColorMode) -> CanonicalColor {
    match mode {
        ColorMode::Light => CanonicalColor::from_rgb8(0x7a, 0xff, 0xd7),
        ColorMode::Dark => CanonicalColor::from_rgb8(0x1d, 0x82, 0x64),
    }
    .for_mode(mode)
}

/// `FRAME_STYLE.strokeWidth`: the width of a frame's outline stroke.
pub(super) const FRAME_OUTLINE_STROKE_WIDTH: f64 = 2.0;

pub(super) fn resolved_frame_outline(width: f64, height: f64) -> Result<Path, ResolveSceneError> {
    let radius = FRAME_RADIUS.min(width / 2.0).min(height / 2.0).max(0.0);
    rounded_rect_path(width, height, radius)
}

/// The exact rounded rectangle `(0, 0)`-`(width, height)` with `radius`
/// corners, as an SVG `<rect rx ry>` of that size draws it: four edges joined
/// by four kappa-approximated quarter arcs, starting and ending where the top
/// edge meets its left arc. Callers bound `radius` to half the shorter side.
pub(super) fn rounded_rect_path(
    width: f64,
    height: f64,
    radius: f64,
) -> Result<Path, ResolveSceneError> {
    let kappa = 0.552_284_749_830_793_6;
    let point = |x, y| Point { x, y };
    let mut operations = Vec::new();
    operations.try_reserve_exact(10)?;
    operations.push(PathOperation::Move(point(radius, 0.0)));
    operations.push(PathOperation::Line(point(width - radius, 0.0)));
    operations.push(PathOperation::Cubic {
        control_1: point(width - radius + radius * kappa, 0.0),
        control_2: point(width, radius - radius * kappa),
        end: point(width, radius),
    });
    operations.push(PathOperation::Line(point(width, height - radius)));
    operations.push(PathOperation::Cubic {
        control_1: point(width, height - radius + radius * kappa),
        control_2: point(width - radius + radius * kappa, height),
        end: point(width - radius, height),
    });
    operations.push(PathOperation::Line(point(radius, height)));
    operations.push(PathOperation::Cubic {
        control_1: point(radius - radius * kappa, height),
        control_2: point(0.0, height - radius + radius * kappa),
        end: point(0.0, height - radius),
    });
    operations.push(PathOperation::Line(point(0.0, radius)));
    operations.push(PathOperation::Cubic {
        control_1: point(0.0, radius - radius * kappa),
        control_2: point(radius - radius * kappa, 0.0),
        end: point(radius, 0.0),
    });
    Ok(Path { operations })
}

pub(super) fn resolved_frame_rect(width: f64, height: f64) -> Result<Path, ResolveSceneError> {
    let point = |x, y| Point { x, y };
    let mut operations = Vec::new();
    operations.try_reserve_exact(5)?;
    operations.push(PathOperation::Move(point(0.0, 0.0)));
    operations.push(PathOperation::Line(point(width, 0.0)));
    operations.push(PathOperation::Line(point(width, height)));
    operations.push(PathOperation::Line(point(0.0, height)));
    operations.push(PathOperation::Line(point(0.0, 0.0)));
    Ok(Path { operations })
}

/// `pointRotateRads`: `point` rotated by the angle whose `(sin, cos)` is given
/// about `center`, in the reference's own association —
/// `(x - cx) * cos - (y - cy) * sin + cx` — since the last ulp of a scene
/// extent is what the SVG `viewBox` prints (#172).
pub(super) fn point_rotate_rads(point: Point, center: Point, (sin, cos): (f64, f64)) -> Point {
    Point {
        x: (point.x - center.x) * cos - (point.y - center.y) * sin + center.x,
        y: (point.x - center.x) * sin + (point.y - center.y) * cos + center.y,
    }
}

/// The scene bounds `getElementBounds` gives a rectangle, embeddable, iframe,
/// image, text or frame: the four corners of `(x, y)`-`(x + width, y + height)`
/// each rotated about `(x + width / 2, y + height / 2)`.
#[allow(clippy::too_many_arguments)]
pub(super) fn include_transformed_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    let center = Point {
        x: x + width / 2.0,
        y: y + height / 2.0,
    };
    let sin_cos = sin_cos(angle);
    let (x1, y1, x2, y2) = (x, y, x + width, y + height);
    for corner in [
        Point { x: x1, y: y1 },
        Point { x: x1, y: y2 },
        Point { x: x2, y: y2 },
        Point { x: x2, y: y1 },
    ] {
        include_point(
            point_rotate_rads(corner, center, sin_cos),
            min_x,
            min_y,
            max_x,
            max_y,
        );
    }
}

fn include_point(point: Point, min_x: &mut f64, min_y: &mut f64, max_x: &mut f64, max_y: &mut f64) {
    *min_x = min_x.min(point.x);
    *min_y = min_y.min(point.y);
    *max_x = max_x.max(point.x);
    *max_y = max_y.max(point.y);
}

/// The scene bounds `getElementBounds` gives an ellipse: not its rotated box
/// but its exact extent, `hypot(w cos, h sin)` by `hypot(h cos, w sin)` about
/// the centre for the half extents `w` and `h`, with `Math.hypot` as V8
/// evaluates it (#172).
#[allow(clippy::too_many_arguments)]
pub(super) fn include_ellipse_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    // `getElementAbsoluteCoords`: `x2 - x1` is `(x + width) - x`, not `width`.
    let (x1, y1, x2, y2) = (x, y, x + width, y + height);
    let center_x = x + width / 2.0;
    let center_y = y + height / 2.0;
    let w = (x2 - x1) / 2.0;
    let h = (y2 - y1) / 2.0;
    let (sin, cos) = sin_cos(angle);
    let ww = ecmascript_hypot(w * cos, h * sin);
    let hh = ecmascript_hypot(h * cos, w * sin);
    include_point(
        Point {
            x: center_x - ww,
            y: center_y - hh,
        },
        min_x,
        min_y,
        max_x,
        max_y,
    );
    include_point(
        Point {
            x: center_x + ww,
            y: center_y + hh,
        },
        min_x,
        min_y,
        max_x,
        max_y,
    );
}

/// The scene bounds `getElementBounds` gives a diamond: its four vertices at
/// the exact midpoints of the box's sides, `(cx, y1)`, `(cx, y2)`, `(x1, cy)`
/// and `(x2, cy)`, each rotated about the centre (#172). The drawn vertices
/// sit at `floor(extent / 2) + 1`; the bounds do not.
#[allow(clippy::too_many_arguments)]
pub(super) fn include_diamond_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    angle: f64,
    min_x: &mut f64,
    min_y: &mut f64,
    max_x: &mut f64,
    max_y: &mut f64,
) {
    let (x1, y1, x2, y2) = (x, y, x + width, y + height);
    let center = Point {
        x: x + width / 2.0,
        y: y + height / 2.0,
    };
    let sin_cos = sin_cos(angle);
    for vertex in [
        Point { x: center.x, y: y1 },
        Point { x: center.x, y: y2 },
        Point { x: x1, y: center.y },
        Point { x: x2, y: center.y },
    ] {
        include_point(
            point_rotate_rads(vertex, center, sin_cos),
            min_x,
            min_y,
            max_x,
            max_y,
        );
    }
}

/// `getBezierValueForT`: the cubic through `p0`, `p1`, `p2`, `p3` at `t`, in
/// the reference's term order with `Math.pow(x, 3)` rounded once (#173).
fn bezier_value_for_t(t: f64, p0: f64, p1: f64, p2: f64, p3: f64) -> f64 {
    let one_minus_t = 1.0 - t;
    cube_rounded_once(one_minus_t) * p0
        + 3.0 * square(one_minus_t) * t * p1
        + 3.0 * one_minus_t * square(t) * p2
        + cube_rounded_once(t) * p3
}

/// `solveQuadratic`: the cubic's values at the roots of its derivative that
/// lie in `[0, 1]`, or `None` when the discriminant is negative, with the
/// reference's coefficient spelling (`3i - 6j + 3k`, `6j - 6i`, `3i` for the
/// three control differences), its exact `a === 0` test and its formula for
/// each root, since a differently spelled solver lands an ulp away (#173).
fn solve_quadratic(p0: f64, p1: f64, p2: f64, p3: f64) -> Option<[Option<f64>; 2]> {
    let i = p1 - p0;
    let j = p2 - p1;
    let k = p3 - p2;
    let a = 3.0 * i - 6.0 * j + 3.0 * k;
    let b = 6.0 * j - 6.0 * i;
    let c = 3.0 * i;
    let sqrt_part = b * b - 4.0 * a * c;
    // `hasSolution = sqrtPart >= 0`: a NaN discriminant has none either.
    let has_solution = sqrt_part >= 0.0;
    if !has_solution {
        return None;
    }
    let (t1, t2) = if a == 0.0 {
        (-c / b, -c / b)
    } else {
        (
            (-b + sqrt_part.sqrt()) / (2.0 * a),
            (-b - sqrt_part.sqrt()) / (2.0 * a),
        )
    };
    let retain = |t: f64| {
        (0.0..=1.0)
            .contains(&t)
            .then(|| bezier_value_for_t(t, p0, p1, p2, p3))
    };
    Some([retain(t1), retain(t2)])
}

/// `getCubicBezierCurveBound`: the exact extent of one cubic — its end
/// points and the retained interior extrema of each axis (#173).
fn cubic_bezier_curve_bound(p0: Point, p1: Point, p2: Point, p3: Point) -> (f64, f64, f64, f64) {
    let axis = |p0: f64, p1: f64, p2: f64, p3: f64| {
        let mut min = p0.min(p3);
        let mut max = p0.max(p3);
        if let Some(solutions) = solve_quadratic(p0, p1, p2, p3) {
            for value in solutions.into_iter().flatten() {
                min = min.min(value);
                max = max.max(value);
            }
        }
        (min, max)
    };
    let (min_x, max_x) = axis(p0.x, p1.x, p2.x, p3.x);
    let (min_y, max_y) = axis(p0.y, p1.y, p2.y, p3.y);
    (min_x, min_y, max_x, max_y)
}

/// `getMinMaxXYFromCurvePathOps(ops, transformXY)`: the extent of a Rough.js
/// path's cubics, each control point passed through `transform` first — the
/// scene extent of a linear element, whose control points the reference
/// places on the page and rotates about the element's centre before taking
/// their analytic extrema (#172, #173). A `move` sets the current point
/// untransformed; a `lineTo` is not implemented by the reference and changes
/// nothing. `None` when the path draws no cubic.
pub(super) fn reference_curve_bounds(
    operations: &[PathOperation],
    transform: impl Fn(Point) -> Point,
) -> Option<(f64, f64, f64, f64)> {
    let mut current = Point { x: 0.0, y: 0.0 };
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for operation in operations {
        match *operation {
            PathOperation::Move(point) => current = point,
            PathOperation::Line(_) => {}
            PathOperation::Cubic {
                control_1,
                control_2,
                end,
            } => {
                let p1 = transform(control_1);
                let p2 = transform(control_2);
                let p3 = transform(end);
                let p0 = transform(current);
                current = end;
                let segment = cubic_bezier_curve_bound(p0, p1, p2, p3);
                bounds = union_bounds(bounds, Some(segment));
            }
        }
    }
    bounds
}

/// The roughness Rough.js is given for `source`: Excalidraw's
/// `generateRoughOptions` tones small shapes down, halving the roughness for
/// a shape under 50 by 20 (a third under 10) and capping it at 2.5, and
/// exempts shapes rounded enough or, for linear elements, long enough for the
/// sketchiness to read.
pub(super) fn adjusted_roughness(source: &NormalizedElement) -> f64 {
    let max_size = source.width.max(source.height);
    let min_size = source.width.min(source.height);
    if min_size >= 20.0 && max_size >= 50.0
        || min_size >= 15.0 && source.roundness.is_some()
        || matches!(source.kind, NormalizedElementKind::Linear(_)) && max_size >= 50.0
    {
        source.roughness
    } else {
        (source.roughness / if max_size < 10.0 { 3.0 } else { 2.0 }).min(2.5)
    }
}

/// A Rough outline's complete generated geometry: the drawing, then, for an
/// elbow arrow, the shape the reference measures its extent with (#176). Both
/// draw on one budget, so the element's segment, work and working-byte
/// charges and the point at which it crosses a local limit count the extent
/// shape's commands like the drawn shape's.
pub(super) fn generate_rough(
    source: &NormalizedElement,
    roughness: f64,
    budget: &mut rough::GenerationBudget,
) -> Result<GeneratedRough, rough::GenerationError> {
    let drawing = rough_drawing(source, roughness, budget)?;
    let extent_outline = match &source.kind {
        NormalizedElementKind::Linear(linear) if linear.elbowed => {
            Some(elbow_extent_outline(source, linear, roughness, budget)?)
        }
        _ => None,
    };
    Ok(GeneratedRough {
        drawing,
        extent_outline,
    })
}

/// `generateLinearElementShape(element)` for an elbow arrow, the shape
/// `getLinearElementRotatedBounds` falls back to while the `ShapeCache` is
/// cold: `generator.linearPath(element.points, generateRoughOptions(element))`
/// — or `generator.curve` when the element carries a `roundness` record, as
/// the reference checks that first — with the element's own options rather
/// than the elbow path's `continuousPath` ones, so `preserveVertices` is the
/// raw `roughness < 2` and the control points jitter (#176). An arrow's
/// options never carry a fill, so the `polygon` branch is unreachable.
fn elbow_extent_outline(
    source: &NormalizedElement,
    linear: &crate::normalization::NormalizedLinear,
    roughness: f64,
    budget: &mut rough::GenerationBudget,
) -> Result<Path, rough::GenerationError> {
    let mut points = Vec::new();
    budget.work(u64::try_from(linear.points.len()).unwrap_or(u64::MAX))?;
    points.try_reserve_exact(linear.points.len())?;
    points.extend(linear.points.iter().map(|point| Point {
        x: point[0],
        y: point[1],
    }));
    let preserve_vertices = source.roughness < 2.0;
    let disable_multi_stroke = source.stroke_style != "solid";
    let drawing = if source.roundness.is_some() {
        rough::curve_drawing(
            &points,
            false,
            source.seed,
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            None,
            source.stroke_width,
            budget,
        )?
    } else {
        rough::linear_drawing(
            &points,
            false,
            source.seed,
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            None,
            source.stroke_width,
            budget,
        )?
    };
    Ok(drawing.outline)
}

pub(super) fn rough_drawing(
    source: &NormalizedElement,
    roughness: f64,
    budget: &mut rough::GenerationBudget,
) -> Result<rough::Drawing, rough::GenerationError> {
    use rough::RoundedSegment;

    let width = source.width;
    let height = source.height;
    // The reference's `preserveVertices` is `continuousPath || roughness < 2`
    // on the element's raw roughness; `continuousPath` is set only for the
    // `generator.path` shapes, the rounded rectangle and diamond, which is
    // what `roundness.is_some()` stands for here. A rounded line or arrow is a
    // `generator.curve` with no `continuousPath`, so its fill takes the raw
    // roughness test alone (#158).
    let preserve_vertices = source.roundness.is_some() || source.roughness < 2.0;
    let curve_preserves_vertices = source.roughness < 2.0;
    let fill_style = (source.background_color != "transparent")
        .then(|| rough::FillStyle::from_normalized(&source.fill_style));
    let disable_multi_stroke = source.stroke_style != "solid";
    match &source.kind {
        NormalizedElementKind::Rectangle | NormalizedElementKind::Embeddable { .. } => {
            let Some(roundness) = source.roundness else {
                return rough::rectangle_drawing(
                    width,
                    height,
                    source.seed,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    fill_style,
                    source.stroke_width,
                    budget,
                );
            };
            let radius = corner_radius(width.min(height), roundness);
            let start = Point { x: radius, y: 0.0 };
            let segments = [
                RoundedSegment::Line(Point {
                    x: width - radius,
                    y: 0.0,
                }),
                RoundedSegment::Quadratic {
                    control: Point { x: width, y: 0.0 },
                    end: Point {
                        x: width,
                        y: radius,
                    },
                },
                RoundedSegment::Line(Point {
                    x: width,
                    y: height - radius,
                }),
                RoundedSegment::Quadratic {
                    control: Point {
                        x: width,
                        y: height,
                    },
                    end: Point {
                        x: width - radius,
                        y: height,
                    },
                },
                RoundedSegment::Line(Point {
                    x: radius,
                    y: height,
                }),
                RoundedSegment::Quadratic {
                    control: Point { x: 0.0, y: height },
                    end: Point {
                        x: 0.0,
                        y: height - radius,
                    },
                },
                RoundedSegment::Line(Point { x: 0.0, y: radius }),
                RoundedSegment::Quadratic {
                    control: Point { x: 0.0, y: 0.0 },
                    end: Point { x: radius, y: 0.0 },
                },
            ];
            let fill_points = rough::rounded_fill_points(start, &segments, roughness, budget)?;
            rough::rounded_drawing(
                start,
                &segments,
                &fill_points,
                source.seed,
                roughness,
                preserve_vertices,
                disable_multi_stroke,
                fill_style,
                source.stroke_width,
                budget,
            )
        }
        NormalizedElementKind::Diamond => {
            // The official export places the diamond's vertices at
            // `floor(extent / 2) + 1`, not at the exact midpoint, so an
            // even-sized diamond sits one pixel right of and below centre.
            // Using the true midpoint moved every diamond outline.
            let mid_x = (width / 2.0).floor() + 1.0;
            let mid_y = (height / 2.0).floor() + 1.0;
            let points = [
                Point { x: mid_x, y: 0.0 },
                Point { x: width, y: mid_y },
                Point {
                    x: mid_x,
                    y: height,
                },
                Point { x: 0.0, y: mid_y },
            ];
            let Some(roundness) = source.roundness else {
                return rough::polygon_drawing(
                    &points,
                    source.seed,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    fill_style,
                    source.stroke_width,
                    budget,
                );
            };
            // The radii come from the diamond's own vertex offsets, as the
            // official export's `getCornerRadius(Math.abs(topX - leftX))` and
            // `getCornerRadius(Math.abs(rightY - topY))` do, not from the
            // exact half extents: for a 120x80 diamond they are 15.25 / 10.25
            // (from 61 and 41), not 15 / 10 (#160).
            let vertical_radius = corner_radius(mid_x, roundness);
            let horizontal_radius = corner_radius(mid_y, roundness);
            let [top, right, bottom, left] = points;
            let start = Point {
                x: top.x + vertical_radius,
                y: top.y + horizontal_radius,
            };
            let segments = [
                RoundedSegment::Line(Point {
                    x: right.x - vertical_radius,
                    y: right.y - horizontal_radius,
                }),
                RoundedSegment::Cubic {
                    control_1: right,
                    control_2: right,
                    end: Point {
                        x: right.x - vertical_radius,
                        y: right.y + horizontal_radius,
                    },
                },
                RoundedSegment::Line(Point {
                    x: bottom.x + vertical_radius,
                    y: bottom.y - horizontal_radius,
                }),
                RoundedSegment::Cubic {
                    control_1: bottom,
                    control_2: bottom,
                    end: Point {
                        x: bottom.x - vertical_radius,
                        y: bottom.y - horizontal_radius,
                    },
                },
                RoundedSegment::Line(Point {
                    x: left.x + vertical_radius,
                    y: left.y + horizontal_radius,
                }),
                RoundedSegment::Cubic {
                    control_1: left,
                    control_2: left,
                    end: Point {
                        x: left.x + vertical_radius,
                        y: left.y - horizontal_radius,
                    },
                },
                RoundedSegment::Line(Point {
                    x: top.x - vertical_radius,
                    y: top.y + horizontal_radius,
                }),
                RoundedSegment::Cubic {
                    control_1: top,
                    control_2: top,
                    end: Point {
                        x: top.x + vertical_radius,
                        y: top.y + horizontal_radius,
                    },
                },
            ];
            let fill_points = rough::rounded_fill_points(start, &segments, roughness, budget)?;
            rough::rounded_drawing(
                start,
                &segments,
                &fill_points,
                source.seed,
                roughness,
                preserve_vertices,
                disable_multi_stroke,
                fill_style,
                source.stroke_width,
                budget,
            )
        }
        NormalizedElementKind::Ellipse => rough::ellipse_drawing(
            width,
            height,
            source.seed,
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            fill_style,
            source.stroke_width,
            budget,
        ),
        NormalizedElementKind::Linear(linear) => {
            let mut points = Vec::new();
            budget.work(u64::try_from(linear.points.len()).unwrap_or(u64::MAX))?;
            points.try_reserve_exact(linear.points.len())?;
            points.extend(linear.points.iter().map(|point| Point {
                x: point[0],
                y: point[1],
            }));
            const LINE_CONFIRM_THRESHOLD: f64 = 8.0;
            let closed = points.len() >= 3
                && points
                    .first()
                    .zip(points.last())
                    .is_some_and(|(first, last)| {
                        hypot(first.x - last.x, first.y - last.y) <= LINE_CONFIRM_THRESHOLD
                    });
            let mut drawing = if linear.elbowed {
                elbow_drawing(
                    &points,
                    source.seed,
                    roughness,
                    disable_multi_stroke,
                    budget,
                )?
            } else if source.roundness.is_some() {
                rough::curve_drawing(
                    &points,
                    closed,
                    source.seed,
                    roughness,
                    curve_preserves_vertices,
                    disable_multi_stroke,
                    closed.then_some(fill_style).flatten(),
                    source.stroke_width,
                    budget,
                )?
            } else {
                if closed && fill_style.is_some() {
                    rough::polygon_drawing(
                        &points,
                        source.seed,
                        roughness,
                        preserve_vertices,
                        disable_multi_stroke,
                        fill_style,
                        source.stroke_width,
                        budget,
                    )?
                } else {
                    rough::linear_drawing(
                        &points,
                        closed,
                        source.seed,
                        roughness,
                        preserve_vertices,
                        disable_multi_stroke,
                        None,
                        source.stroke_width,
                        budget,
                    )?
                }
            };
            if matches!(
                linear.kind,
                crate::normalization::NormalizedLinearKind::Arrow
            ) {
                add_arrowheads(&mut drawing, linear, source, roughness, budget)?;
            }
            Ok(drawing)
        }
        NormalizedElementKind::Freedraw(_)
        | NormalizedElementKind::Text(_)
        | NormalizedElementKind::Image(_)
        | NormalizedElementKind::Frame(_)
        | NormalizedElementKind::MagicFrame(_) => {
            unreachable!("only outlines reach Rough geometry")
        }
    }
}

fn elbow_drawing(
    points: &[Point],
    seed: i32,
    roughness: f64,
    disable_multi_stroke: bool,
    budget: &mut rough::GenerationBudget,
) -> Result<rough::Drawing, rough::GenerationError> {
    use rough::RoundedSegment;

    let start = points[0];
    let mut segments = Vec::new();
    segments.try_reserve_exact(points.len().saturating_mul(2))?;
    for index in 1..points.len().saturating_sub(1) {
        let previous = points[index - 1];
        let corner = points[index];
        let next = points[index + 1];
        let radius = 16.0_f64
            .min(hypot(corner.x - previous.x, corner.y - previous.y) / 2.0)
            .min(hypot(next.x - corner.x, next.y - corner.y) / 2.0);
        let previous_is_horizontal = (previous.x - corner.x).abs() >= (previous.y - corner.y).abs();
        let next_is_horizontal = (next.x - corner.x).abs() >= (next.y - corner.y).abs();
        let approach = if previous_is_horizontal {
            Point {
                x: corner.x
                    + if previous.x < corner.x {
                        -radius
                    } else {
                        radius
                    },
                y: corner.y,
            }
        } else {
            Point {
                x: corner.x,
                y: corner.y
                    + if previous.y < corner.y {
                        -radius
                    } else {
                        radius
                    },
            }
        };
        let departure = if next_is_horizontal {
            Point {
                x: corner.x + if next.x < corner.x { -radius } else { radius },
                y: corner.y,
            }
        } else {
            Point {
                x: corner.x,
                y: corner.y + if next.y < corner.y { -radius } else { radius },
            }
        };
        segments.push(RoundedSegment::Line(approach));
        segments.push(RoundedSegment::Quadratic {
            control: corner,
            end: departure,
        });
    }
    segments.push(RoundedSegment::Line(
        *points.last().unwrap_or(&Point { x: 0.0, y: 0.0 }),
    ));
    rough::rounded_drawing(
        start,
        &segments,
        points,
        seed,
        roughness,
        true,
        disable_multi_stroke,
        None,
        1.0,
        budget,
    )
}

/// The two members of the Rough.js options object that the reference's
/// `getArrowheadShapes` reads and, for some head kinds, mutates. The outline
/// and both heads share one object: the start head is generated first, and a
/// head that assigns to the object changes what the end head is generated
/// with (#171).
#[derive(Clone, Copy, Debug, PartialEq)]
struct SharedArrowheadOptions {
    roughness: f64,
    dash: rough::DecorationDash,
}

fn add_arrowheads(
    drawing: &mut rough::Drawing,
    linear: &crate::normalization::NormalizedLinear,
    source: &NormalizedElement,
    roughness: f64,
    budget: &mut rough::GenerationBudget,
) -> Result<(), rough::GenerationError> {
    // `generateRoughOptions`: the element's adjusted roughness and its own
    // `strokeLineDash`, which is what the outline was generated with.
    let mut options = SharedArrowheadOptions {
        roughness,
        dash: match source.stroke_style.as_str() {
            "dashed" => rough::DecorationDash::Dashed,
            "dotted" => rough::DecorationDash::Dotted,
            _ => rough::DecorationDash::Solid,
        },
    };
    for (start, arrowhead) in [
        (true, linear.start_arrowhead.as_deref()),
        (false, linear.end_arrowhead.as_deref()),
    ] {
        let Some(arrowhead) = arrowhead else { continue };
        let Some(points) = arrowhead_points(drawing, linear, start, arrowhead, source.stroke_width)
        else {
            continue;
        };
        let preserve_vertices = source.roughness < 2.0;
        match arrowhead {
            "dot" | "circle" | "circle_outline" => {
                // `{ ...options, roughness: Math.min(0.5, options.roughness) }`
                // after `delete options.strokeLineDash`: the clamp is the
                // head's own, the deletion is shared.
                options.dash = rough::DecorationDash::Solid;
                let head_roughness = options.roughness.min(0.5);
                let [center_x, center_y, diameter, ..] = points;
                let mut head = rough::circle_drawing(
                    diameter,
                    source.seed,
                    head_roughness,
                    preserve_vertices,
                    source.stroke_style != "solid",
                    Some(rough::FillStyle::Solid),
                    source.stroke_width,
                    budget,
                )?;
                translate_drawing(
                    &mut head,
                    center_x - diameter / 2.0,
                    center_y - diameter / 2.0,
                );
                drawing.decorations.push(rough::Decoration {
                    drawing: head,
                    fill_paint: if arrowhead == "circle_outline" {
                        rough::DecorationFillPaint::Background
                    } else {
                        rough::DecorationFillPaint::Stroke
                    },
                    dash: options.dash,
                });
            }
            "triangle" | "triangle_outline" => {
                // `{ ...options, roughness: Math.min(1, options.roughness) }`
                // after `delete options.strokeLineDash`.
                options.dash = rough::DecorationDash::Solid;
                let head_roughness = options.roughness.min(1.0);
                let [tip_x, tip_y, left_x, left_y, right_x, right_y, ..] = points;
                let head_points = [
                    Point { x: tip_x, y: tip_y },
                    Point {
                        x: left_x,
                        y: left_y,
                    },
                    Point {
                        x: right_x,
                        y: right_y,
                    },
                    Point { x: tip_x, y: tip_y },
                ];
                let head = rough::polygon_drawing(
                    &head_points,
                    source.seed,
                    head_roughness,
                    preserve_vertices,
                    source.stroke_style != "solid",
                    Some(rough::FillStyle::Solid),
                    source.stroke_width,
                    budget,
                )?;
                drawing.decorations.push(rough::Decoration {
                    drawing: head,
                    fill_paint: if arrowhead == "triangle_outline" {
                        rough::DecorationFillPaint::Background
                    } else {
                        rough::DecorationFillPaint::Stroke
                    },
                    dash: options.dash,
                });
            }
            "diamond" | "diamond_outline" => {
                // `{ ...options, roughness: Math.min(1, options.roughness) }`
                // after `delete options.strokeLineDash`.
                options.dash = rough::DecorationDash::Solid;
                let head_roughness = options.roughness.min(1.0);
                let head_points = [
                    Point {
                        x: points[0],
                        y: points[1],
                    },
                    Point {
                        x: points[2],
                        y: points[3],
                    },
                    Point {
                        x: points[4],
                        y: points[5],
                    },
                    Point {
                        x: points[6],
                        y: points[7],
                    },
                    Point {
                        x: points[0],
                        y: points[1],
                    },
                ];
                let head = rough::polygon_drawing(
                    &head_points,
                    source.seed,
                    head_roughness,
                    preserve_vertices,
                    source.stroke_style != "solid",
                    Some(rough::FillStyle::Solid),
                    source.stroke_width,
                    budget,
                )?;
                drawing.decorations.push(rough::Decoration {
                    drawing: head,
                    fill_paint: if arrowhead == "diamond_outline" {
                        rough::DecorationFillPaint::Background
                    } else {
                        rough::DecorationFillPaint::Stroke
                    },
                    dash: options.dash,
                });
            }
            "crowfoot_one" => {
                // `generator.line(x3, y3, x4, y4, options)`: the shared
                // options as they stand, roughness unclamped and the
                // element's dash pattern kept unless an earlier head
                // deleted or replaced it.
                add_open_arrowhead(drawing, source, arrowhead, points, options, budget)?;
            }
            _ => {
                // `bar`, `arrow`, `crowfoot_many`, `crowfoot_one_or_many`:
                // the default branch assigns `options.strokeLineDash` (the
                // shortened dotted pattern, or deleted) and
                // `options.roughness = Math.min(1, options.roughness)` on
                // the shared object, so the head that follows inherits both.
                options.dash = if source.stroke_style == "dotted" {
                    rough::DecorationDash::DottedHead
                } else {
                    rough::DecorationDash::Solid
                };
                options.roughness = options.roughness.min(1.0);
                let crossbar = (arrowhead == "crowfoot_one_or_many")
                    .then(|| {
                        arrowhead_points(
                            drawing,
                            linear,
                            start,
                            "crowfoot_one",
                            source.stroke_width,
                        )
                    })
                    .flatten();
                add_open_arrowhead(drawing, source, arrowhead, points, options, budget)?;
                if let Some(crossbar) = crossbar {
                    add_open_arrowhead(drawing, source, "crowfoot_one", crossbar, options, budget)?;
                }
            }
        }
    }
    Ok(())
}

fn add_open_arrowhead(
    drawing: &mut rough::Drawing,
    source: &NormalizedElement,
    arrowhead: &str,
    points: [f64; 8],
    options: SharedArrowheadOptions,
    budget: &mut rough::GenerationBudget,
) -> Result<(), rough::GenerationError> {
    let tip = Point {
        x: points[0],
        y: points[1],
    };
    let first = Point {
        x: points[2],
        y: points[3],
    };
    let second = Point {
        x: points[4],
        y: points[5],
    };
    let mut push_line = |start, end| -> Result<(), rough::GenerationError> {
        let head = rough::linear_drawing(
            &[start, end],
            false,
            source.seed,
            options.roughness,
            source.roughness < 2.0,
            source.stroke_style != "solid",
            None,
            source.stroke_width,
            budget,
        )?;
        drawing.decorations.push(rough::Decoration {
            drawing: head,
            fill_paint: rough::DecorationFillPaint::Stroke,
            dash: options.dash,
        });
        Ok(())
    };
    if arrowhead == "crowfoot_one" {
        push_line(first, second)?;
    } else {
        push_line(first, tip)?;
        push_line(second, tip)?;
    }
    Ok(())
}

fn arrowhead_points(
    drawing: &rough::Drawing,
    linear: &crate::normalization::NormalizedLinear,
    start: bool,
    arrowhead: &str,
    stroke_width: f64,
) -> Option<[f64; 8]> {
    let operations = &drawing.outline.operations;
    let index = if start {
        1
    } else {
        operations.len().checked_sub(1)?
    };
    let PathOperation::Cubic {
        control_1,
        control_2,
        end,
    } = operations.get(index)?
    else {
        return None;
    };
    let p0 = match operations.get(index.checked_sub(1)?)? {
        PathOperation::Move(point) => *point,
        PathOperation::Cubic { end, .. } => *end,
        PathOperation::Line(point) => *point,
    };
    let p3 = *end;
    let equation = |coordinate: fn(Point) -> f64| {
        let t = 0.3_f64;
        cube(1.0 - t) * coordinate(p3)
            + 3.0 * t * square(1.0 - t) * coordinate(*control_2)
            + 3.0 * square(t) * (1.0 - t) * coordinate(*control_1)
            + cube(t) * coordinate(p0)
    };
    let near = Point {
        x: equation(|p| p.x),
        y: equation(|p| p.y),
    };
    let tip = if start { p0 } else { p3 };
    let distance = hypot(tip.x - near.x, tip.y - near.y);
    if distance == 0.0 {
        return None;
    }
    let normal = Point {
        x: (tip.x - near.x) / distance,
        y: (tip.y - near.y) / distance,
    };
    let size: f64 = match arrowhead {
        "arrow" => 25.0,
        "diamond" | "diamond_outline" => 12.0,
        "crowfoot_many" | "crowfoot_one" | "crowfoot_one_or_many" => 20.0,
        _ => 15.0,
    };
    let endpoint_index = if start { 0 } else { linear.points.len() - 1 };
    let adjacent_index = if start { 1 } else { linear.points.len() - 2 };
    let segment_length = hypot(
        linear.points[endpoint_index][0] - linear.points[adjacent_index][0],
        linear.points[endpoint_index][1] - linear.points[adjacent_index][1],
    );
    let minimum = size.min(
        segment_length
            * if arrowhead.starts_with("diamond") {
                0.25
            } else {
                0.5
            },
    );
    let base = Point {
        x: tip.x - normal.x * minimum,
        y: tip.y - normal.y * minimum,
    };
    if matches!(arrowhead, "dot" | "circle" | "circle_outline") {
        return Some([
            tip.x,
            tip.y,
            minimum + stroke_width - 2.0,
            0.0,
            0.0,
            0.0,
            0.0,
            0.0,
        ]);
    }
    let angle = if arrowhead == "bar" {
        90.0
    } else if arrowhead == "arrow" {
        20.0
    } else {
        25.0
    };
    let rotate = |point: Point, center: Point, degrees: f64| {
        let radians = degrees.to_radians();
        let dx = point.x - center.x;
        let dy = point.y - center.y;
        Point {
            x: center.x + dx * cos(radians) - dy * sin(radians),
            y: center.y + dx * sin(radians) + dy * cos(radians),
        }
    };
    if matches!(arrowhead, "crowfoot_many" | "crowfoot_one_or_many") {
        let first = rotate(tip, base, -angle);
        let second = rotate(tip, base, angle);
        return Some([
            base.x, base.y, first.x, first.y, second.x, second.y, 0.0, 0.0,
        ]);
    }
    let first = rotate(base, tip, -angle);
    let second = rotate(base, tip, angle);
    if matches!(arrowhead, "diamond" | "diamond_outline") {
        let adjacent = linear.points[adjacent_index];
        let direction = if start {
            atan2(adjacent[1] - tip.y, adjacent[0] - tip.x)
        } else {
            atan2(tip.y - adjacent[1], tip.x - adjacent[0])
        };
        let opposite = Point {
            x: tip.x + if start { minimum * 2.0 } else { -minimum * 2.0 } * cos(direction),
            y: tip.y + if start { minimum * 2.0 } else { -minimum * 2.0 } * sin(direction),
        };
        return Some([
            tip.x, tip.y, first.x, first.y, opposite.x, opposite.y, second.x, second.y,
        ]);
    }
    Some([tip.x, tip.y, first.x, first.y, second.x, second.y, 0.0, 0.0])
}

fn translate_drawing(drawing: &mut rough::Drawing, x: f64, y: f64) {
    fn translate(path: &mut Path, x: f64, y: f64) {
        for operation in &mut path.operations {
            match operation {
                PathOperation::Move(point) | PathOperation::Line(point) => {
                    point.x += x;
                    point.y += y;
                }
                PathOperation::Cubic {
                    control_1,
                    control_2,
                    end,
                } => {
                    for point in [control_1, control_2, end] {
                        point.x += x;
                        point.y += y;
                    }
                }
            }
        }
    }
    translate(&mut drawing.outline, x, y);
    if let Some(fill) = &mut drawing.fill {
        translate(&mut fill.path, x, y);
    }
}

/// The local box the reference canvas rotates an element about: for a linear
/// element the cubic extent of its drawn Rough.js outline — what
/// `getElementAbsoluteCoords` reads from the `ShapeCache` that `renderElement`
/// has just warmed, for an elbow arrow's drawn path as for any other linear
/// kind — and the points' box when the outline draws no cubic; the element box
/// for every other kind (#177). The scene extent is measured elsewhere, by
/// `include_element_bounds`, and the SVG rotates about the points' box.
fn local_outline_bounds(
    source: &NormalizedElement,
    drawing: Option<&rough::Drawing>,
) -> (f64, f64, f64, f64) {
    let NormalizedElementKind::Linear(linear) = &source.kind else {
        return (0.0, 0.0, source.width, source.height);
    };
    if let Some(bounds) = drawing.and_then(|drawing| drawing.outline.cubic_bounds()) {
        return bounds;
    }
    linear_points_bounds(linear)
}

/// The box of a linear element's points in its own coordinates: what
/// `LinearElementEditor.getElementAbsoluteCoords` reduces the points to while
/// the `ShapeCache` is cold, before adding the element's position.
pub(super) fn linear_points_bounds(
    linear: &crate::normalization::NormalizedLinear,
) -> (f64, f64, f64, f64) {
    linear.points.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, min_y, max_x, max_y), point| {
            (
                min_x.min(point[0]),
                min_y.min(point[1]),
                max_x.max(point[0]),
                max_y.max(point[1]),
            )
        },
    )
}

pub(super) fn union_bounds(
    first: Option<(f64, f64, f64, f64)>,
    second: Option<(f64, f64, f64, f64)>,
) -> Option<(f64, f64, f64, f64)> {
    match (first, second) {
        (Some(first), Some(second)) => Some((
            first.0.min(second.0),
            first.1.min(second.1),
            first.2.max(second.2),
            first.3.max(second.3),
        )),
        (Some(bounds), None) | (None, Some(bounds)) => Some(bounds),
        (None, None) => None,
    }
}

/// The corner radius the reference clips an image element to:
/// `getCornerRadius(Math.min(element.width, element.height), element)` in both
/// its SVG `<clipPath>` and its canvas `roundRect` clip, and 0 without a
/// roundness record.
pub(super) fn image_corner_radius(
    width: f64,
    height: f64,
    roundness: Option<crate::normalization::NormalizedRoundness>,
) -> f64 {
    roundness.map_or(0.0, |roundness| corner_radius(width.min(height), roundness))
}

fn corner_radius(size: f64, roundness: crate::normalization::NormalizedRoundness) -> f64 {
    const PROPORTIONAL_RADIUS: f64 = 0.25;
    match roundness {
        crate::normalization::NormalizedRoundness::Proportional => size * PROPORTIONAL_RADIUS,
        crate::normalization::NormalizedRoundness::Adaptive { radius } => {
            radius.min(size * PROPORTIONAL_RADIUS)
        }
    }
}

/// Sutherland–Hodgman clipping of a convex `subject` polygon against a convex
/// `clip` polygon of either winding; both are page-space `(x, y)` pairs.
#[cfg(any(feature = "pdf", test))]
pub(super) fn clip_convex_polygon(
    subject: &[(f64, f64)],
    clip: &[(f64, f64)],
) -> Result<Vec<(f64, f64)>, FormatEncodeError> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(subject.len())
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_workspace",
            subject: "pdf_link_clip_input",
        })?;
    output.extend_from_slice(subject);
    let orientation = clip
        .iter()
        .zip(clip.iter().cycle().skip(1))
        .take(clip.len())
        .map(|(left, right)| left.0 * right.1 - right.0 * left.1)
        .sum::<f64>()
        .signum();
    for (edge_start, edge_end) in clip
        .iter()
        .copied()
        .zip(clip.iter().copied().cycle().skip(1))
        .take(clip.len())
    {
        let input = output;
        output = Vec::new();
        output
            .try_reserve(input.len().saturating_add(1))
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_workspace",
                subject: "pdf_clipped_link_polygon",
            })?;
        let Some(mut previous) = input.last().copied() else {
            break;
        };
        let mut previous_inside =
            polygon_half_plane(previous, edge_start, edge_end) * orientation >= 0.0;
        for current in input {
            let current_inside =
                polygon_half_plane(current, edge_start, edge_end) * orientation >= 0.0;
            if current_inside != previous_inside {
                output.push(line_intersection(previous, current, edge_start, edge_end));
            }
            if current_inside {
                output.push(current);
            }
            previous = current;
            previous_inside = current_inside;
        }
    }
    Ok(output)
}

#[cfg(any(feature = "pdf", test))]
fn polygon_half_plane(point: (f64, f64), edge_start: (f64, f64), edge_end: (f64, f64)) -> f64 {
    (edge_end.0 - edge_start.0) * (point.1 - edge_start.1)
        - (edge_end.1 - edge_start.1) * (point.0 - edge_start.0)
}

#[cfg(any(feature = "pdf", test))]
fn line_intersection(
    start: (f64, f64),
    end: (f64, f64),
    edge_start: (f64, f64),
    edge_end: (f64, f64),
) -> (f64, f64) {
    let start_distance = polygon_half_plane(start, edge_start, edge_end);
    let end_distance = polygon_half_plane(end, edge_start, edge_end);
    let denominator = start_distance - end_distance;
    let ratio = if denominator == 0.0 {
        0.0
    } else {
        (start_distance / denominator).clamp(0.0, 1.0)
    };
    // This helper is reached only for an inside/outside transition: the
    // intersection is a convex combination of the admitted endpoints. Clamp
    // the final rounding (not the source geometry) to that closed interval so
    // no clipping pass can manufacture a coordinate beyond its input range.
    (
        (start.0 + (end.0 - start.0) * ratio).clamp(start.0.min(end.0), start.0.max(end.0)),
        (start.1 + (end.1 - start.1) * ratio).clamp(start.1.min(end.1), start.1.max(end.1)),
    )
}

/// ECMAScript `Math.round`: the closest integral value, a tie going toward
/// positive infinity, so `-2.5` is `-2` where `f64::round` gives `-3`. A
/// result of negative zero is canonicalized to zero. The fraction is compared
/// rather than `(value + 0.5).floor()` taken, which would carry
/// `0.49999999999999994` up to 1.
pub(super) fn ecmascript_round(value: f64) -> f64 {
    let floor = value.floor();
    let rounded = if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    };
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

#[cfg(test)]
mod tests {
    use super::ecmascript_round;

    #[test]
    fn clipping_intermediates_remain_in_the_admitted_convex_envelope() {
        // Includes nearly parallel edges and the hard coordinate maximum.
        for limit in [1.0, 100.0, 1e12] {
            for epsilon in [0.0, f64::EPSILON, 0.00001, 0.5] {
                let mut polygon = vec![
                    (-limit, -limit),
                    (limit, -limit),
                    (limit, limit),
                    (-limit, limit),
                ];
                for clip in [
                    [
                        (-limit, -limit * epsilon),
                        (limit, 0.0),
                        (limit, limit),
                        (-limit, limit),
                    ],
                    [
                        (-limit, -limit),
                        (limit * epsilon, -limit),
                        (0.0, limit),
                        (-limit, limit),
                    ],
                ] {
                    polygon = super::clip_convex_polygon(&polygon, &clip).unwrap();
                    assert!(polygon.iter().all(|(x, y)| x.is_finite()
                        && y.is_finite()
                        && x.abs() <= limit
                        && y.abs() <= limit));
                }
            }
        }
    }

    #[test]
    fn ecmascript_round_carries_ties_up_and_leaves_the_near_half_alone() {
        assert_eq!(ecmascript_round(2.5), 3.0);
        assert_eq!(ecmascript_round(-1.5), -1.0);
        assert_eq!(ecmascript_round(-2.5), -2.0);
        assert_eq!(ecmascript_round(-0.5).to_bits(), 0.0_f64.to_bits());
        assert_eq!(ecmascript_round(40.4), 40.0);
        // `(value + 0.5).floor()` would carry the largest double below one
        // half up to 1; `Math.round` does not.
        assert_eq!(ecmascript_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(ecmascript_round(96.0), 96.0);
    }
}
